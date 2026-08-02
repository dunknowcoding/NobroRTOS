//! Opt-in Cortex-M0/M0+ PSP/PendSV context-switch mechanism.
//!
//! This is the privileged-only second architecture for bounded P-SLICE. It
//! saves R4-R11 around the hardware exception frame, retains 8-byte PSP
//! alignment, and commits a queued switch only in PendSV. Cortex-M0/M0+ has no
//! BASEPRI and common RP2040/SAMD21 parts have no MPU, so this module makes no
//! isolation claim; exact ports must supply their own interrupt-ceiling and
//! peripheral-ownership admission.

use portable_atomic::{AtomicU32, Ordering};

const EXC_RETURN_THREAD_PSP: u32 = 0xFFFF_FFFD;
const XPSR_THUMB: u32 = 1 << 24;
const CONTROL_SPSEL: u32 = 1 << 1;
const SOFTWARE_WORDS: usize = 8;
const HARDWARE_WORDS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CortexM0SwitchError {
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

#[repr(C, align(8))]
pub struct CortexM0ContextRecord {
    psp: AtomicU32,
    exc_return: AtomicU32,
    control: AtomicU32,
}

impl CortexM0ContextRecord {
    pub const fn empty() -> Self {
        Self {
            psp: AtomicU32::new(0),
            exc_return: AtomicU32::new(EXC_RETURN_THREAD_PSP),
            control: AtomicU32::new(CONTROL_SPSEL),
        }
    }

    pub fn saved_psp(&self) -> u32 {
        self.psp.load(Ordering::Acquire)
    }

    pub fn exc_return(&self) -> u32 {
        self.exc_return.load(Ordering::Acquire)
    }

    /// Build the initial software and hardware exception frames.
    ///
    /// # Safety
    /// `stack` must remain live and exclusively owned by this context while it
    /// can be selected. `entry` must not return.
    pub unsafe fn initialize(
        &self,
        stack: &'static mut [u8],
        entry: extern "C" fn(usize) -> !,
        arg: usize,
    ) -> Result<(), CortexM0SwitchError> {
        if stack.as_ptr() as usize & 7 != 0 || stack.len() & 7 != 0 {
            return Err(CortexM0SwitchError::StackMisaligned);
        }
        let frame_bytes = (SOFTWARE_WORDS + HARDWARE_WORDS) * core::mem::size_of::<u32>();
        if stack.len() < frame_bytes + 32 {
            return Err(CortexM0SwitchError::StackTooSmall);
        }
        let top = stack.as_mut_ptr().add(stack.len()) as usize;
        let frame = (top - frame_bytes) as *mut u32;
        for index in 0..SOFTWARE_WORDS {
            frame.add(index).write_volatile(0);
        }
        let hardware = frame.add(SOFTWARE_WORDS);
        hardware.add(0).write_volatile(arg as u32);
        hardware.add(1).write_volatile(0);
        hardware.add(2).write_volatile(0);
        hardware.add(3).write_volatile(0);
        hardware.add(4).write_volatile(0);
        hardware
            .add(5)
            .write_volatile(nobro_m0_slice_task_returned as *const () as usize as u32);
        hardware
            .add(6)
            .write_volatile((entry as *const () as usize as u32) | 1);
        hardware.add(7).write_volatile(XPSR_THUMB);
        self.psp.store(frame as u32, Ordering::Release);
        self.exc_return
            .store(EXC_RETURN_THREAD_PSP, Ordering::Release);
        self.control.store(CONTROL_SPSEL, Ordering::Release);
        Ok(())
    }
}

impl Default for CortexM0ContextRecord {
    fn default() -> Self {
        Self::empty()
    }
}

#[no_mangle]
static NOBRO_M0_SLICE_CURRENT_RECORD: AtomicU32 = AtomicU32::new(0);
#[no_mangle]
static NOBRO_M0_SLICE_NEXT_RECORD: AtomicU32 = AtomicU32::new(0);
#[no_mangle]
static NOBRO_M0_SLICE_SWITCH_ACTIVE: AtomicU32 = AtomicU32::new(0);
#[no_mangle]
static NOBRO_M0_SLICE_PENDSV_PRIORITY: AtomicU32 = AtomicU32::new(0);

pub struct CortexM0SliceSwitch;

impl CortexM0SliceSwitch {
    /// Start the first prepared context. Logical priority is 0..3 on the exact
    /// RP2040/SAMD21 class; zero is reserved here for the most urgent IRQs.
    ///
    /// # Safety
    /// `next` and its stack must remain live and exclusively context-owned.
    pub unsafe fn start(
        next: &'static CortexM0ContextRecord,
        pendsv_logical_priority: u8,
        minimum_logical_priority: u8,
    ) -> Result<(), CortexM0SwitchError> {
        if next.saved_psp() == 0 {
            return Err(CortexM0SwitchError::ContextNotInitialized);
        }
        if NOBRO_M0_SLICE_CURRENT_RECORD.load(Ordering::Acquire) != 0 {
            return Err(CortexM0SwitchError::AlreadyStarted);
        }
        if pendsv_logical_priority == 0 || pendsv_logical_priority >= 4 {
            return Err(CortexM0SwitchError::InvalidPendSvPriority);
        }
        if pendsv_logical_priority < minimum_logical_priority {
            return Err(CortexM0SwitchError::PendSvWouldPreemptCeiling);
        }
        NOBRO_M0_SLICE_PENDSV_PRIORITY
            .store(u32::from(pendsv_logical_priority << 6), Ordering::Release);
        Self::queue(core::ptr::null(), next)
    }

    /// Queue a save/restore transition; the architectural state changes only
    /// after PendSV runs.
    ///
    /// # Safety
    /// Both records and stacks must remain live, and `current` must be the PSP
    /// context presently executing.
    pub unsafe fn switch(
        current: &'static CortexM0ContextRecord,
        next: &'static CortexM0ContextRecord,
    ) -> Result<(), CortexM0SwitchError> {
        if next.saved_psp() == 0 {
            return Err(CortexM0SwitchError::ContextNotInitialized);
        }
        let current_ptr = current as *const CortexM0ContextRecord as u32;
        if NOBRO_M0_SLICE_CURRENT_RECORD.load(Ordering::Acquire) != current_ptr {
            return Err(CortexM0SwitchError::CurrentContextMismatch);
        }
        Self::queue(current, next)
    }

    unsafe fn queue(
        current: *const CortexM0ContextRecord,
        next: &'static CortexM0ContextRecord,
    ) -> Result<(), CortexM0SwitchError> {
        let priority = NOBRO_M0_SLICE_PENDSV_PRIORITY.load(Ordering::Acquire);
        if priority == 0 {
            return Err(CortexM0SwitchError::NotConfigured);
        }
        let next_ptr = next as *const CortexM0ContextRecord as u32;
        if NOBRO_M0_SLICE_SWITCH_ACTIVE
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(CortexM0SwitchError::SwitchAlreadyPending);
        }
        if NOBRO_M0_SLICE_NEXT_RECORD
            .compare_exchange(0, next_ptr, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            NOBRO_M0_SLICE_SWITCH_ACTIVE.store(0, Ordering::Release);
            return Err(CortexM0SwitchError::SwitchAlreadyPending);
        }
        NOBRO_M0_SLICE_CURRENT_RECORD.store(current as u32, Ordering::Release);
        cortex_m::asm::dmb();
        let mut peripherals = cortex_m::Peripherals::steal();
        peripherals.SCB.set_priority(
            cortex_m::peripheral::scb::SystemHandler::PendSV,
            priority as u8,
        );
        cortex_m::peripheral::SCB::set_pendsv();
        Ok(())
    }

    pub fn current_record_address() -> u32 {
        NOBRO_M0_SLICE_CURRENT_RECORD.load(Ordering::Acquire)
    }
}

