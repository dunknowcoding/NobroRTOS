//! nobro-database persistence on real flash.
//!
//! Boot sequence: read the table image from a dedicated NVMC flash page; a valid image
//! recovers the table (recovered=1), an invalid/blank page starts fresh. The app then
//! bumps the boot-counter row, appends one row for this boot, and writes the image
//! back. `NOBRO_DB_PERSIST_REPORT` carries the result across a reset:
//! `recovered` flips to 1 and `boot_count` climbs while the row set stays intact.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use nobro_database::{PersistentTable, RecordCodec, Table};
use nobro_storage::Flash;
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

impl RecordCodec for BootRecord {
    const SCHEMA_ID: u32 = 0x4E42_5253; // stable boot-record schema
    const ENCODED_LEN: usize = 8;

    fn encode(&self, out: &mut [u8]) -> bool {
        if out.len() != Self::ENCODED_LEN {
            return false;
        }
        out[..4].copy_from_slice(&self.value.to_le_bytes());
        out[4..8].copy_from_slice(&self.marker.to_le_bytes());
        true
    }

    fn decode(input: &[u8]) -> Option<Self> {
        if input.len() != Self::ENCODED_LEN {
            return None;
        }
        let record = Self {
            value: u32::from_le_bytes(input[..4].try_into().ok()?),
            marker: u32::from_le_bytes(input[4..8].try_into().ok()?),
        };
        (record.marker == MARKER).then_some(record)
    }
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
/// Dedicated alternating pages, clear of the app image and other persistence demos.
const PAGES: [u32; 2] = [0x8_4000, 0x8_5000];
const PAGE_WORDS: usize = 1024;

#[derive(Clone, Copy)]
enum FlashError {
    Verify,
}

struct NvmcDbFlash;

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

unsafe fn flash_write_word(addr: u32, value: u32) {
    core::ptr::write_volatile(NVMC_CONFIG as *mut u32, 1);
    nvmc_wait();
    core::ptr::write_volatile(addr as *mut u32, value);
    nvmc_wait();
    core::ptr::write_volatile(NVMC_CONFIG as *mut u32, 0);
    nvmc_wait();
}

impl Flash for NvmcDbFlash {
    type Error = FlashError;
    const WORDS: usize = PAGE_WORDS;

    fn erase(&mut self, page: usize) -> Result<(), Self::Error> {
        unsafe { flash_erase(PAGES[page]) };
        (0..PAGE_WORDS)
            .all(|word| self.read_word(page, word) == u32::MAX)
            .then_some(())
            .ok_or(FlashError::Verify)
    }

    fn write_word(&mut self, page: usize, word: usize, value: u32) -> Result<(), Self::Error> {
        if self.read_word(page, word) != u32::MAX {
            return Err(FlashError::Verify);
        }
        let address = PAGES[page] + (word as u32) * 4;
        unsafe { flash_write_word(address, value) };
        (self.read_word(page, word) == value)
            .then_some(())
            .ok_or(FlashError::Verify)
    }

    fn read_word(&self, page: usize, word: usize) -> u32 {
        unsafe { core::ptr::read_volatile((PAGES[page] + (word as u32) * 4) as *const u32) }
    }
}

#[entry]
fn main() -> ! {
    const CAP: usize = 8;
    // Image budget: header 20 + CAP rows * (4 key + 8 record) + checksum 4.
    const IMG_MAX: usize = 20 + CAP * 12 + 4;

    let mut image = [0u8; IMG_MAX];
    let mut persisted = PersistentTable::mount(NvmcDbFlash);
    let (mut table, recovered) = match persisted.load::<BootRecord, CAP>(&mut image) {
        Ok(table) => (table, 1u32),
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

    // Persist transactionally and remount immediately through the same recovery path.
    let image_len = table.to_image(&mut image).unwrap_or(0);
    ok &= image_len > 0;
    ok &= persisted.save(&table, &mut image).is_ok();
    let persisted = PersistentTable::mount(persisted.into_flash());
    let readback = persisted.load::<BootRecord, CAP>(&mut image);
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
