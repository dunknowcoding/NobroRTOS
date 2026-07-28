//! Honest System-ON idle + RTC wake: policy requests LowPower, the RTC2 provider
//! admits/enters Idle, and the transition report keeps both facts distinct.
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
use nobro_power::{ExecutorPower, PowerHookError, PowerMode, PowerPlatform, PowerVetoReason};

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    wakes: u32,
    mean_interval_us: u32,
    requested_low_power: u32,
    selected_idle: u32,
    effective_idle: u32,
    diagnostic_checksum: u32,
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
    requested_low_power: 0,
    selected_idle: 0,
    effective_idle: 0,
    diagnostic_checksum: 0,
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

struct Rtc2Idle;

impl PowerPlatform for Rtc2Idle {
    fn program_wake(&mut self, deadline_us: Option<u64>) -> Result<(), PowerHookError> {
        deadline_us
            .map(|_| ())
            .ok_or(PowerHookError { source: 2, code: 1 })
    }

    fn constrain_mode(&self, requested: PowerMode) -> PowerMode {
        requested.shallower(PowerMode::Idle)
    }

    fn enter(&mut self, mode: PowerMode) -> Result<PowerMode, PowerHookError> {
        if mode == PowerMode::Active {
            return Ok(PowerMode::Active);
        }
        unsafe {
            while rd(RTC2 + 0x140) == 0 {
                cortex_m::asm::wfe();
            }
            wr(RTC2 + 0x140, 0);
            wr(0xE000_E284, 1 << 4);
            wr(RTC2 + 0x008, 1);
        }
        Ok(PowerMode::Idle)
    }

    fn suspend(&mut self, _task_id: u16) -> Result<(), PowerHookError> {
        Ok(())
    }

    fn resume(&mut self, _task_id: u16) -> Result<(), PowerHookError> {
        Ok(())
    }
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

    let power = ExecutorPower::<1>::new(1_000_000, 100_000, 1_000);
    let mut platform = Rtc2Idle;
    let mut wakes: u32 = 0;
    let mut requested_low_power: u32 = 0;
    let mut selected_idle: u32 = 0;
    let mut effective_idle: u32 = 0;
    let t_start = Hal::now_us();

    while wakes < TARGET_WAKES {
        let now = Hal::now_us();
        let transition = power
            .apply_idle(
                now,
                false,
                Some(now.saturating_add(u64::from(PERIOD_US))),
                &mut platform,
            )
            .unwrap_or_else(|_| defmt::panic!("power transition"));
        if transition.requested == PowerMode::LowPower {
            requested_low_power += 1;
        }
        if transition.selected == PowerMode::Idle
            && transition.vetoes.contains(PowerVetoReason::PlatformLimited)
        {
            selected_idle += 1;
        }
        if transition.effective == PowerMode::Idle {
            effective_idle += 1;
        }
        wakes += 1;
    }

    let elapsed = Hal::now_us().wrapping_sub(t_start);
    let mean = (elapsed / u64::from(TARGET_WAKES)) as u32;
    let lo = PERIOD_US - PERIOD_US / 5;
    let hi = PERIOD_US + PERIOD_US / 5;
    let pass = lease_ok
        && wakes == TARGET_WAKES
        && mean >= lo
        && mean <= hi
        && requested_low_power == TARGET_WAKES
        && selected_idle == TARGET_WAKES
        && effective_idle == TARGET_WAKES;
    let ap = u32::from(pass);
    let cs =
        MAGIC ^ 2 ^ 1 ^ ap ^ wakes ^ mean ^ requested_low_power ^ selected_idle ^ effective_idle;
    unsafe {
        NOBRO_RTC_SLEEP_REPORT = Report {
            magic: MAGIC,
            version: 2,
            completed: 1,
            all_pass: ap,
            wakes,
            mean_interval_us: mean,
            requested_low_power,
            selected_idle,
            effective_idle,
            diagnostic_checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
