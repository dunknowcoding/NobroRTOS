//! AIRON hardware abstraction — portable traits + platform backends.
//!
//! Application code should prefer:
//! - `traits::*` and `platform::ActivePlatform` for portable logic
//! - Legacy module paths (`timer`, `pwm`, …) remain for the nRF52840 port

#![no_std]

pub mod board_desc;
pub mod lease;
pub mod platform;
pub mod snapshots;
pub mod traits;

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
pub mod radio_sim;
#[cfg(feature = "platform-nrf52840")]
pub mod timer;

pub use board_desc::{BoardDesc, BusLayout, ServoProfile};
pub use lease::{LeaseError, Resource, ResourceLease};
pub use platform::ActivePlatform;
pub use snapshots::{BoardParity, EventCaptureSnapshot, PwmSnapshot};
pub use traits::{
    HalBus, HalClock, HalDeadline, HalEventCapture, HalLease, HalSelfTest, HalServoPwm,
    PlatformHal,
};

#[cfg(feature = "platform-nrf52840")]
pub use board::Board;
#[cfg(feature = "platform-nrf52840")]
pub use bus::{BusError, TwimBus, TWIM0_BASE, TWIM1_BASE};
#[cfg(feature = "platform-nrf52840")]
pub use deadline_timer::DeadlineTimer;
#[cfg(feature = "platform-nrf52840")]
pub use inspect::scene_d_pass;
#[cfg(feature = "platform-nrf52840")]
pub use pwm::{PwmServo, SERVO_PIN};
#[cfg(feature = "platform-nrf52840")]
pub use radio_sim::RadioRxSim;
#[cfg(feature = "platform-nrf52840")]
pub use timer::MicroTimer;

/// Type alias kept for eval / docs that refer to PPI wiring checks.
#[cfg(feature = "platform-nrf52840")]
pub type PpiRadioSnapshot = EventCaptureSnapshot;
