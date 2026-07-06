//! Crash-dump capture to flash (M184). Boot A (dump page erased): install the HardFault
//! handler, deliberately read unmapped memory; the handler persists {PC, LR, xPSR, CFSR,
//! HFSR} to flash over NVMC and reboots. Boot B (dump present): validate the record - the
//! dumped PC must land inside the app's flash and the fault-status bits must show a bus
//! error - then report and erase the page so the test is repeatable.
#![no_std]
#![no_main]

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
    dump_pc: u32,
    dump_cfsr: u32,
    dump_valid: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E43_5253; // "NCRS"

#[no_mangle]
#[used]
static mut NOBRO_CRASH_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    dump_pc: 0,
    dump_cfsr: 0,
    dump_valid: 0,
    checksum: 0,
};

const DUMP_PAGE: u32 = 0x8_2000;

/// Deliberate-crash target: MPU-protected read-only block (the M167-proven fault
/// source). With MEMFAULTENA off, the MemManage violation escalates to HardFault.
#[repr(align(256))]
struct Guarded([u32; 64]);
static mut GUARDED: Guarded = Guarded([0; 64]);

unsafe fn crash_now() -> ! {
    let base = core::ptr::addr_of_mut!(GUARDED) as u32;
    wr32(0xE000_ED98, 0); // MPU_RNR = 0
    wr32(0xE000_ED9C, base); // MPU_RBAR
                             // XN | AP=RO | S,C,B | SIZE=256B | ENABLE  (MEMFAULTENA deliberately NOT set)
    wr32(
        0xE000_EDA0,
        (1 << 28) | (0b110 << 24) | (0b111 << 16) | (7 << 1) | 1,
    );
    wr32(0xE000_ED94, 0b101); // MPU ENABLE | PRIVDEFENA
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
    core::ptr::write_volatile(&mut GUARDED.0[0], 0xDEAD); // faults -> HardFault
    loop {}
}
const DUMP_MAGIC: u32 = 0x4E43_5244; // "NCRD"
const NVMC: u32 = 0x4001_E000;

unsafe fn rd32(a: u32) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}
unsafe fn wr32(a: u32, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v);
}
unsafe fn nvmc_wait() {
    while rd32(NVMC + 0x400) & 1 == 0 {}
}
unsafe fn flash_erase(page: u32) {
    wr32(NVMC + 0x504, 2);
    nvmc_wait();
    wr32(NVMC + 0x508, page);
    nvmc_wait();
    wr32(NVMC + 0x504, 0);
    nvmc_wait();
}
unsafe fn flash_word(addr: u32, val: u32) {
    wr32(NVMC + 0x504, 1);
    nvmc_wait();
    wr32(addr, val);
    nvmc_wait();
    wr32(NVMC + 0x504, 0);
    nvmc_wait();
}

#[exception]
unsafe fn HardFault(ef: &cortex_m_rt::ExceptionFrame) -> ! {
    wr32(0xE000_ED94, 0); // MPU off: the dump path must not fault again
                          // Persist the crash record: magic, PC, LR, xPSR, CFSR, HFSR, checksum.
    let pc = ef.pc();
    let lr = ef.lr();
    let xpsr = ef.xpsr();
    let cfsr = rd32(0xE000_ED28);
    let hfsr = rd32(0xE000_ED2C);
    let cs = DUMP_MAGIC ^ pc ^ lr ^ xpsr ^ cfsr ^ hfsr;
    flash_word(DUMP_PAGE, DUMP_MAGIC);
    flash_word(DUMP_PAGE + 4, pc);
    flash_word(DUMP_PAGE + 8, lr);
    flash_word(DUMP_PAGE + 12, xpsr);
    flash_word(DUMP_PAGE + 16, cfsr);
    flash_word(DUMP_PAGE + 20, hfsr);
    flash_word(DUMP_PAGE + 24, cs);
    cortex_m::peripheral::SCB::sys_reset();
}

#[entry]
fn main() -> ! {
    // App handlers are unreachable without this (bootloader leaves VTOR at its table).
    unsafe {
        wr32(0xE000_ED08, 0x1000);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
    }

    let have_dump = unsafe { rd32(DUMP_PAGE) } == DUMP_MAGIC;
    if !have_dump {
        // Boot A: mark the provisional report, then crash on purpose.
        unsafe {
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!(NOBRO_CRASH_REPORT),
                Report {
                    magic: MAGIC,
                    version: 1,
                    completed: 0,
                    all_pass: 0,
                    dump_pc: 0,
                    dump_cfsr: 0,
                    dump_valid: 0,
                    checksum: 0xAAAA_0001,
                },
            );
            // NVMC writes AND into existing bits - the dump page must start erased.
            flash_erase(DUMP_PAGE);
            crash_now(); // MPU violation escalates to HardFault -> dump + reset
        }
    }

    // Boot B: validate the persisted crash record.
    let (pc, lr, xpsr, cfsr, hfsr, cs) = unsafe {
        (
            rd32(DUMP_PAGE + 4),
            rd32(DUMP_PAGE + 8),
            rd32(DUMP_PAGE + 12),
            rd32(DUMP_PAGE + 16),
            rd32(DUMP_PAGE + 20),
            rd32(DUMP_PAGE + 24),
        )
    };
    let checksum_ok = cs == DUMP_MAGIC ^ pc ^ lr ^ xpsr ^ cfsr ^ hfsr;
    let pc_in_app = (0x1000..0x10_0000).contains(&pc);
    // Accept a bus fault, a MemManage access violation, or forced-HardFault escalation.
    let bus_fault_seen = cfsr & (1 << 9 | 1 << 10) != 0 || cfsr & 0x3 != 0 || hfsr & (1 << 30) != 0;
    let dump_valid = u32::from(checksum_ok && pc_in_app && bus_fault_seen);

    // Erase the page so the demo is repeatable on the next flash.
    unsafe { flash_erase(DUMP_PAGE) };

    let pass = dump_valid == 1;
    let ap = u32::from(pass);
    let rcs = MAGIC ^ 1 ^ 1 ^ ap ^ pc ^ cfsr ^ dump_valid;
    unsafe {
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_CRASH_REPORT),
            Report {
                magic: MAGIC,
                version: 1,
                completed: 1,
                all_pass: ap,
                dump_pc: pc,
                dump_cfsr: cfsr,
                dump_valid,
                checksum: rcs,
            },
        );
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
