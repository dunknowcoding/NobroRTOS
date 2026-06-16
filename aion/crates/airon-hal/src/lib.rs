//! nRF52840 HAL aligned with ArduinoNRF NrfPeripherals semantics.

#![no_std]

pub mod board;
pub mod lease;
pub mod ppi;
pub mod timer;

pub use board::Board;
pub use lease::{LeaseError, Resource, ResourceLease};
pub use timer::MicroTimer;
