#![no_std]

#[cfg(test)]
extern crate std;

#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub mod deep;
pub mod event_capture;
pub mod event_dma;
pub mod evidence;
pub mod lease;
pub mod peripherals;
pub mod power_reset;
pub mod providers;
pub mod system;
pub mod usb_session;
