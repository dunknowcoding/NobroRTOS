//! Opt-in Cortex-M4F PSP/PendSV context-switch port.
//!
//! This is a mechanism, not a scheduler or isolation boundary. The kernel owns
//! admission, task state, recovery, and stack guards. This module owns only the
//! architectural frame: R4-R11, EXC_RETURN, 8-byte PSP alignment, and S16-S31
//! when an extended FPU frame is active. Hardware stacks the basic frame and,
//! under lazy FPU rules, S0-S15/FPSCR.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::priority_ceiling::PriorityCeiling;

const EXC_RETURN_THREAD_PSP_BASIC: u32 = 0xFFFF_FFFD;
const XPSR_THUMB: u32 = 1 << 24;
const SOFTWARE_WORDS: usize = 8; // R4-R11
const HARDWARE_WORDS: usize = 8; // R0-R3,R12,LR,PC,xPSR

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextSwitchError {
    StackTooSmall,
    StackMisaligned,
    ContextNotInitialized,
    AlreadyStarted,
    SwitchAlreadyPending,
    CurrentContextMismatch,
    InvalidPendSvPriority,
    PendSvWouldPreemptCeiling,
    NotConfigured,
}

/// Saved architecture state. Atomics make concurrent PendSV writes legal while
/// thread/ISR code retains a shared reference to the record.
#[repr(C, align(8))]
pub struct ContextRecord {
    psp: AtomicU32,
    exc_return: AtomicU32,
    basepri: AtomicU32,
}

impl ContextRecord {
    pub const fn empty() -> Self {
        Self {
            psp: AtomicU32::new(0),
            exc_return: AtomicU32::new(EXC_RETURN_THREAD_PSP_BASIC),
            basepri: AtomicU32::new(0),
        }
    }

    pub fn saved_psp(&self) -> u32 {
        self.psp.load(Ordering::Acquire)
    }

    pub fn exc_return(&self) -> u32 {
        self.exc_return.load(Ordering::Acquire)
    }

    /// Build the first software + hardware exception frame at the top of an
    /// owned stack. `entry(arg)` begins in unprivileged-neutral thread mode on
    /// PSP; privilege/MPU policy is intentionally not changed here.
    ///
    /// # Safety
    /// `stack` must remain exclusively owned by this context while runnable.
    pub unsafe fn initialize(
        &self,
        stack: &'static mut [u8],
        entry: extern "C" fn(usize) -> !,
        arg: usize,
    ) -> Result<(), ContextSwitchError> {
        if stack.as_ptr() as usize & 7 != 0 || stack.len() & 7 != 0 {
            return Err(ContextSwitchError::StackMisaligned);
        }
        let frame_bytes = (SOFTWARE_WORDS + HARDWARE_WORDS) * core::mem::size_of::<u32>();
        if stack.len() < frame_bytes + 32 {
            return Err(ContextSwitchError::StackTooSmall);
        }
        let top = stack.as_mut_ptr().add(stack.len()) as usize;
        let frame = (top - frame_bytes) as *mut u32;
        for index in 0..SOFTWARE_WORDS {
            frame.add(index).write_volatile(0);
        }
        let hardware = frame.add(SOFTWARE_WORDS);
        hardware.add(0).write_volatile(arg as u32); // R0
        hardware.add(1).write_volatile(0); // R1
        hardware.add(2).write_volatile(0); // R2
        hardware.add(3).write_volatile(0); // R3
        hardware.add(4).write_volatile(0); // R12
        hardware
            .add(5)
            .write_volatile(nobro_slice_task_returned as *const () as usize as u32); // LR
        hardware
            .add(6)
            .write_volatile((entry as *const () as usize as u32) | 1); // PC/Thumb
        hardware.add(7).write_volatile(XPSR_THUMB); // xPSR
        self.psp.store(frame as u32, Ordering::Release);
        self.exc_return
            .store(EXC_RETURN_THREAD_PSP_BASIC, Ordering::Release);
        self.basepri.store(0, Ordering::Release);
        Ok(())
    }
}

impl Default for ContextRecord {
    fn default() -> Self {
        Self::empty()
    }
}

#[no_mangle]
static NOBRO_SLICE_CURRENT_RECORD: AtomicU32 = AtomicU32::new(0);
#[no_mangle]
static NOBRO_SLICE_NEXT_RECORD: AtomicU32 = AtomicU32::new(0);
#[no_mangle]
static NOBRO_SLICE_SWITCH_ACTIVE: AtomicU32 = AtomicU32::new(0);
#[no_mangle]
static NOBRO_SLICE_PENDSV_PRIORITY: AtomicU32 = AtomicU32::new(0);

pub struct CortexMSliceSwitch;

