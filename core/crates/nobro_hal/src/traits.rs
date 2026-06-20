//! Platform HAL capability traits used by apps and adapters.
//!
//! New MCU ports implement these for a `platform::<soc>::Platform` type and register it
//! as `[features] default = ["platform-nrf52840"]` in `airon-hal/Cargo.toml`.

use crate::board_desc::{BoardDesc, ServoProfile};
use crate::lease::{LeaseError, LeaseGuard, Resource};
use crate::snapshots::{BoardParity, EventCaptureSnapshot, PwmSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HardwareCapability {
    Timebase,
    ResourceLease,
    DeadlineTimer,
    EventCapture,
    ServoPwm,
    Bus,
    SelfTest,
}

impl HardwareCapability {
    pub const fn bit(self) -> u32 {
        match self {
            Self::Timebase => 1 << 0,
            Self::ResourceLease => 1 << 1,
            Self::DeadlineTimer => 1 << 2,
            Self::EventCapture => 1 << 3,
            Self::ServoPwm => 1 << 4,
            Self::Bus => 1 << 5,
            Self::SelfTest => 1 << 6,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HardwareCapabilitySet(pub u32);

impl HardwareCapabilitySet {
    pub const EMPTY: Self = Self(0);

    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn with(self, capability: HardwareCapability) -> Self {
        Self(self.0 | capability.bit())
    }

    pub const fn contains(self, capability: HardwareCapability) -> bool {
        self.0 & capability.bit() != 0
    }

    pub const fn contains_all(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    pub const fn missing(self, required: Self) -> Self {
        Self(required.0 & !self.0)
    }
}

/// Platform capability metadata for host-side and compile-time compatibility checks.
pub trait HalCompatibility {
    const CAPABILITIES: HardwareCapabilitySet;

    fn supports(required: HardwareCapabilitySet) -> bool {
        Self::CAPABILITIES.contains_all(required)
    }
}

/// Microsecond monotonic clock (system timebase).
pub trait HalClock {
    fn now_us() -> u64;
}

/// Hardware timestamp latch (nRF PPI, STM32 TRGO, RP2040 PIO, etc.).
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
    unsafe fn set_active_pulse_us(pulse_us: u32);
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

/// Exclusive peripheral lease with semantics shared across platforms.
pub trait HalLease {
    fn acquire(resource: Resource, owner: u8) -> Result<(), LeaseError>;
    fn release(resource: Resource, owner: u8) -> Result<(), LeaseError>;
    fn is_held(resource: Resource) -> bool;
    fn acquire_guard(resource: Resource, owner: u8) -> Result<LeaseGuard, LeaseError>;
}

/// Root marker for a platform backend (one impl per SoC family).
pub trait PlatformHal:
    HalCompatibility + HalClock + HalLease + HalDeadline + HalEventCapture + HalServoPwm
{
    const PLATFORM_ID: &'static str;
    type Board: BoardDesc;
    fn servo_profile() -> ServoProfile;
    unsafe fn init_timebase();
    /// One-shot bring-up: deadline timer, event capture, servo PWM for eval demos.
    unsafe fn init_scheduling_demo(profile: ServoProfile);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_sets_report_missing_bits() {
        let platform = HardwareCapabilitySet::EMPTY
            .with(HardwareCapability::Timebase)
            .with(HardwareCapability::ResourceLease);
        let required = HardwareCapabilitySet::EMPTY
            .with(HardwareCapability::Timebase)
            .with(HardwareCapability::Bus);

        assert!(platform.contains(HardwareCapability::Timebase));
        assert!(!platform.contains_all(required));
        assert_eq!(
            platform.missing(required),
            HardwareCapabilitySet::EMPTY.with(HardwareCapability::Bus)
        );
    }
}