#[no_mangle]
extern "C" fn nobro_m0_slice_task_returned() -> ! {
    cortex_m::asm::udf()
}

#[cfg(target_arch = "arm")]
core::arch::global_asm!(
    r#"
    .syntax unified
    .thumb
    .global PendSV
    .type PendSV,%function
PendSV:
    mrs     r0, psp
    ldr     r3, =NOBRO_M0_SLICE_CURRENT_RECORD
    ldr     r2, [r3]
    cmp     r2, #0
    beq     1f
    subs    r0, #32
    stmia   r0!, {{r4-r7}}
    mov     r4, r8
    mov     r5, r9
    mov     r6, r10
    mov     r7, r11
    stmia   r0!, {{r4-r7}}
    subs    r0, #32
    str     r0, [r2, #0]
    mov     r1, lr
    str     r1, [r2, #4]
    mrs     r1, control
    str     r1, [r2, #8]
1:
    ldr     r3, =NOBRO_M0_SLICE_NEXT_RECORD
    ldr     r2, [r3]
    cmp     r2, #0
    beq     3f
    ldr     r0, [r2, #0]
    ldr     r1, [r2, #4]
    mov     lr, r1
    ldr     r1, [r2, #8]
    adds    r0, #16
    ldmia   r0!, {{r4-r7}}
    mov     r8, r4
    mov     r9, r5
    mov     r10, r6
    mov     r11, r7
    subs    r0, #32
    ldmia   r0!, {{r4-r7}}
    adds    r0, #16
    msr     psp, r0
    msr     control, r1
    dsb
    isb
    movs    r0, #0
    str     r0, [r3]
    ldr     r3, =NOBRO_M0_SLICE_CURRENT_RECORD
    str     r2, [r3]
    ldr     r3, =NOBRO_M0_SLICE_SWITCH_ACTIVE
    str     r0, [r3]
3:
    bx      lr
    .size PendSV, .-PendSV
"#
);

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn entry(_arg: usize) -> ! {
        loop {
            core::hint::spin_loop();
        }
    }

    #[test]
    fn frame_is_bounded_aligned_and_contains_entry_contract() {
        #[repr(align(8))]
        struct Stack([u8; 128]);
        let mut stack = Stack([0; 128]);
        let record = CortexM0ContextRecord::empty();
        unsafe {
            let owned: &'static mut [u8] = core::mem::transmute(&mut stack.0[..]);
            record.initialize(owned, entry, 0x1234).unwrap();
        }
        assert_ne!(record.saved_psp(), 0);
        assert_eq!(record.saved_psp() & 7, 0);
        assert_eq!(record.exc_return(), EXC_RETURN_THREAD_PSP);
    }

    #[test]
    fn start_rejects_uninitialized_and_unsafe_priorities_before_mutation() {
        static RECORD: CortexM0ContextRecord = CortexM0ContextRecord::empty();
        assert_eq!(
            unsafe { CortexM0SliceSwitch::start(&RECORD, 3, 2) },
            Err(CortexM0SwitchError::ContextNotInitialized)
        );
    }
}
