//! Sensor data logging to on-chip flash (M50) on a development board: erase a dedicated
//! flash page via NVMC, log a run of synthetic sensor samples, read them back, and
//! verify integrity (count + sum). Persists across reset. Self-certifies via
//! NOBRO_FLASH_LOG_REPORT (J-Link mem32).
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    page_addr: u32,
    samples_written: u32,
    samples_verified: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E46_4C47; // "NFLG"

#[no_mangle]
#[used]
static mut NOBRO_FLASH_LOG_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    page_addr: 0,
    samples_written: 0,
    samples_verified: 0,
    checksum: 0,
};

const NVMC: u32 = 0x4001_E000;
const NVMC_READY: u32 = NVMC + 0x400;
const NVMC_CONFIG: u32 = NVMC + 0x504;
const NVMC_ERASEPAGE: u32 = NVMC + 0x508;

unsafe fn nvmc_wait() {
    while core::ptr::read_volatile(NVMC_READY as *const u32) & 1 == 0 {}
}
unsafe fn flash_erase(page: u32) {
    core::ptr::write_volatile(NVMC_CONFIG as *mut u32, 2); // erase-enable
    nvmc_wait();
    core::ptr::write_volatile(NVMC_ERASEPAGE as *mut u32, page);
    nvmc_wait();
    core::ptr::write_volatile(NVMC_CONFIG as *mut u32, 0);
    nvmc_wait();
}
unsafe fn flash_write_word(addr: u32, val: u32) {
    core::ptr::write_volatile(NVMC_CONFIG as *mut u32, 1); // write-enable
    nvmc_wait();
    core::ptr::write_volatile(addr as *mut u32, val);
    nvmc_wait();
    core::ptr::write_volatile(NVMC_CONFIG as *mut u32, 0);
    nvmc_wait();
}
unsafe fn flash_read_word(addr: u32) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

// synthetic accel-magnitude-like sample for index i (the "sensor" we are logging)
fn sample_for(i: u32) -> u32 {
    1000u32.wrapping_add(i.wrapping_mul(37) % 256)
}

const N: u32 = 32;

#[entry]
fn main() -> ! {
    // A page well clear of the tiny app image (app starts at 0x1000).
    let page: u32 = 0x8_0000;

    let mut written = 0u32;
    let mut wsum = 0u32;
    unsafe {
        flash_erase(page);
        for i in 0..N {
            let s = sample_for(i);
            flash_write_word(page + i * 4, s);
            wsum = wsum.wrapping_add(s);
            written += 1;
        }
    }

    // read back + verify integrity
    let mut verified = 0u32;
    let mut rsum = 0u32;
    unsafe {
        for i in 0..N {
            let v = flash_read_word(page + i * 4);
            rsum = rsum.wrapping_add(v);
            if v == sample_for(i) {
                verified += 1;
            }
        }
    }

    let pass = written == N && verified == N && rsum == wsum;
    let ap = u32::from(pass);
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ page ^ written ^ verified;
    unsafe {
        NOBRO_FLASH_LOG_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            page_addr: page,
            samples_written: written,
            samples_verified: verified,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
