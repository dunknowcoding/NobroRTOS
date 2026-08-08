//! State-restoring physical qualification for the reserved SAMD21 flash row.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use nobro_port_samd21::providers::Samd21Flash;
use panic_halt as _;

#[allow(dead_code)]
mod masked_critical_section;

const MAGIC: u32 = 0x5344_3231;

#[repr(C)]
pub struct Report {
    magic: u32,
    completed: u32,
    exercised: u32,
    restored: u32,
    all_pass: u32,
}

#[no_mangle]
#[used]
pub static mut NOBRO_SAMD21_DEEP_REPORT: Report = Report {
    magic: 0,
    completed: 0,
    exercised: 0,
    restored: 0,
    all_pass: 0,
};

#[entry]
fn main() -> ! {
    masked_critical_section::init();
    let mut flash = Samd21Flash::try_new(0xd1).unwrap();
    let mut backup = [0u8; 256];
    let read_ok = flash.read(0, &mut backup).is_ok();
    let pattern = [0xa5u8; 64];
    let mut observed = [0u8; 256];
    let exercised = read_ok
        && flash.erase_row(0).is_ok()
        && (0..4).all(|page| flash.program_page(page * 64, &pattern).is_ok())
        && flash.read(0, &mut observed).is_ok()
        && observed.iter().all(|byte| *byte == 0xa5);

    let mut restored = flash.erase_row(0).is_ok();
    for (page, bytes) in backup.chunks_exact(64).enumerate() {
        let mut page_data = [0u8; 64];
        page_data.copy_from_slice(bytes);
        restored &= flash.program_page(page as u32 * 64, &page_data).is_ok();
    }
    let mut verify = [0u8; 256];
    restored &= flash.read(0, &mut verify).is_ok() && verify == backup;

    unsafe {
        NOBRO_SAMD21_DEEP_REPORT = Report {
            magic: MAGIC,
            completed: 1,
            exercised: u32::from(exercised),
            restored: u32::from(restored),
            all_pass: u32::from(exercised && restored),
        };
    }
    loop {
        cortex_m::asm::wfi();
    }
}
