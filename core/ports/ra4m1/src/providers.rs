//! Reusable RA4M1 providers used by the native Rust port.
//!
//! The clock uses the Cortex-M4 DWT counter after the board clock is configured at
//! 48 MHz. Callers must sample it at least once per 32-bit counter wrap (about 89 s).
//! The deadline provider owns SysTick as a hardware one-shot countdown. ADC, PWM,
//! I2C, and SPI on Arduino UNO R4 are exposed through `NobroArduinoProviders.h`,
//! which delegates to the installed Arduino Renesas core instead of duplicating FSP.

use core::{cell::UnsafeCell, convert::Infallible};

use cortex_m::peripheral::{DCB, DWT, SYST};
use nobro_hal::{
    HalAlarm, HalByteIo, HalClock, HalCompatibility, HardwareCapability, HardwareCapabilitySet,
};
use nobro_usb::{RaUsbfsCdc, Stage, UsbConfig, UsbStack};

const CORE_CLOCK_HZ: u64 = 48_000_000;
const CYCLES_PER_US: u64 = CORE_CLOCK_HZ / 1_000_000;
const SYST_MAX_RELOAD: u64 = 0x00ff_ffff;

pub struct Ra4m1Providers;

impl HalCompatibility for Ra4m1Providers {
    const CAPABILITIES: HardwareCapabilitySet = HardwareCapabilitySet::EMPTY
        .with(HardwareCapability::Timebase)
        .with(HardwareCapability::DeadlineTimer)
        .with(HardwareCapability::Usb);
}

#[derive(Clone, Copy)]
struct ClockState {
    last_cycles: u32,
    total_cycles: u64,
    initialized: bool,
}

struct ClockStorage(UnsafeCell<ClockState>);

// SAFETY: every access is serialized by `critical_section::with`.
unsafe impl Sync for ClockStorage {}

static CLOCK: ClockStorage = ClockStorage(UnsafeCell::new(ClockState {
    last_cycles: 0,
    total_cycles: 0,
    initialized: false,
}));

pub struct Ra4m1Clock;

impl Ra4m1Clock {
    /// Start the free-running DWT cycle counter after the RA4M1 clock reaches 48 MHz.
    pub fn init(dcb: &mut DCB, dwt: &mut DWT) {
        dcb.enable_trace();
        dwt.set_cycle_count(0);
        dwt.enable_cycle_counter();
        critical_section::with(|_| {
            // SAFETY: the critical-section token serializes the only mutable access.
            let state = unsafe { &mut *CLOCK.0.get() };
            *state = ClockState {
                last_cycles: DWT::cycle_count(),
                total_cycles: 0,
                initialized: true,
            };
        });
    }
}

impl HalClock for Ra4m1Clock {
    fn now_us() -> u64 {
        let current = DWT::cycle_count();
        critical_section::with(|_| {
            // SAFETY: the critical-section token serializes the only mutable access.
            let state = unsafe { &mut *CLOCK.0.get() };
            if !state.initialized {
                return 0;
            }
            state.total_cycles = state
                .total_cycles
                .saturating_add(u64::from(current.wrapping_sub(state.last_cycles)));
            state.last_cycles = current;
            state.total_cycles / CYCLES_PER_US
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlarmError {
    ZeroDelay,
    DelayTooLong,
}

pub struct Ra4m1Alarm {
    systick: SYST,
    deadline_us: Option<u64>,
}

impl Ra4m1Alarm {
    pub fn new(mut systick: SYST) -> Self {
        systick.disable_counter();
        systick.disable_interrupt();
        systick.set_clock_source(cortex_m::peripheral::syst::SystClkSource::Core);
        Self {
            systick,
            deadline_us: None,
        }
    }
}

impl HalAlarm for Ra4m1Alarm {
    type Error = AlarmError;

    fn arm_after_us(&mut self, delay_us: u64) -> Result<u64, Self::Error> {
        if delay_us == 0 {
            return Err(AlarmError::ZeroDelay);
        }
        let cycles = delay_us
            .checked_mul(CYCLES_PER_US)
            .ok_or(AlarmError::DelayTooLong)?;
        if cycles == 0 || cycles - 1 > SYST_MAX_RELOAD {
            return Err(AlarmError::DelayTooLong);
        }
        self.systick.disable_counter();
        self.systick.set_reload((cycles - 1) as u32);
        self.systick.clear_current();
        let deadline = Ra4m1Clock::now_us().saturating_add(delay_us);
        self.deadline_us = Some(deadline);
        self.systick.enable_counter();
        Ok(deadline)
    }

    fn cancel(&mut self) {
        self.systick.disable_counter();
        self.systick.clear_current();
        self.deadline_us = None;
    }

    fn deadline_us(&self) -> Option<u64> {
        self.deadline_us
    }

    fn poll_due(&mut self, now_us: u64) -> bool {
        if self.deadline_us.is_none() {
            return false;
        }
        if self.systick.has_wrapped() || self.deadline_us.is_some_and(|deadline| now_us >= deadline)
        {
            self.cancel();
            true
        } else {
            false
        }
    }
}

pub struct Ra4m1Usb(RaUsbfsCdc);

impl Ra4m1Usb {
    pub fn mount(config: &UsbConfig) -> Self {
        Self(RaUsbfsCdc::mount(config))
    }

    pub fn poll(&mut self) {
        let _ = self.0.poll();
    }

    pub fn configured(&self) -> bool {
        self.0.configured()
    }

    pub fn stage(&self) -> Stage {
        self.0.stage()
    }
}

impl HalByteIo for Ra4m1Usb {
    type Error = Infallible;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        Ok(self.0.read(bytes))
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        let _ = self.0.write(bytes);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        let _ = self.0.poll();
        Ok(())
    }
}
