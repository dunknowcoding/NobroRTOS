//! NobroRTOS hardware abstraction with portable traits and platform backends.
//!
//! Application code should prefer:
//! - `traits::*` and `platform::ActivePlatform` for portable logic
//! - Legacy module paths (`timer`, `pwm`, etc.) remain for the nRF52840 port

#![no_std]

pub mod board_desc;
pub mod board_fixtures;
pub mod lease;
pub mod platform;
pub mod snapshots;
pub mod traits;

#[cfg(all(feature = "board-promicro-nosd", feature = "board-nicenano-s140"))]
compile_error!("nobro-hal: enable exactly one board-* feature");

#[cfg(all(
    feature = "platform-nrf52840",
    not(any(feature = "board-promicro-nosd", feature = "board-nicenano-s140"))
))]
compile_error!("nobro-hal: enable one board-* feature");

#[cfg(feature = "platform-nrf52840")]
pub mod board;
#[cfg(feature = "platform-nrf52840")]
pub mod bus;
#[cfg(feature = "platform-nrf52840")]
pub mod deadline_timer;
#[cfg(feature = "platform-nrf52840")]
pub mod inspect;
#[cfg(feature = "platform-nrf52840")]
pub mod ppi;
#[cfg(feature = "platform-nrf52840")]
pub mod pwm;
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

pub use board_desc::{
    BoardCapacity, BoardDesc, BoardPackage, BoardPackageError, BoardPins, BootLayout, BootProfile,
    BusLayout, ServoProfile,
};
pub use board_fixtures::{
    fixture_for_feature, profile_fixture_for_feature, BoardPackageFixture, BoardProfileFixture,
    BOARD_PACKAGE_FIXTURES, BOARD_PROFILE_FIXTURES, NRF52840_BOARD_CAPACITY, NRF52840_BOARD_PINS,
    NRF52840_SERVO_CENTER_US, PROMICRO_NRF52840_NOSD_PACKAGE, PROMICRO_NRF52840_S140_PACKAGE,
};
pub use lease::{LeaseError, LeaseGuard, Resource, ResourceLease};
#[cfg(feature = "platform-nrf52840")]
pub use platform::ActivePlatform;
pub use snapshots::{
    BoardPackageReport, BoardParity, BoardProfileReport, EventCaptureSnapshot, PwmSnapshot,
    BOARD_PACKAGE_REPORT_MAGIC, BOARD_PACKAGE_REPORT_VERSION, BOARD_PROFILE_REPORT_MAGIC,
    BOARD_PROFILE_REPORT_VERSION,
};
pub use traits::{
    HalBus, HalClock, HalCompatibility, HalDeadline, HalEventCapture, HalLease, HalSelfTest,
    HalServoPwm, HardwareCapability, HardwareCapabilitySet, PlatformHal,
};

#[cfg(feature = "platform-nrf52840")]
pub use board::{Board, ACTIVE_BOARD_PACKAGE, I2C_SCL_PIN, I2C_SDA_PIN};
#[cfg(feature = "platform-nrf52840")]
pub use bus::{BusError, TwimBus, TWIM0_BASE, TWIM1_BASE};
#[cfg(feature = "platform-nrf52840")]
pub use deadline_timer::DeadlineTimer;
#[cfg(feature = "platform-nrf52840")]
pub use inspect::scene_d_pass;
#[cfg(feature = "platform-nrf52840")]
pub use pwm::{PwmBank, PwmServo, SERVO_PIN};
#[cfg(feature = "platform-nrf52840")]
pub use radio_hw::Radio;
#[cfg(feature = "platform-nrf52840")]
pub use radio_sim::RadioRxSim;
#[cfg(feature = "platform-nrf52840")]
pub use spim_hw::Spim0;
#[cfg(feature = "platform-nrf52840")]
pub use timer::MicroTimer;
#[cfg(feature = "platform-nrf52840")]
pub use twim_hw::Twim0;

/// Type alias kept for eval / docs that refer to PPI wiring checks.
#[cfg(feature = "platform-nrf52840")]
pub type PpiRadioSnapshot = EventCaptureSnapshot;
