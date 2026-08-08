//! Sensor data logging to on-chip flash on a development board: erase a dedicated
//! flash page via NVMC, log a run of synthetic sensor samples, read them back, and
//! verify integrity (count + sum). Persists across reset. Publishes
//! NOBRO_FLASH_LOG_REPORT.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use nobro_hal::{NrfNvmc, APP_STORAGE_START};
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
    diagnostic_checksum: u32,
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
    diagnostic_checksum: 0,
};

// synthetic accel-magnitude-like sample for index i (the "sensor" we are logging)
fn sample_for(i: u32) -> u32 {
    1000u32.wrapping_add(i.wrapping_mul(37) % 256)
}

const N: u32 = 32;

#[entry]
fn main() -> ! {
    // Page zero is the first linker-reserved application-storage page. The
    // provider refuses every address outside that dedicated four-page region.
    let page = APP_STORAGE_START;
    let mut flash = NrfNvmc::try_acquire(1).unwrap_or_else(|_| defmt::panic!("NVMC lease"));

    let mut written = 0u32;
    let mut wsum = 0u32;
    flash
        .erase_page(0)
        .unwrap_or_else(|_| defmt::panic!("NVMC erase"));
    for i in 0..N {
        let s = sample_for(i);
        flash
            .write_word(0, i, s)
            .unwrap_or_else(|_| defmt::panic!("NVMC write"));
        wsum = wsum.wrapping_add(s);
        written += 1;
    }

    // read back + verify integrity
    let mut verified = 0u32;
    let mut rsum = 0u32;
    for i in 0..N {
        let v = flash
            .read_word(0, i)
            .unwrap_or_else(|_| defmt::panic!("NVMC read"));
        rsum = rsum.wrapping_add(v);
        if v == sample_for(i) {
            verified += 1;
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
            diagnostic_checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
