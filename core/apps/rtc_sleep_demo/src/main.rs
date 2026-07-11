//! Low-power idle + RTC wake (M158): RTC2 fires every ~50 ms off the LFCLK while the CPU
//! WFE-idles; the nobro-power policy selects the mode each cycle. Verifies 40 wakes and
//! that the measured mean interval matches the programmed period within 20%.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_hal::{
    lease::Resource,
    traits::{HalClock, HalLease, HalTimebaseProvider},
    ActivePlatform as Hal,
};
use nobro_power::{PowerManager, PowerMode};

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    wakes: u32,
    mean_interval_us: u32,
    mode_idle_picks: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E52_5443; // "NRTC"

#[no_mangle]
#[used]
static mut NOBRO_RTC_SLEEP_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    wakes: 0,
    mean_interval_us: 0,
    mode_idle_picks: 0,
    checksum: 0,
};

const CLOCK: u32 = 0x4000_0000;
const RTC2: u32 = 0x4002_4000;
const TARGET_WAKES: u32 = 40;
const PERIOD_TICKS: u32 = 1638; // ~50 ms at 32768 Hz
const PERIOD_US: u32 = PERIOD_TICKS * 1_000_000 / 32_768;

unsafe fn rd(a: u32) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}
unsafe fn wr(a: u32, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v);
}

#[entry]
fn main() -> ! {
    Hal::acquire(Resource::Timer0, 2).unwrap_or_else(|_| defmt::panic!("timer lease"));
    unsafe {
        Hal::init_timebase();
    }
    // RTC2 is a managed resource: take its lease before touching the peripheral.
    let lease_ok = Hal::acquire(Resource::Rtc2, 9).is_ok();

    unsafe {
        // LFCLK from the internal RC (no crystal needed), then RTC2 CC[0] wake.
        wr(CLOCK + 0x518, 0); // LFCLKSRC = RC
        wr(CLOCK + 0x008, 1); // TASKS_LFCLKSTART
        while rd(CLOCK + 0x104) == 0 {} // EVENTS_LFCLKSTARTED
        wr(RTC2 + 0x004, 1); // TASKS_STOP
        wr(RTC2 + 0x008, 1); // TASKS_CLEAR
        wr(RTC2 + 0x508, 0); // PRESCALER = 0 -> 32768 Hz
        wr(RTC2 + 0x540, PERIOD_TICKS); // CC[0]
        wr(RTC2 + 0x304, 1 << 16); // INTENSET: COMPARE0 (event -> SEV via SEVONPEND)
        wr(RTC2 + 0x000, 1); // TASKS_START
    }
    // Wake WFE on the pended (masked) interrupt without an ISR: SCR.SEVONPEND (bit 4)
    // makes a pending-but-disabled IRQ emit an event; SLEEPDEEP stays clear (System ON).
    unsafe {
        let scr = 0xE000_ED10 as *mut u32;
        core::ptr::write_volatile(scr, (core::ptr::read_volatile(scr) | (1 << 4)) & !(1 << 2));
    }

    let mut pm = PowerManager::new(1_000_000, 100_000);
    let mut wakes: u32 = 0;
    let mut idle_picks: u32 = 0;
    let t_start = Hal::now_us();

    while wakes < TARGET_WAKES {
        // policy: no work pending, next deadline one 50 ms RTC period away -> the
        // policy picks LowPower (Idle is reserved for deadlines under 2 ms).
        if pm.select(false, Some(u64::from(PERIOD_US))) == PowerMode::LowPower {
            idle_picks += 1;
        }
        unsafe {
            while rd(RTC2 + 0x140) == 0 {
                cortex_m::asm::wfe(); // sleep until the RTC compare pends
            }
            wr(RTC2 + 0x140, 0); // clear EVENTS_COMPARE[0]
            wr(0xE000_E284, 1 << 4); // NVIC ICPR[1]: unpend RTC2 (IRQ 36)
            wr(RTC2 + 0x008, 1); // TASKS_CLEAR: restart the period
        }
        wakes += 1;
        pm.end_window();
    }

    let elapsed = Hal::now_us().wrapping_sub(t_start);
    let mean = (elapsed / u64::from(TARGET_WAKES)) as u32;
    let lo = PERIOD_US - PERIOD_US / 5;
    let hi = PERIOD_US + PERIOD_US / 5;
    let pass =
        lease_ok && wakes == TARGET_WAKES && mean >= lo && mean <= hi && idle_picks == TARGET_WAKES;
    let ap = u32::from(pass);
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ wakes ^ mean ^ idle_picks;
    unsafe {
        NOBRO_RTC_SLEEP_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            wakes,
            mean_interval_us: mean,
            mode_idle_picks: idle_picks,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
