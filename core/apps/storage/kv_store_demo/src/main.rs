//! Wear-leveled KV flash store on real hardware (M181): back nobro-storage's KvStore
//! with the nRF52840 NVMC over two dedicated 4 KB pages, drive enough puts to force
//! compactions (page switches + erases on both pages), then verify the latest values and
//! remount persistence. Self-certifies via NOBRO_KV_REPORT (J-Link mem32).
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_storage::{Flash, KvStore};

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    puts: u32,
    gets_ok: u32,
    active_page: u32,
    remount_ok: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E4B_5631; // "NKV1"

#[no_mangle]
#[used]
static mut NOBRO_KV_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    puts: 0,
    gets_ok: 0,
    active_page: 0,
    remount_ok: 0,
    checksum: 0,
};

const NVMC: u32 = 0x4001_E000;
const NVMC_READY: u32 = NVMC + 0x400;
const NVMC_CONFIG: u32 = NVMC + 0x504;
const NVMC_ERASEPAGE: u32 = NVMC + 0x508;
// Two dedicated pages, clear of the app image and the M50 log page (0x80000).
const PAGE_ADDR: [u32; 2] = [0x8_2000, 0x8_3000];

struct NvmcFlash;

unsafe fn nvmc_wait() {
    while core::ptr::read_volatile(NVMC_READY as *const u32) & 1 == 0 {}
}

impl Flash for NvmcFlash {
    type Error = ();
    const WORDS: usize = 1024; // 4 KB page

    fn erase(&mut self, page: usize) -> Result<(), Self::Error> {
        unsafe {
            core::ptr::write_volatile(NVMC_CONFIG as *mut u32, 2);
            nvmc_wait();
            core::ptr::write_volatile(NVMC_ERASEPAGE as *mut u32, PAGE_ADDR[page]);
            nvmc_wait();
            core::ptr::write_volatile(NVMC_CONFIG as *mut u32, 0);
            nvmc_wait();
        }
        (self.read_word(page, 0) == u32::MAX && self.read_word(page, Self::WORDS - 1) == u32::MAX)
            .then_some(())
            .ok_or(())
    }

    fn write_word(&mut self, page: usize, word: usize, val: u32) -> Result<(), Self::Error> {
        unsafe {
            core::ptr::write_volatile(NVMC_CONFIG as *mut u32, 1);
            nvmc_wait();
            core::ptr::write_volatile((PAGE_ADDR[page] + (word as u32) * 4) as *mut u32, val);
            nvmc_wait();
            core::ptr::write_volatile(NVMC_CONFIG as *mut u32, 0);
            nvmc_wait();
        }
        (self.read_word(page, word) == val).then_some(()).ok_or(())
    }

    fn read_word(&self, page: usize, word: usize) -> u32 {
        unsafe { core::ptr::read_volatile((PAGE_ADDR[page] + (word as u32) * 4) as *const u32) }
    }
}

#[entry]
fn main() -> ! {
    // Start from clean pages each run so the pass criteria are deterministic.
    let mut f = NvmcFlash;
    let _ = f.erase(0);
    let _ = f.erase(1);

    let mut kv = KvStore::mount(NvmcFlash).unwrap();

    // 1024 words = 3-word header + 340 committed records; this forces compactions.
    let mut puts: u32 = 0;
    for i in 0..1200u32 {
        if kv.put((i % 5) as u16, 10_000 + i).is_ok() {
            puts += 1;
        }
    }

    // latest value for key k is the last i with i % 5 == k:
    // 1199 % 5 == 4 -> key4=11199, key3=11198, key2=11197, key1=11196, key0=11195.
    let mut gets_ok: u32 = 0;
    for k in 0..5u16 {
        let expect = 10_000 + (1195 + u32::from(k));
        if kv.get(k) == Some(expect) {
            gets_ok += 1;
        }
    }
    let active_page = kv.active_page() as u32;

    // Remount (fresh mount over the same flash) and re-verify: persistence.
    drop(kv);
    let kv2 = KvStore::mount(NvmcFlash).unwrap();
    let mut remount_ok: u32 = 0;
    for k in 0..5u16 {
        let expect = 10_000 + (1195 + u32::from(k));
        if kv2.get(k) == Some(expect) {
            remount_ok += 1;
        }
    }

    let pass = puts == 1200 && gets_ok == 5 && remount_ok == 5;
    let ap = u32::from(pass);
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ puts ^ gets_ok ^ active_page ^ remount_ok;
    unsafe {
        NOBRO_KV_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            puts,
            gets_ok,
            active_page,
            remount_ok,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
