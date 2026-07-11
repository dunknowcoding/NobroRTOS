//! MPU protection as a kernel profile on real silicon (M167, upgraded for
//! MEM-02): `nobro_hal::mpu::KernelMpuPlan` turns a 256-byte RAM block
//! read-only; writing it raises MemManage; the handler decodes an attributable
//! `MpuFaultRecord` (CFSR/MMFAR/stacked PC + the in-flight module code),
//! disables the MPU, and the retried store completes. Proves hardware-enforced
//! memory isolation with clean recovery AND fault attribution.
//! NOBRO_MPU_REPORT (mem32).
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m_rt::{entry, exception};
use defmt_rtt as _;
use panic_halt as _;

use nobro_hal::mpu::{KernelMpuPlan, MpuFaultRecord, MpuRegionSpec};
use nobro_kernel::{module_code, ModuleId};

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    write_before_ok: u32,
    faults_caught: u32,
    write_after_ok: u32,
    fault_module: u32,
    fault_was_data_access: u32,
    fault_addr: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E4D_5055; // "NMPU"

#[no_mangle]
#[used]
static mut NOBRO_MPU_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    write_before_ok: 0,
    faults_caught: 0,
    write_after_ok: 0,
    fault_module: 0,
    fault_was_data_access: 0,
    fault_addr: 0,
    checksum: 0,
};

/// The protected block: 256-byte aligned so it can be an MPU region base.
#[repr(align(256))]
struct Guarded([u32; 64]);
static mut GUARDED: Guarded = Guarded([0; 64]);

static FAULTS: AtomicU32 = AtomicU32::new(0);
static FAULT_MODULE: AtomicU32 = AtomicU32::new(0);
static FAULT_DATA: AtomicU32 = AtomicU32::new(0);
static FAULT_ADDR: AtomicU32 = AtomicU32::new(0);
/// The module identity "executing" during the protected phase — in the full
/// executor this comes from `ExecutionSentinel`; the demo pins it directly.
static IN_FLIGHT: AtomicU32 = AtomicU32::new(0);

const CFSR: u32 = 0xE000_ED28;
const MMFAR: u32 = 0xE000_ED34;

unsafe fn rd(a: u32) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}
unsafe fn wr(a: u32, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v);
}

/// App base per bootloader layout: fault handlers only fire once VTOR points
/// at OUR vector table (see the VTOR gotcha).
const APP_BASE: u32 = if cfg!(feature = "board-nicenano-s140") {
    0x26000
} else {
    0x1000
};

#[exception]
fn MemoryManagement() {
    let n = FAULTS.fetch_add(1, Ordering::AcqRel) + 1;
    unsafe {
        // Capture and attribute BEFORE clearing status: this is the service's
        // fault-frame story on real registers. (The stacked PC needs the
        // exception frame; cortex-m-rt's non-unsafe handler hides it, so the
        // demo records the MMFAR-based record here and the stacked PC via the
        // HardFault escalation path only.)
        let record = MpuFaultRecord::decode_mem_manage(
            rd(CFSR),
            rd(MMFAR),
            0,
            IN_FLIGHT.load(Ordering::Acquire),
        );
        FAULT_MODULE.store(record.module_code, Ordering::Release);
        FAULT_DATA.store(u32::from(record.data_access), Ordering::Release);
        FAULT_ADDR.store(record.fault_address, Ordering::Release);

        // Recovery: drop protection so the faulting store retries cleanly.
        KernelMpuPlan::<1>::disable();
        wr(CFSR, 0xFF); // clear MMFSR status bits (write-1-to-clear)
        core::ptr::write_volatile(core::ptr::addr_of_mut!(NOBRO_MPU_REPORT.faults_caught), n);
        if n > 3 {
            // fault storm: park with diagnostics instead of looping forever
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!(NOBRO_MPU_REPORT.checksum),
                0xDEAD_0001,
            );
            loop {}
        }
    }
}

