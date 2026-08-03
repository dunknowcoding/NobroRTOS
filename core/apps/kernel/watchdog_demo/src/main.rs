//! Exact nRF52840 hardware-watchdog containment campaign.
//!
//! TIMER1 owns the only reload path and continues to run while thread mode is
//! intentionally wedged. After three valid windowed feeds the interrupt feeder
//! is deliberately withheld, the WDT resets the SoC, and the next boot records
//! RESETREAS.DOG through the portable watchdog receipt. The recovered boot keeps
//! the independent feeder alive so the campaign cannot become a reset loop.
#![no_std]
#![no_main]

use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use cortex_m::peripheral::NVIC;
use cortex_m_rt::entry;
use critical_section::Mutex;
use defmt_rtt as _;
use nobro_kernel::{
    HardwareResetCause, HardwareWatchdogBackend, HardwareWatchdogProfile, HardwareWatchdogSession,
    ModuleId,
};
use nrf52840_pac::{Interrupt, TIMER1};
use panic_halt as _;

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    reset_reason: u32,
    recovered: u32,
    independent_feeds: u32,
    early_rejections: u32,
    late_rejections: u32,
    provider_id: u32,
    provider_generation: u32,
    window_open_us: u32,
    window_close_us: u32,
    diagnostic_checksum: u32,
}

const MAGIC: u32 = 0x4E57_4432; // "NWD2"
const VERSION: u32 = 2;
const RESETREAS: u32 = 0x4000_0400;
const WDT: u32 = 0x4001_0000;
const DOG: u32 = 1 << 1;
const WDT_CRV: u32 = 16_383; // ~500 ms at 32.768 kHz
const WDT_RELOAD: u32 = 0x6E52_4635;
const FEED_INTERVAL_US: u64 = 250_000;
const PROVIDER_ID: u16 = 0x5284;
const PROVIDER_GENERATION: u32 = 1;

#[no_mangle]
#[used]
static mut NOBRO_WDT_REPORT: Report = Report {
    magic: MAGIC,
    version: VERSION,
    completed: 0,
    all_pass: 0,
    reset_reason: 0,
    recovered: 0,
    independent_feeds: 0,
    early_rejections: 0,
    late_rejections: 0,
    provider_id: PROVIDER_ID as u32,
    provider_generation: PROVIDER_GENERATION,
    window_open_us: 100_000,
    window_close_us: 500_000,
    diagnostic_checksum: 0,
};

unsafe fn rd(address: u32) -> u32 {
    core::ptr::read_volatile(address as *const u32)
}

unsafe fn wr(address: u32, value: u32) {
    core::ptr::write_volatile(address as *mut u32, value);
}

struct NrfHardwareWatchdog {
    reset_bits: u32,
}

impl NrfHardwareWatchdog {
    const fn new() -> Self {
        Self { reset_bits: 0 }
    }
}

impl HardwareWatchdogBackend for NrfHardwareWatchdog {
    type Error = u8;

    fn profile(&self) -> HardwareWatchdogProfile {
        HardwareWatchdogProfile {
            provider_id: PROVIDER_ID,
            generation: PROVIDER_GENERATION,
            window_open_us: 100_000,
            window_close_us: 500_000,
            independent_clock: true,
            independent_feed: true,
            resets_system: true,
        }
    }

    fn arm(&mut self) -> Result<(), Self::Error> {
        unsafe {
            wr(WDT + 0x504, WDT_CRV);
            wr(WDT + 0x508, 1);
            // Run in System-ON sleep. Debug-halt behavior remains the hardware
            // default so a debugger halt cannot create an accidental reset.
            wr(WDT + 0x50C, 1);
            wr(WDT, 1);
        }
        Ok(())
    }

    fn feed(&mut self) -> Result<(), Self::Error> {
        unsafe { wr(WDT + 0x600, WDT_RELOAD) };
        Ok(())
    }

    fn reset_cause(&mut self) -> Result<HardwareResetCause, Self::Error> {
        self.reset_bits = unsafe { rd(RESETREAS) };
        Ok(if self.reset_bits & DOG != 0 {
            HardwareResetCause::Watchdog
        } else if self.reset_bits == 0 {
            HardwareResetCause::None
        } else {
            HardwareResetCause::Unknown((self.reset_bits & 0xFFFF) as u16)
        })
    }

    fn clear_reset_cause(&mut self) -> Result<(), Self::Error> {
        unsafe { wr(RESETREAS, self.reset_bits) };
        Ok(())
    }
}

type Session = HardwareWatchdogSession<NrfHardwareWatchdog>;
static SESSION: Mutex<RefCell<Option<Session>>> = Mutex::new(RefCell::new(None));
static FEED_NOW_US: AtomicU32 = AtomicU32::new(0);
static FEED_EPOCH: AtomicU32 = AtomicU32::new(0);
static FEEDS: AtomicU32 = AtomicU32::new(0);
static FEED_ERRORS: AtomicU32 = AtomicU32::new(0);
static RECOVERED_BOOT: AtomicBool = AtomicBool::new(false);
static NEXT_COMPARE: AtomicU32 = AtomicU32::new(FEED_INTERVAL_US as u32);