impl CortexMSliceSwitch {
    /// Start the first prepared PSP context on the next PendSV exception.
    ///
    /// # Safety
    /// `next` and its stack must remain alive and exclusively context-owned.
    pub unsafe fn start(
        next: &'static ContextRecord,
        pendsv_logical_priority: u8,
        ceiling: PriorityCeiling,
    ) -> Result<(), ContextSwitchError> {
        if next.saved_psp() == 0 {
            return Err(ContextSwitchError::ContextNotInitialized);
        }
        if NOBRO_SLICE_CURRENT_RECORD.load(Ordering::Acquire) != 0 {
            return Err(ContextSwitchError::AlreadyStarted);
        }
        // Logical zero is normally reserved for the most urgent platform
        // interrupt. PendSV must be at or below the kernel ceiling: otherwise
        // it could switch tasks halfway through a process-wide critical-section
        // transaction and expose partially updated shared state.
        if pendsv_logical_priority == 0 || pendsv_logical_priority >= 8 {
            return Err(ContextSwitchError::InvalidPendSvPriority);
        }
        let raw = pendsv_logical_priority << 5;
        if raw < ceiling.raw() {
            return Err(ContextSwitchError::PendSvWouldPreemptCeiling);
        }
        NOBRO_SLICE_PENDSV_PRIORITY.store(u32::from(raw), Ordering::Release);
        Self::queue(core::ptr::null(), next)
    }

    /// Save `current` and restore `next` on the next PendSV exception.
    ///
    /// # Safety
    /// Both records and their stacks must remain live; `current` must be the
    /// context presently running on PSP.
    pub unsafe fn switch(
        current: &'static ContextRecord,
        next: &'static ContextRecord,
    ) -> Result<(), ContextSwitchError> {
        if next.saved_psp() == 0 {
            return Err(ContextSwitchError::ContextNotInitialized);
        }
        let current_ptr = current as *const ContextRecord as u32;
        let observed = NOBRO_SLICE_CURRENT_RECORD.load(Ordering::Acquire);
        if observed != current_ptr {
            return Err(ContextSwitchError::CurrentContextMismatch);
        }
        Self::queue(current, next)
    }

    unsafe fn queue(
        current: *const ContextRecord,
        next: &'static ContextRecord,
    ) -> Result<(), ContextSwitchError> {
        let pendsv_priority = NOBRO_SLICE_PENDSV_PRIORITY.load(Ordering::Acquire);
        if pendsv_priority == 0 {
            return Err(ContextSwitchError::NotConfigured);
        }
        let next_ptr = next as *const ContextRecord as u32;
        if NOBRO_SLICE_SWITCH_ACTIVE
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ContextSwitchError::SwitchAlreadyPending);
        }
        if NOBRO_SLICE_NEXT_RECORD
            .compare_exchange(0, next_ptr, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            NOBRO_SLICE_SWITCH_ACTIVE.store(0, Ordering::Release);
            return Err(ContextSwitchError::SwitchAlreadyPending);
        }
        NOBRO_SLICE_CURRENT_RECORD.store(current as u32, Ordering::Release);
        cortex_m::asm::dmb();
        let mut peripherals = cortex_m::Peripherals::steal();
        peripherals.SCB.set_priority(
            cortex_m::peripheral::scb::SystemHandler::PendSV,
            pendsv_priority as u8,
        );
        cortex_m::peripheral::SCB::set_pendsv();
        Ok(())
    }

    pub fn current_record_address() -> u32 {
        NOBRO_SLICE_CURRENT_RECORD.load(Ordering::Acquire)
    }
}

#[no_mangle]
extern "C" fn nobro_slice_task_returned() -> ! {
    loop {
        cortex_m::asm::udf();
    }
}

core::arch::global_asm!(
    r#"
    .syntax unified
    .thumb
    .fpu fpv4-sp-d16
    .global PendSV
    .type PendSV,%function
PendSV:
    mrs     r0, psp
    isb
    ldr     r3, =NOBRO_SLICE_CURRENT_RECORD
    ldr     r2, [r3]
    cbz     r2, 1f
    tst     lr, #0x10
    bne     0f
    vstmdb  r0!, {{s16-s31}}
0:
    stmdb   r0!, {{r4-r11}}
    str     r0, [r2, #0]
    str     lr, [r2, #4]
    mrs     r1, basepri
    str     r1, [r2, #8]
1:
    ldr     r3, =NOBRO_SLICE_NEXT_RECORD
    ldr     r2, [r3]
    cbz     r2, 3f
    ldr     r0, [r2, #0]
    ldr     lr, [r2, #4]
    ldr     r1, [r2, #8]
    ldmia   r0!, {{r4-r11}}
    tst     lr, #0x10
    bne     2f
    vldmia  r0!, {{s16-s31}}
2:
    msr     psp, r0
    msr     basepri, r1
    dsb
    isb
    movs    r0, #0
    str     r0, [r3]
    ldr     r3, =NOBRO_SLICE_CURRENT_RECORD
    str     r2, [r3]
    ldr     r3, =NOBRO_SLICE_SWITCH_ACTIVE
    str     r0, [r3]
3:
    bx      lr
    .size PendSV, .-PendSV
"#
);