#[exception]
unsafe fn HardFault(ef: &cortex_m_rt::ExceptionFrame) -> ! {
    // escalation diagnostics: record the stacked PC + CFSR so the report shows why
    core::ptr::write_volatile(
        core::ptr::addr_of_mut!(NOBRO_MPU_REPORT.faults_caught),
        0xFFFF,
    );
    core::ptr::write_volatile(
        core::ptr::addr_of_mut!(NOBRO_MPU_REPORT.write_after_ok),
        ef.pc(),
    );
    core::ptr::write_volatile(core::ptr::addr_of_mut!(NOBRO_MPU_REPORT.checksum), rd(CFSR));
    loop {}
}

#[entry]
fn main() -> ! {
    // The app lives behind the bootloader, which leaves VTOR pointing at its
    // own table - so OUR fault handlers are never reached. Point VTOR at the
    // app's vector table before arming anything that faults.
    unsafe {
        wr(0xE000_ED08, APP_BASE); // SCB->VTOR
        core::arch::asm!("dsb", "isb", options(nostack, preserves_flags));
    }
    let base = core::ptr::addr_of_mut!(GUARDED) as u32;

    // Phase 1: MPU off - the block is writable.
    unsafe {
        core::ptr::write_volatile(&mut GUARDED.0[0], 0x1111_2222);
    }
    let write_before_ok =
        u32::from(unsafe { core::ptr::read_volatile(&GUARDED.0[0]) } == 0x1111_2222);

    // Provisional report: proves main started and phase 1 finished.
    unsafe {
        NOBRO_MPU_REPORT = Report {
            magic: MAGIC,
            version: 2,
            completed: 0,
            all_pass: 0,
            write_before_ok,
            faults_caught: 0,
            write_after_ok: 0,
            fault_module: 0,
            fault_was_data_access: 0,
            fault_addr: 0,
            checksum: 0xAAAA_0001,
        };
    }

    // Phase 2: install a permissive (PRIVDEFENA) plan whose only region turns
    // the guarded block read-only, then write - must fault exactly once with a
    // record attributed to the in-flight module, then complete after the
    // handler drops protection.
    let mut plan = KernelMpuPlan::<1>::new(false);
    plan.add(MpuRegionSpec {
        base,
        size_bytes: 256,
        access: nobro_hal::mpu::MpuAccess::ReadOnly,
        executable: false,
        device: false,
    })
    .unwrap();
    IN_FLIGHT.store(module_code(ModuleId::Sensor), Ordering::Release);
    unsafe {
        plan.install().unwrap();
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_MPU_REPORT.checksum),
            0xAAAA_0002,
        ); // MPU armed
        core::ptr::write_volatile(&mut GUARDED.0[1], 0x3333_4444);
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_MPU_REPORT.checksum),
            0xAAAA_0003,
        ); // survived
    }
    IN_FLIGHT.store(0, Ordering::Release);
    let faults_caught = FAULTS.load(Ordering::Acquire);
    let write_after_ok =
        u32::from(unsafe { core::ptr::read_volatile(&GUARDED.0[1]) } == 0x3333_4444);
    let fault_module = FAULT_MODULE.load(Ordering::Acquire);
    let fault_was_data_access = FAULT_DATA.load(Ordering::Acquire);
    let fault_addr = FAULT_ADDR.load(Ordering::Acquire);

    let pass = write_before_ok == 1
        && faults_caught == 1
        && write_after_ok == 1
        && fault_module == module_code(ModuleId::Sensor)
        && fault_was_data_access == 1;
    let ap = u32::from(pass);
    let cs = MAGIC
        ^ 2
        ^ 1
        ^ ap
        ^ write_before_ok
        ^ faults_caught
        ^ write_after_ok
        ^ fault_module
        ^ fault_was_data_access
        ^ fault_addr;
    unsafe {
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_MPU_REPORT),
            Report {
                magic: MAGIC,
                version: 2,
                completed: 1,
                all_pass: ap,
                write_before_ok,
                faults_caught,
                write_after_ok,
                fault_module,
                fault_was_data_access,
                fault_addr,
                checksum: cs,
            },
        );
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
