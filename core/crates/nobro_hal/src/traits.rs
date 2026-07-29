//! Platform HAL capability traits used by apps and adapters.
//!
//! New MCU ports implement these for a `platform::<soc>::Platform` type and register it
//! as `[features] default = ["platform-nrf52840"]` in `nobro-hal/Cargo.toml`.

use crate::board_desc::{BoardDesc, ServoProfile};
use crate::lease::LeaseError;
use crate::snapshots::EventCaptureSnapshot;

pub const HARDWARE_CAPABILITY_CONTRACT_VERSION: u16 = 2;
pub const HARDWARE_CAPABILITY_COUNT: usize = 23;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HardwareCapability {
    Timebase = 0,
    Deadline = 1,
    Event = 2,
    DmaCompletion = 3,
    Gpio = 4,
    Irq = 5,
    Uart = 6,
    ByteIo = 7,
    Adc = 8,
    Pwm = 9,
    Servo = 10,
    Pulse = 11,
    I2c = 12,
    Spi = 13,
    Usb = 14,
    Watchdog = 15,
    Rtc = 16,
    Flash = 17,
    Reset = 18,
    Power = 19,
    Cache = 20,
    Multicore = 21,
    Lease = 22,
}

/// Compile-time witness that one exact composition has wired a capability.
///
/// Implement this marker only beside the concrete provider composition. The
/// target build compiles the implementation and `HardwareCapabilitySet::witnessed`
/// refuses to construct the witness bit without it.
pub trait HardwareCapabilityWitness<const CAPABILITY: u8> {}

