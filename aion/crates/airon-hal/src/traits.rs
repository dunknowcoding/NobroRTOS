//! Platform HAL capability traits — apps/adapters depend on these, not on register blocks.
//!
//! New MCU ports implement these for a `platform::<soc>::Platform` type and register it
//! as `[features] default = ["platform-nrf52840"]` in `airon-hal/Cargo.toml`.

use crate::board_desc::{BoardDesc, ServoProfile};
use crate::lease::{LeaseError, Resource};
use crate::snapshots::{BoardParity, EventCaptureSnapshot, PwmSnapshot};

/// Microsecond monotonic clock (system timebase).
pub trait HalClock {
    fn now_us() -> u64;
}

/// Hardware-timestamp latch (nRF PPI, STM32 TRGO, RP2040 PIO, …).
pub trait HalEventCapture {
    unsafe fn init();
    unsafe fn trigger_and_latency_us() -> Option<u32>;
    fn latency_stats() -> (u32, u32);
    unsafe fn capture_snapshot(channel: usize) -> EventCaptureSnapshot;
}

/// 50 Hz deadline / servo slot timer.
pub trait HalDeadline {
    unsafe fn init();
    fn enable_interrupt();
    fn on_interrupt();
    /// Polled compare path (used when NVIC path is disabled).
    fn poll_compare(on_tick: impl FnOnce(u64));
}

/// Servo-style PWM backend.
pub trait HalServoPwm {
    unsafe fn init_50hz(pin: u8, pulse_us: u32);
    fn read_pulse_us() -> u32;
}

/// I2C/SPI bus stub or real backend with lease integration.
pub trait HalBus {
    type Error;
    fn acquire_twim0(owner: u8) -> Result<Self, LeaseError>
    where
        Self: Sized;
    fn read_stub(&self, addr: u8, buf: &mut [u8]) -> Result<(), Self::Error>;
}

/// Register readback self-test (replaces scope for CI / autonomous eval).
pub trait HalSelfTest<B: BoardDesc> {
    unsafe fn scene_d_pass(profile: ServoProfile) -> (bool, PwmSnapshot, BoardParity);
}

/// Exclusive peripheral lease — semantics shared across platforms.
pub trait HalLease {
    fn acquire(resource: Resource, owner: u8) -> Result<(), LeaseError>;
    fn release(resource: Resource, owner: u8) -> Result<(), LeaseError>;
    fn is_held(resource: Resource) -> bool;
}

/// Root marker for a platform backend (one impl per SoC family).
pub trait PlatformHal:
    HalClock + HalLease + HalDeadline + HalEventCapture + HalServoPwm
{
    const PLATFORM_ID: &'static str;
    type Board: BoardDesc;
    fn servo_profile() -> ServoProfile;
    unsafe fn init_timebase();
    /// One-shot bring-up: deadline timer, event capture, servo PWM for eval demos.
    unsafe fn init_scheduling_demo(profile: ServoProfile);
}