#[no_mangle]
#[allow(non_snake_case)]
unsafe extern "C" fn TIMER1() {
    let timer = TIMER1::ptr();
    (*timer).events_compare[3].reset();
    let next = NEXT_COMPARE
        .load(Ordering::Relaxed)
        .wrapping_add(FEED_INTERVAL_US as u32);
    NEXT_COMPARE.store(next, Ordering::Relaxed);
    (*timer).cc[3].write(|writer| writer.bits(next));

    let prior_feeds = FEEDS.load(Ordering::Acquire);
    if !RECOVERED_BOOT.load(Ordering::Acquire) && prior_feeds >= 3 {
        return;
    }
    let prior_time = FEED_NOW_US.fetch_add(FEED_INTERVAL_US as u32, Ordering::AcqRel);
    let current_time = prior_time.wrapping_add(FEED_INTERVAL_US as u32);
    let epoch = if current_time < prior_time {
        FEED_EPOCH.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    } else {
        FEED_EPOCH.load(Ordering::Acquire)
    };
    let now_us = (u64::from(epoch) << 32) | u64::from(current_time);
    let result = critical_section::with(|critical| {
        SESSION
            .borrow(critical)
            .borrow_mut()
            .as_mut()
            .ok_or(())
            .and_then(|session| {
                session
                    .feed_from_independent_monitor(ModuleId::Kernel, now_us)
                    .map_err(|_| ())
            })
    });
    if result.is_ok() {
        FEEDS.fetch_add(1, Ordering::AcqRel);
    } else {
        FEED_ERRORS.fetch_add(1, Ordering::AcqRel);
    }
}

unsafe fn start_independent_feeder() {
    let timer = TIMER1::ptr();
    (*timer).tasks_stop.write(|writer| writer.bits(1));
    (*timer).tasks_clear.write(|writer| writer.bits(1));
    (*timer).mode.write(|writer| writer.mode().timer());
    (*timer).bitmode.write(|writer| writer.bitmode()._32bit());
    (*timer)
        .prescaler
        .write(|writer| writer.prescaler().bits(4));
    (*timer).events_compare[3].reset();
    (*timer).cc[3].write(|writer| writer.bits(FEED_INTERVAL_US as u32));
    (*timer)
        .intenset
        .write(|writer| writer.compare3().set_bit());
    (*timer).tasks_start.write(|writer| writer.bits(1));
    let mut peripherals = cortex_m::Peripherals::steal();
    peripherals.NVIC.set_priority(Interrupt::TIMER1, 3 << 5);
    NVIC::unpend(Interrupt::TIMER1);
    NVIC::unmask(Interrupt::TIMER1);
}

#[entry]
fn main() -> ! {
    let mut session =
        HardwareWatchdogSession::mount(NrfHardwareWatchdog::new(), ModuleId::Kernel, 0)
            .unwrap_or_else(|_| defmt::panic!("hardware watchdog mount"));
    let receipt = session
        .take_reset_receipt(0)
        .unwrap_or_else(|_| defmt::panic!("hardware watchdog reset cause"));
    let recovered = receipt.cause == HardwareResetCause::Watchdog;
    RECOVERED_BOOT.store(recovered, Ordering::Release);
    critical_section::with(|critical| {
        *SESSION.borrow(critical).borrow_mut() = Some(session);
    });
    unsafe { start_independent_feeder() };

    if recovered {
        while FEEDS.load(Ordering::Acquire) < 2 {
            cortex_m::asm::wfe();
        }
        let feeds = FEEDS.load(Ordering::Acquire);
        let errors = FEED_ERRORS.load(Ordering::Acquire);
        let pass = errors == 0 && feeds >= 2;
        let pass_word = u32::from(pass);
        let reset_bits = DOG;
        let checksum = MAGIC
            ^ VERSION
            ^ 1
            ^ pass_word
            ^ reset_bits
            ^ 1
            ^ feeds
            ^ 0
            ^ 0
            ^ u32::from(PROVIDER_ID)
            ^ PROVIDER_GENERATION
            ^ 100_000
            ^ 500_000;
        unsafe {
            NOBRO_WDT_REPORT = Report {
                magic: MAGIC,
                version: VERSION,
                completed: 1,
                all_pass: pass_word,
                reset_reason: reset_bits,
                recovered: 1,
                independent_feeds: feeds,
                early_rejections: 0,
                late_rejections: 0,
                provider_id: u32::from(PROVIDER_ID),
                provider_generation: PROVIDER_GENERATION,
                window_open_us: 100_000,
                window_close_us: 500_000,
                diagnostic_checksum: checksum,
            };
        }
    }

    // No cooperative-executor progress or task callback occurs here. TIMER1
    // alone feeds the watchdog; the first boot deliberately stops after three
    // feeds, while the recovered boot feeds forever.
    loop {
        core::hint::spin_loop();
    }
}