impl HardwareCapability {
    pub const fn bit(self) -> u32 {
        1 << (self as u8)
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Timebase => "timebase",
            Self::Deadline => "deadline",
            Self::Event => "event",
            Self::DmaCompletion => "dma_completion",
            Self::Gpio => "gpio",
            Self::Irq => "irq",
            Self::Uart => "uart",
            Self::ByteIo => "byte_io",
            Self::Adc => "adc",
            Self::Pwm => "pwm",
            Self::Servo => "servo",
            Self::Pulse => "pulse",
            Self::I2c => "i2c",
            Self::Spi => "spi",
            Self::Usb => "usb",
            Self::Watchdog => "watchdog",
            Self::Rtc => "rtc",
            Self::Flash => "flash",
            Self::Reset => "reset",
            Self::Power => "power",
            Self::Cache => "cache",
            Self::Multicore => "multicore",
            Self::Lease => "lease",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HardwareCapabilitySet(pub u32);

impl HardwareCapabilitySet {
    pub const EMPTY: Self = Self(0);
    pub const ALL: Self = Self((1 << HARDWARE_CAPABILITY_COUNT) - 1);

    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn with(self, capability: HardwareCapability) -> Self {
        Self(self.0 | capability.bit())
    }

    /// Add a capability only when the composition supplies its compile witness.
    ///
    /// ```compile_fail
    /// use nobro_hal::{HardwareCapability, HardwareCapabilitySet};
    ///
    /// struct UnwitnessedComposition;
    /// let _ = HardwareCapabilitySet::EMPTY
    ///     .witnessed::<UnwitnessedComposition, { HardwareCapability::Usb as u8 }>(
    ///         HardwareCapability::Usb,
    ///     );
    /// ```
    pub const fn witnessed<T, const CAPABILITY: u8>(self, capability: HardwareCapability) -> Self
    where
        T: HardwareCapabilityWitness<CAPABILITY>,
    {
        assert!(capability as u8 == CAPABILITY);
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

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn only_known(self) -> bool {
        self.0 & !Self::ALL.0 == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityProfileKind {
    Deep,
    Constrained,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityDeclarationState {
    Required,
    Supported,
    HardwareInapplicable,
    Unimplemented,
}

/// Versioned, four-state capability declaration for one exact composition.
///
/// `profile_required` names the selected profile's requirements. A required
/// capability remains in [`CapabilityDeclarationState::Required`] until a
/// concrete compiled witness moves it to `Supported`. Hardware-inapplicable
/// and unimplemented are separate fail-closed states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HardwareCapabilityDeclaration {
    pub contract_version: u16,
    pub profile_id: &'static str,
    pub profile_kind: CapabilityProfileKind,
    pub profile_required: HardwareCapabilitySet,
    pub supported: HardwareCapabilitySet,
    pub hardware_inapplicable: HardwareCapabilitySet,
    pub unimplemented: HardwareCapabilitySet,
    pub trait_witnesses: HardwareCapabilitySet,
}

impl HardwareCapabilityDeclaration {
    pub const fn new(
        profile_id: &'static str,
        profile_kind: CapabilityProfileKind,
        profile_required: HardwareCapabilitySet,
        supported: HardwareCapabilitySet,
        hardware_inapplicable: HardwareCapabilitySet,
        unimplemented: HardwareCapabilitySet,
        trait_witnesses: HardwareCapabilitySet,
    ) -> Self {
        Self {
            contract_version: HARDWARE_CAPABILITY_CONTRACT_VERSION,
            profile_id,
            profile_kind,
            profile_required,
            supported,
            hardware_inapplicable,
            unimplemented,
            trait_witnesses,
        }
    }

    pub const fn pending_required(self) -> HardwareCapabilitySet {
        self.supported.missing(self.profile_required)
    }

    pub const fn state(self, capability: HardwareCapability) -> CapabilityDeclarationState {
        if self.supported.contains(capability) {
            CapabilityDeclarationState::Supported
        } else if self.profile_required.contains(capability) {
            CapabilityDeclarationState::Required
        } else if self.hardware_inapplicable.contains(capability) {
            CapabilityDeclarationState::HardwareInapplicable
        } else {
            CapabilityDeclarationState::Unimplemented
        }
    }

    pub const fn profile_is_satisfied(self) -> bool {
        self.pending_required().is_empty()
    }

    pub const fn is_exact_profile(self) -> bool {
        self.profile_is_satisfied() && self.supported.bits() == self.profile_required.bits()
    }

    pub const fn is_valid(self) -> bool {
        let pending = self.pending_required();
        let classified = self
            .supported
            .union(pending)
            .union(self.hardware_inapplicable)
            .union(self.unimplemented);
        self.contract_version == HARDWARE_CAPABILITY_CONTRACT_VERSION
            && !self.profile_id.is_empty()
            && self.profile_required.only_known()
            && self.supported.only_known()
            && self.hardware_inapplicable.only_known()
            && self.unimplemented.only_known()
            && self.trait_witnesses.only_known()
            && self.supported.bits() == self.trait_witnesses.bits()
            && self
                .supported
                .intersection(self.hardware_inapplicable)
                .is_empty()
            && self.supported.intersection(self.unimplemented).is_empty()
            && pending.intersection(self.hardware_inapplicable).is_empty()
            && pending.intersection(self.unimplemented).is_empty()
            && self
                .hardware_inapplicable
                .intersection(self.unimplemented)
                .is_empty()
            && classified.bits() == HardwareCapabilitySet::ALL.bits()
    }
}

/// Platform capability metadata for host-side and compile-time compatibility checks.
pub trait HalCompatibility {
    const DECLARATION: HardwareCapabilityDeclaration;
    const CAPABILITIES: HardwareCapabilitySet = Self::DECLARATION.supported;

    fn supports(required: HardwareCapabilitySet) -> bool {
        Self::DECLARATION.supported.contains_all(required)
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
    Adc,
    Uart,
    Usb,
    Dma,
    Power,
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
    pub const EVENT_CAPTURE_TIMER: Self = Self::new(LeaseClass::Timer, 2);
    pub const DEADLINE_TIMER: Self = Self::new(LeaseClass::Timer, 1);
    pub const PRIMARY_I2C: Self = Self::new(LeaseClass::I2c, 0);
    pub const SECONDARY_I2C: Self = Self::new(LeaseClass::I2c, 1);
    pub const PRIMARY_SPI: Self = Self::new(LeaseClass::Spi, 0);
    pub const PRIMARY_RADIO: Self = Self::new(LeaseClass::Radio, 0);
    pub const PRIMARY_PWM: Self = Self::new(LeaseClass::Pwm, 0);
    pub const EVENT_ROUTER: Self = Self::new(LeaseClass::EventRouter, 0);
    pub const SOFTWARE_EVENT: Self = Self::new(LeaseClass::SoftwareEvent, 0);
    pub const PRIMARY_ADC: Self = Self::new(LeaseClass::Adc, 0);
    pub const PRIMARY_UART: Self = Self::new(LeaseClass::Uart, 0);
    pub const USB_DEVICE: Self = Self::new(LeaseClass::Usb, 0);
    pub const PRIMARY_DMA: Self = Self::new(LeaseClass::Dma, 0);
    pub const SYSTEM_POWER: Self = Self::new(LeaseClass::Power, 0);
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

/// Owned single-ended ADC channel.
///
/// Pin muxing, reference selection, and acquisition timing belong to construction.
/// Reads are bounded and return an error instead of waiting forever for hardware.
pub trait HalAdcChannel {
    type Error;

    fn max_sample(&self) -> u16;
    fn read(&mut self) -> Result<u16, Self::Error>;
}

/// Bounded byte-stream transport for USB CDC or USB Serial/JTAG providers.
pub trait HalByteIo {
    type Error;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error>;
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
    fn flush(&mut self) -> Result<(), Self::Error>;
}

/// Reset status and the fail-closed system-reset operation.
pub trait HalReset {
    type Cause: Copy + Eq;

    fn reset_cause() -> Self::Cause;
    fn system_reset() -> !;
}

/// Sleep state entered by a portable power provider.
///
/// `CpuSleep` retains the platform's admitted peripheral clocks and RAM. Deeper
/// modes are deliberately separate capabilities because wake sources and retained
/// state vary by exact board composition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleMode {
    CpuSleep,
}

/// Owned low-power entry point.
///
/// A provider must return only after an admitted interrupt wakes the CPU. It may
/// reject entry when a mounted transport or active peripheral vetoes sleep.
pub trait HalPower {
    type Error;

    fn idle(&mut self, mode: IdleMode) -> Result<(), Self::Error>;
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
    /// One-shot bring-up for deadline timer, event capture, and servo PWM examples.
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
            .with(HardwareCapability::Lease);
        let required = HardwareCapabilitySet::EMPTY
            .with(HardwareCapability::Timebase)
            .with(HardwareCapability::I2c);

        assert!(platform.contains(HardwareCapability::Timebase));
        assert!(!platform.contains_all(required));
        assert_eq!(
            platform.missing(required),
            HardwareCapabilitySet::EMPTY.with(HardwareCapability::I2c)
        );
    }

    #[test]
    fn capability_declaration_partitions_every_v2_capability() {
        let required = HardwareCapabilitySet::EMPTY
            .with(HardwareCapability::Timebase)
            .with(HardwareCapability::Deadline);
        let supported = HardwareCapabilitySet::EMPTY.with(HardwareCapability::Timebase);
        let inapplicable = HardwareCapabilitySet::EMPTY.with(HardwareCapability::Multicore);
        let unimplemented = HardwareCapabilitySet::ALL
            .without(required)
            .without(inapplicable);
        let declaration = HardwareCapabilityDeclaration::new(
            "test-constrained-v2",
            CapabilityProfileKind::Constrained,
            required,
            supported,
            inapplicable,
            unimplemented,
            supported,
        );

        assert!(declaration.is_valid());
        assert!(!declaration.profile_is_satisfied());
        assert_eq!(
            declaration.state(HardwareCapability::Deadline),
            CapabilityDeclarationState::Required
        );
        assert_eq!(
            declaration.state(HardwareCapability::Timebase),
            CapabilityDeclarationState::Supported
        );
        assert_eq!(
            declaration.state(HardwareCapability::Multicore),
            CapabilityDeclarationState::HardwareInapplicable
        );
        assert_eq!(
            declaration.state(HardwareCapability::Gpio),
            CapabilityDeclarationState::Unimplemented
        );
    }

    #[test]
    fn declaration_rejects_unwitnessed_support_and_overlapping_states() {
        let supported = HardwareCapabilitySet::EMPTY.with(HardwareCapability::Timebase);
        let unimplemented = HardwareCapabilitySet::ALL.without(supported);
        let unwitnessed = HardwareCapabilityDeclaration::new(
            "bad-v2",
            CapabilityProfileKind::Deep,
            supported,
            supported,
            HardwareCapabilitySet::EMPTY,
            unimplemented,
            HardwareCapabilitySet::EMPTY,
        );
        assert!(!unwitnessed.is_valid());

        let overlap = HardwareCapabilityDeclaration::new(
            "bad-v2",
            CapabilityProfileKind::Deep,
            supported,
            supported,
            supported,
            unimplemented,
            supported,
        );
        assert!(!overlap.is_valid());
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
