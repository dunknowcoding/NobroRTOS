//! MPU protection regions on real silicon (M167): region 0 turns a 256-byte RAM block
//! read-only; writing it raises MemManage; the handler counts the fault, disables the
//! MPU, and the retried store completes. Proves hardware-enforced memory isolation with
//! clean recovery. NOBRO_MPU_REPORT (mem32).
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m_rt::{entry, exception};
use defmt_rtt as _;
use panic_halt as _;

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
    checksum: 0,
};

/// The protected block: 256-byte aligned so it can be an MPU region base.
#[repr(align(256))]
struct Guarded([u32; 64]);
static mut GUARDED: Guarded = Guarded([0; 64]);

static FAULTS: AtomicU32 = AtomicU32::new(0);

const MPU_CTRL: u32 = 0xE000_ED94;
const MPU_RNR: u32 = 0xE000_ED98;
const MPU_RBAR: u32 = 0xE000_ED9C;
const MPU_RASR: u32 = 0xE000_EDA0;
const SHCSR: u32 = 0xE000_ED24;

unsafe fn wr(a: u32, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v);
}
unsafe fn rd(a: u32) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}

/// Region 0: the guarded block, read-only for all, SRAM attributes, no execute.
unsafe fn mpu_protect(base: u32) {
    wr(MPU_RNR, 0);
    wr(MPU_RBAR, base);
    // XN | AP=RO(0b110) | TEX=0,S=1,C=1,B=1 | SIZE=7 (256 B) | ENABLE
    let rasr = (1 << 28) | (0b110 << 24) | (0b111 << 16) | (7 << 1) | 1;
    wr(MPU_RASR, rasr);
    cortex_m::asm::dsb();
    // MemManage fault enabled; MPU on with the default map for everything else.
    wr(SHCSR, rd(SHCSR) | (1 << 16));
    wr(MPU_CTRL, 0b101); // ENABLE | PRIVDEFENA
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
}

#[exception]
fn MemoryManagement() {
    let n = FAULTS.fetch_add(1, Ordering::AcqRel) + 1;
    unsafe {
        wr(MPU_CTRL, 0); // drop protection so the faulting store retries cleanly
        wr(0xE000_ED28, 0xFF); // clear MMFSR status bits (write-1-to-clear)
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
        core::ptr::write_volatile(core::ptr::addr_of_mut!(NOBRO_MPU_REPORT.faults_caught), n);
        if n > 3 {
            // fault storm: park with diagnostics instead of looping forever
            core::ptr::write_volatile(core::ptr::addr_of_mut!(NOBRO_MPU_REPORT.checksum), 0xDEAD_0001);
            loop {}
        }
    }
}

#[exception]
unsafe fn HardFault(ef: &cortex_m_rt::ExceptionFrame) -> ! {
    // escalation diagnostics: record the stacked PC + CFSR so the report shows why
    core::ptr::write_volatile(core::ptr::addr_of_mut!(NOBRO_MPU_REPORT.faults_caught), 0xFFFF);
    core::ptr::write_volatile(core::ptr::addr_of_mut!(NOBRO_MPU_REPORT.write_after_ok), ef.pc());
    core::ptr::write_volatile(core::ptr::addr_of_mut!(NOBRO_MPU_REPORT.checksum), rd(0xE000_ED28));
    loop {}
}

#[entry]
fn main() -> ! {
    // The app lives at 0x1000 behind the bootloader, which leaves VTOR pointing at its
    // own table - so OUR fault handlers are never reached. Point VTOR at the app's
    // vector table before arming anything that faults.
    unsafe {
        wr(0xE000_ED08, 0x1000); // SCB->VTOR = app vector table
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
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
            version: 1,
            completed: 0,
            all_pass: 0,
            write_before_ok,
            faults_caught: 0,
            write_after_ok: 0,
            checksum: 0xAAAA_0001,
        };
    }

    // Phase 2: protect, then write - must fault exactly once, then complete after the
    // handler drops the region.
    unsafe {
        mpu_protect(base);
        core::ptr::write_volatile(core::ptr::addr_of_mut!(NOBRO_MPU_REPORT.checksum), 0xAAAA_0002); // MPU armed
        core::ptr::write_volatile(&mut GUARDED.0[1], 0x3333_4444);
        core::ptr::write_volatile(core::ptr::addr_of_mut!(NOBRO_MPU_REPORT.checksum), 0xAAAA_0003); // survived
    }
    let faults_caught = FAULTS.load(Ordering::Acquire);
    let write_after_ok =
        u32::from(unsafe { core::ptr::read_volatile(&GUARDED.0[1]) } == 0x3333_4444);

    let pass = write_before_ok == 1 && faults_caught == 1 && write_after_ok == 1;
    let ap = u32::from(pass);
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ write_before_ok ^ faults_caught ^ write_after_ok;
    unsafe {
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_MPU_REPORT),
            Report {
                magic: MAGIC,
                version: 1,
                completed: 1,
                all_pass: ap,
                write_before_ok,
                faults_caught,
                write_after_ok,
                checksum: cs,
            },
        );
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
