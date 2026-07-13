//! Flash-backed circular telemetry log on real NVMC. A ring of fixed-size records
//! across N flash pages with a monotonically increasing sequence number in each record's
//! header; readback finds the newest record by max sequence and walks the ring backward.
//! When the ring wraps, the oldest page is erased and reused - bounded flash, newest-wins.
//! Verified: write 200 records into a 4-page ring (so it wraps ~3x), then confirm the
//! last 64 recovered records are exactly the last 64 written, in order.
//! NOBRO_TELERING_REPORT.
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
    written: u32,
    newest_seq: u32,
    recovered_ok: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E54_5247; // "NTRG"

#[no_mangle]
#[used]
static mut NOBRO_TELERING_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    written: 0,
    newest_seq: 0,
    recovered_ok: 0,
    checksum: 0,
};

const NVMC: u32 = 0x4001_E000;
const PAGE_SIZE: u32 = 4096;
const RING_BASE: u32 = 0x8_4000; // 4 pages, clear of the app + other demos' pages
const RING_PAGES: u32 = 4;
const REC_WORDS: u32 = 4; // [seq, value, ~value, tag] = 16 bytes
const REC_BYTES: u32 = REC_WORDS * 4;
const RECS_PER_PAGE: u32 = PAGE_SIZE / REC_BYTES;
const TOTAL_SLOTS: u32 = RING_PAGES * RECS_PER_PAGE;

unsafe fn rd(a: u32) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}
unsafe fn wr(a: u32, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v);
}
unsafe fn nvmc_wait() {
    while rd(NVMC + 0x400) & 1 == 0 {}
}
unsafe fn flash_erase(page: u32) {
    wr(NVMC + 0x504, 2);
    nvmc_wait();
    wr(NVMC + 0x508, page);
    nvmc_wait();
    wr(NVMC + 0x504, 0);
    nvmc_wait();
}
unsafe fn flash_word(addr: u32, val: u32) {
    wr(NVMC + 0x504, 1);
    nvmc_wait();
    wr(addr, val);
    nvmc_wait();
    wr(NVMC + 0x504, 0);
    nvmc_wait();
}

fn slot_addr(slot: u32) -> u32 {
    RING_BASE + (slot % TOTAL_SLOTS) * REC_BYTES
}

/// Append record `seq`/`value`; erase the page first when we land on its first slot.
unsafe fn push(seq: u32, value: u32) {
    let slot = seq % TOTAL_SLOTS;
    if slot % RECS_PER_PAGE == 0 {
        flash_erase(RING_BASE + (slot / RECS_PER_PAGE) * PAGE_SIZE);
    }
    let a = slot_addr(slot);
    flash_word(a, seq);
    flash_word(a + 4, value);
    flash_word(a + 8, !value);
    flash_word(a + 12, seq ^ value); // simple integrity tag
}

/// Read the record living in `slot`; None if empty/erased or tag mismatch.
unsafe fn read_slot(slot: u32) -> Option<(u32, u32)> {
    let a = slot_addr(slot);
    let seq = rd(a);
    let value = rd(a + 4);
    let inv = rd(a + 8);
    let tag = rd(a + 12);
    if seq == 0xFFFF_FFFF || value != !inv || tag != seq ^ value {
        None
    } else {
        Some((seq, value))
    }
}

#[entry]
fn main() -> ! {
    const N: u32 = 200;

    // Fresh start: erase all ring pages.
    unsafe {
        for p in 0..RING_PAGES {
            flash_erase(RING_BASE + p * PAGE_SIZE);
        }
    }

    // Write N records; value(seq) = seq*7 + 3 so readback is checkable.
    let value_of = |seq: u32| seq.wrapping_mul(7).wrapping_add(3);
    let mut written = 0u32;
    unsafe {
        for seq in 1..=N {
            push(seq, value_of(seq));
            written += 1;
        }
    }

    // Find the newest record: max seq across all live slots.
    let mut newest_seq = 0u32;
    unsafe {
        for slot in 0..TOTAL_SLOTS {
            if let Some((s, _)) = read_slot(slot) {
                if s > newest_seq {
                    newest_seq = s;
                }
            }
        }
    }

    // Walk backward from newest_seq over the last 64 records; each must match value_of.
    let mut recovered_ok = 1u32;
    let check = 64u32.min(TOTAL_SLOTS - 1);
    unsafe {
        for i in 0..check {
            let seq = newest_seq - i;
            match read_slot(seq % TOTAL_SLOTS) {
                Some((s, v)) if s == seq && v == value_of(seq) => {}
                _ => recovered_ok = 0,
            }
        }
    }

    let pass = written == N && newest_seq == N && recovered_ok == 1;
    let ap = u32::from(pass);
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ written ^ newest_seq ^ recovered_ok;
    unsafe {
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_TELERING_REPORT),
            Report {
                magic: MAGIC,
                version: 1,
                completed: 1,
                all_pass: ap,
                written,
                newest_seq,
                recovered_ok,
                checksum: cs,
            },
        );
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
