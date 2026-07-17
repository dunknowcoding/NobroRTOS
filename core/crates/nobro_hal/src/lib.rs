//! NobroRTOS hardware abstraction with portable traits and platform backends.
//!
//! Application code should prefer:
//! - `traits::*` and `platform::ActivePlatform` for portable logic
//! - Legacy module paths (`timer`, `pwm`, etc.) remain for the nRF52840 port

#![no_std]

pub mod board_catalog;
pub mod board_desc;
pub mod completion;
pub mod lease;
pub mod mpu;
pub mod platform;
pub mod snapshots;
pub mod traits;

#[cfg(all(feature = "board-promicro-nosd", feature = "board-nicenano-s140"))]
compile_error!("nobro-hal: enable exactly one board-* feature");

#[cfg(all(feature = "cortex-m-slice", feature = "board-nicenano-s140"))]
compile_error!(
    "nobro-hal: cortex-m-slice cannot be combined with board-nicenano-s140; \
     the current port programs PendSV through CMSIS and has no SoftDevice NVIC integration"
);

#[cfg(all(
    feature = "platform-nrf52840",
    not(any(feature = "board-promicro-nosd", feature = "board-nicenano-s140"))
))]
compile_error!("nobro-hal: enable one board-* feature");

#[cfg(feature = "platform-nrf52840")]
pub mod board;
#[cfg(feature = "platform-nrf52840")]
pub mod bus;
#[cfg(feature = "cortex-m-slice")]
pub mod context_switch;
#[cfg(feature = "platform-nrf52840")]
pub mod deadline_timer;
#[cfg(feature = "platform-nrf52840-rt")]
pub mod power_nrf;
#[cfg(feature = "platform-nrf52840")]
pub mod ppi;
#[cfg(feature = "platform-nrf52840")]
pub mod priority_ceiling;
#[cfg(feature = "platform-nrf52840")]
pub mod pwm;
mod quiesce;
#[cfg(feature = "platform-nrf52840")]
pub mod radio_hw;
#[cfg(feature = "platform-nrf52840")]
pub mod radio_sim;
#[cfg(feature = "platform-nrf52840")]
pub mod spim_hw;
#[cfg(feature = "platform-nrf52840")]
pub mod timer;
#[cfg(feature = "platform-nrf52840")]
pub mod twim_hw;

pub use board_catalog::{
    package_for_feature, profile_for_feature, BoardPackageDefinition, BoardProfileDefinition,
    BOARD_PACKAGES, BOARD_PROFILES, NRF52840_BOARD_CAPACITY, NRF52840_BOARD_PINS,
    NRF52840_SERVO_CENTER_US, PROMICRO_NRF52840_NOSD_PACKAGE, PROMICRO_NRF52840_S140_PACKAGE,
};
pub use board_desc::{
    BoardCapacity, BoardDesc, BoardPackage, BoardPackageError, BoardPins, BootLayout, BootProfile,
    BusLayout, ServoProfile,
};
pub use completion::{CompletionCell, CompletionError};
pub use lease::{LeaseError, LeaseGuard, Resource, ResourceLease};
#[cfg(feature = "platform-nrf52840")]
pub use platform::nrf52840::NrfSchedulingSession;
#[cfg(feature = "platform-nrf52840")]
pub use platform::ActivePlatform;
pub use snapshots::{
    BoardPackageReport, BoardParity, BoardProfileReport, EventCaptureSnapshot, PwmSnapshot,
    BOARD_PACKAGE_REPORT_MAGIC, BOARD_PACKAGE_REPORT_VERSION, BOARD_PROFILE_REPORT_MAGIC,
    BOARD_PROFILE_REPORT_VERSION,
};
pub use traits::{
    HalAlarm, HalBus, HalByteIo, HalClock, HalCompatibility, HalDeadline, HalEventCapture, HalI2c,
    HalLease, HalPwmChannel, HalSchedulingProvider, HalServoPwm, HalSpi, HalTimebaseProvider,
    HardwareCapability, HardwareCapabilitySet, LeaseClass, LeaseId, PlatformHal, TransferMode,
};

#[cfg(feature = "platform-nrf52840")]
pub use board::{Board, ACTIVE_BOARD_PACKAGE, I2C_SCL_PIN, I2C_SDA_PIN};
#[cfg(feature = "platform-nrf52840")]
pub use bus::{BusError, TwimBus, TWIM0_BASE, TWIM1_BASE};
#[cfg(feature = "cortex-m-slice")]
pub use context_switch::{ContextRecord, ContextSwitchError, CortexMSliceSwitch};
#[cfg(feature = "platform-nrf52840")]
pub use deadline_timer::DeadlineTimer;
#[cfg(feature = "platform-nrf52840-rt")]
pub use power_nrf::NrfTimerPower;
#[cfg(feature = "platform-nrf52840")]
pub use priority_ceiling::{PriorityCeiling, PriorityCeilingError};
#[cfg(feature = "platform-nrf52840")]
pub use pwm::{PwmBank, PwmBankSession, PwmServo, PwmSession, SERVO_PIN};
#[cfg(feature = "platform-nrf52840")]
pub use radio_hw::{Radio, RadioError, RadioSession};
#[cfg(feature = "platform-nrf52840")]
pub use radio_sim::RadioRxSim;
#[cfg(feature = "platform-nrf52840")]
pub use spim_hw::Spim0;
#[cfg(feature = "platform-nrf52840")]
pub use timer::MicroTimer;
#[cfg(feature = "platform-nrf52840")]
pub use twim_hw::Twim0;

/// Event-capture snapshot produced by the nRF52840 PPI provider.
#[cfg(feature = "platform-nrf52840")]
pub type PpiRadioSnapshot = EventCaptureSnapshot;
