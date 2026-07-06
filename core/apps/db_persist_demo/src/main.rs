//! nobro-database persistence on real flash (M217).
//!
//! Boot sequence: read the table image from a dedicated NVMC flash page; a valid image
//! recovers the table (recovered=1), an invalid/blank page starts fresh. The app then
//! bumps the boot-counter row, appends one row for this boot, and writes the image
//! back. `NOBRO_DB_PERSIST_REPORT` (J-Link mem32) carries the proof: across a reset,
//! `recovered` flips to 1 and `boot_count` climbs while the row set stays intact.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use nobro_database::Table;
use panic_halt as _;

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    recovered: u32,
    boot_count: u32,
    rows: u32,
    image_len: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E44_4250; // "NDBP"

#[no_mangle]
#[used]
static mut NOBRO_DB_PERSIST_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    recovered: 0,
    boot_count: 0,
    rows: 0,
    image_len: 0,
    checksum: 0,
};

/// One row of persisted state: a value plus a fixed marker that must survive intact.
#[derive(Clone, Copy, Default)]
struct BootRecord {
    value: u32,
    marker: u32,
}
const MARKER: u32 = 0x4E42_5253; // "NBRS"
/// Key of the boot-counter row; per-boot rows use BOOT_ROW_BASE + count.
const COUNTER_KEY: u32 = 1;
const BOOT_ROW_BASE: u32 = 100;

// ---------------------------------------------------------------- NVMC flash driver

const NVMC: u32 = 0x4001_E000;
const NVMC_READY: u32 = NVMC + 0x400;
const NVMC_CONFIG: u32 = NVMC + 0x504;
const NVMC_ERASEPAGE: u32 = NVMC + 0x508;
/// Dedicated page, clear of the app image and of flash_log_demo's page (0x80000).
const PAGE: u32 = 0x8_4000;

unsafe fn nvmc_wait() {
    while core::ptr::read_volatile(NVMC_READY as *const u32) & 1 == 0 {}
}

unsafe fn flash_erase(page: u32) {
    core::ptr::write_volatile(NVMC_CONFIG as *mut u32, 2);
    nvmc_wait();
    core::ptr::write_volatile(NVMC_ERASEPAGE as *mut u32, page);
    nvmc_wait();
    core::ptr::write_volatile(NVMC_CONFIG as *mut u32, 0);
    nvmc_wait();
}

unsafe fn flash_write_words(addr: u32, data: &[u8]) {
    core::ptr::write_volatile(NVMC_CONFIG as *mut u32, 1);
    nvmc_wait();
    for (i, chunk) in data.chunks(4).enumerate() {
        let mut word = [0xFFu8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        core::ptr::write_volatile(
            (addr + (i as u32) * 4) as *mut u32,
            u32::from_le_bytes(word),
        );
        nvmc_wait();
    }
    core::ptr::write_volatile(NVMC_CONFIG as *mut u32, 0);
    nvmc_wait();
}

fn flash_slice(addr: u32, len: usize) -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(addr as *const u8, len) }
}

#[entry]
fn main() -> ! {
    const CAP: usize = 8;
    // Image budget: header 8 + CAP rows * (4 key + 8 record) + crc 4.
    const IMG_MAX: usize = 8 + CAP * 12 + 4;

    // Recover the table from flash (the image is length-prefixed by row count, so try
    // the maximal window; from_image validates magic + checksum).
    let (mut table, recovered) =
        match Table::<BootRecord, CAP>::from_image(flash_slice(PAGE, IMG_MAX)) {
            Ok(t) => (t, 1u32),
            Err(_) => (Table::new(), 0u32),
        };

    // Rows recovered from the previous boot must carry the marker intact.
    let mut ok = table.iter().all(|(_, r)| r.marker == MARKER);

    // Bump the boot counter and append this boot's row (older per-boot rows rotate out
    // once the fixed capacity would overflow - deleting the oldest is a query).
    let boot_count = table.get(COUNTER_KEY).map(|r| r.value).unwrap_or(0) + 1;
    ok &= table
        .upsert(
            COUNTER_KEY,
            BootRecord {
                value: boot_count,
                marker: MARKER,
            },
        )
        .is_ok();
    if table.len() == table.capacity() {
        if let Some(oldest) = table.next_key(BOOT_ROW_BASE) {
            let _ = table.delete(oldest);
        }
    }
    ok &= table
        .insert(
            BOOT_ROW_BASE + boot_count,
            BootRecord {
                value: boot_count,
                marker: MARKER,
            },
        )
        .is_ok();

    // Persist the image and verify it reads back as the same table.
    let mut img = [0u8; IMG_MAX];
    let image_len = table.to_image(&mut img).unwrap_or(0);
    ok &= image_len > 0;
    unsafe {
        flash_erase(PAGE);
        flash_write_words(PAGE, &img[..image_len]);
    }
    let readback = Table::<BootRecord, CAP>::from_image(flash_slice(PAGE, image_len));
    ok &= matches!(&readback, Ok(t) if t.len() == table.len()
        && t.get(COUNTER_KEY).map(|r| r.value) == Some(boot_count));

    let ap = u32::from(ok);
    let rows = table.len() as u32;
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ recovered ^ boot_count ^ rows ^ image_len as u32;
    unsafe {
        NOBRO_DB_PERSIST_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            recovered,
            boot_count,
            rows,
            image_len: image_len as u32,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
