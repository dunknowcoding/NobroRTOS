//! Platform HAL capability traits used by apps and adapters.
//!
//! New MCU ports implement these for a `platform::<soc>::Platform` type and register it
//! as `[features] default = ["platform-nrf52840"]` in `nobro-hal/Cargo.toml`.

use crate::board_desc::{BoardDesc, ServoProfile};
use crate::lease::LeaseError;
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
    I2c,
    Spi,
    Usb,
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
            Self::I2c => 1 << 7,
            Self::Spi => 1 << 8,
            Self::Usb => 1 << 9,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferMode {
    Polling,
    Dma,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseClass {
    Timer,
    I2c,
    Spi,
    Radio,
    Pwm,
    EventRouter,
    SoftwareEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseId {
    pub class: LeaseClass,
    pub instance: u8,
}

impl LeaseId {
    pub const fn new(class: LeaseClass, instance: u8) -> Self {
        Self { class, instance }
    }

    pub const SYSTEM_TIMER: Self = Self::new(LeaseClass::Timer, 0);
    pub const LOW_POWER_TIMER: Self = Self::new(LeaseClass::Timer, 2);
    pub const DEADLINE_TIMER: Self = Self::new(LeaseClass::Timer, 1);
    pub const PRIMARY_I2C: Self = Self::new(LeaseClass::I2c, 0);
    pub const SECONDARY_I2C: Self = Self::new(LeaseClass::I2c, 1);
    pub const PRIMARY_SPI: Self = Self::new(LeaseClass::Spi, 0);
    pub const PRIMARY_RADIO: Self = Self::new(LeaseClass::Radio, 0);
    pub const PRIMARY_PWM: Self = Self::new(LeaseClass::Pwm, 0);
    pub const EVENT_ROUTER: Self = Self::new(LeaseClass::EventRouter, 0);
    pub const SOFTWARE_EVENT: Self = Self::new(LeaseClass::SoftwareEvent, 0);
}

/// Hardware timestamp latch (nRF PPI, STM32 TRGO, RP2040 PIO, etc.).
pub trait HalEventCapture {
    /// # Safety
    /// Caller must own the capture peripheral's lease and call this once before any
    /// other method; it writes the platform's event-routing registers.
    unsafe fn init();
    /// # Safety
    /// Requires a prior successful [`HalEventCapture::init`]; fires a hardware event
    /// and reads the latched timestamp registers.
    unsafe fn trigger_and_latency_us() -> Option<u32>;
    fn latency_stats() -> (u32, u32);
    /// # Safety
    /// Requires a prior successful [`HalEventCapture::init`]; `channel` must be a
    /// channel this platform routed during init (out-of-range reads undefined data).
    unsafe fn capture_snapshot(channel: usize) -> EventCaptureSnapshot;
}

/// 50 Hz deadline / servo slot timer.
pub trait HalDeadline {
    /// # Safety
    /// Caller must own the deadline timer's lease and call this once; it configures
    /// the timer peripheral's mode, prescaler, and compare registers.
    unsafe fn init();
    /// # Safety
    /// The deadline lease must be live and initialization complete.
    unsafe fn enable_interrupt();
    /// # Safety
    /// Call only from the configured deadline interrupt while its lease is live.
    unsafe fn on_interrupt();
    /// Polled compare path (used when NVIC path is disabled).
    /// # Safety
    /// The deadline timer must be initialized and protected by a live lease session.
    unsafe fn poll_compare(on_tick: impl FnOnce(u64));
}

/// Servo-style PWM backend.
pub trait HalServoPwm {
    /// # Safety
    /// Caller must own the PWM lease; `pin` must be the board's wired servo pin
    /// (driving an arbitrary pin can conflict with other peripherals' pin muxing).
    unsafe fn init_50hz(pin: u8, pulse_us: u32);
    /// # Safety
    /// Requires a prior [`HalServoPwm::init_50hz`]; writes the live PWM compare
    /// buffer the peripheral is DMA-reading.
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

/// Portable I2C transaction provider. Backends state whether execution is polled or DMA.
pub trait HalI2c {
    type Error;
    const TRANSFER_MODE: TransferMode;
    fn write(&mut self, address: u8, bytes: &[u8]) -> Result<(), Self::Error>;
    fn read(&mut self, address: u8, bytes: &mut [u8]) -> Result<(), Self::Error>;
    fn write_read(&mut self, address: u8, write: &[u8], read: &mut [u8])
        -> Result<(), Self::Error>;
}

/// Portable full-duplex SPI transaction provider.
pub trait HalSpi {
    type Error;
    const TRANSFER_MODE: TransferMode;
    fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> Result<(), Self::Error>;
}

/// Owned one-shot alarm used by ports whose timer peripherals cannot be represented by
/// the legacy static [`HalDeadline`] interface.
pub trait HalAlarm {
    type Error;

    fn arm_after_us(&mut self, delay_us: u64) -> Result<u64, Self::Error>;
    fn cancel(&mut self);
    fn deadline_us(&self) -> Option<u64>;
    fn poll_due(&mut self, now_us: u64) -> bool;
}

/// Owned PWM channel. Frequency/timer selection belongs to construction; application
/// code changes only the bounded duty value.
pub trait HalPwmChannel {
    type Error;

    fn max_duty(&self) -> u16;
    fn set_duty(&mut self, duty: u16) -> Result<(), Self::Error>;
}

/// Bounded byte-stream transport for USB CDC or USB Serial/JTAG providers.
pub trait HalByteIo {
    type Error;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error>;
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
    fn flush(&mut self) -> Result<(), Self::Error>;
}

/// Register readback self-test (replaces scope for CI / autonomous eval).
pub trait HalSelfTest<B: BoardDesc> {
    /// # Safety
    /// Reconfigures live peripherals (PWM/timers) for the self-test scene; caller
    /// must hold the relevant leases and accept the outputs toggling on real pins.
    unsafe fn scene_d_pass(profile: ServoProfile) -> (bool, PwmSnapshot, BoardParity);
}

/// Exclusive peripheral lease with semantics shared across platforms.
pub trait HalLease {
    fn acquire(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError>;
    fn release(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError>;
    fn is_held(resource: impl Into<LeaseId>) -> bool;
    fn owner(resource: impl Into<LeaseId>) -> Option<u8>;
    fn release_all_for_owner(owner: u8) -> usize;
}

/// Root identity marker. Capabilities are implemented through independent provider traits.
pub trait PlatformHal: HalCompatibility {
    const PLATFORM_ID: &'static str;
    type Board: BoardDesc;
}

pub trait HalTimebaseProvider: HalClock {
    /// # Safety
    /// Call once at boot before any timestamped API; starts the platform's
    /// free-running timebase peripheral (caller must own its lease).
    unsafe fn init_timebase();
}

pub trait HalSchedulingProvider:
    HalTimebaseProvider + HalDeadline + HalEventCapture + HalServoPwm
{
    fn servo_profile() -> ServoProfile;
    /// One-shot bring-up: deadline timer, event capture, servo PWM for eval demos.
    /// # Safety
    /// Combines the init methods above - same lease and call-once requirements.
    unsafe fn init_scheduling_demo(profile: ServoProfile);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct LoopbackBus;

    impl HalI2c for LoopbackBus {
        type Error = ();
        const TRANSFER_MODE: TransferMode = TransferMode::Polling;

        fn write(&mut self, address: u8, bytes: &[u8]) -> Result<(), Self::Error> {
            (address < 0x80 && !bytes.is_empty())
                .then_some(())
                .ok_or(())
        }

        fn read(&mut self, address: u8, bytes: &mut [u8]) -> Result<(), Self::Error> {
            bytes.fill(address);
            Ok(())
        }

        fn write_read(
            &mut self,
            address: u8,
            write: &[u8],
            read: &mut [u8],
        ) -> Result<(), Self::Error> {
            self.write(address, write)?;
            self.read(address, read)
        }
    }

    impl HalSpi for LoopbackBus {
        type Error = ();
        const TRANSFER_MODE: TransferMode = TransferMode::Dma;

        fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> Result<(), Self::Error> {
            if write.len() != read.len() {
                return Err(());
            }
            read.copy_from_slice(write);
            Ok(())
        }
    }

    struct Alarm {
        deadline: Option<u64>,
    }

    impl HalAlarm for Alarm {
        type Error = ();

        fn arm_after_us(&mut self, delay_us: u64) -> Result<u64, Self::Error> {
            let deadline = 10 + delay_us;
            self.deadline = Some(deadline);
            Ok(deadline)
        }

        fn cancel(&mut self) {
            self.deadline = None;
        }

        fn deadline_us(&self) -> Option<u64> {
            self.deadline
        }

        fn poll_due(&mut self, now_us: u64) -> bool {
            if self.deadline.is_some_and(|deadline| now_us >= deadline) {
                self.cancel();
                true
            } else {
                false
            }
        }
    }

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

    #[test]
    fn portable_bus_contracts_expose_transactions_and_execution_mode() {
        let mut bus = LoopbackBus;
        let mut i2c = [0; 3];
        HalI2c::write_read(&mut bus, 0x52, &[1], &mut i2c).unwrap();
        assert_eq!(i2c, [0x52; 3]);
        assert_eq!(
            <LoopbackBus as HalI2c>::TRANSFER_MODE,
            TransferMode::Polling
        );

        let mut spi = [0; 3];
        HalSpi::transfer(&mut bus, &[1, 2, 3], &mut spi).unwrap();
        assert_eq!(spi, [1, 2, 3]);
        assert_eq!(<LoopbackBus as HalSpi>::TRANSFER_MODE, TransferMode::Dma);
        assert!(HalSpi::transfer(&mut bus, &[1], &mut spi).is_err());
    }

    #[test]
    fn owned_alarm_releases_after_deadline() {
        let mut alarm = Alarm { deadline: None };
        assert_eq!(alarm.arm_after_us(25), Ok(35));
        assert!(!alarm.poll_due(34));
        assert!(alarm.poll_due(35));
        assert_eq!(alarm.deadline_us(), None);
    }
}
