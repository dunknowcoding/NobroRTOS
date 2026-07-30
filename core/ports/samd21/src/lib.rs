#![no_std]

#[cfg(test)]
extern crate std;

#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(target_arch = "arm")]
pub mod board;
pub mod event_dma;
pub mod lease;
pub mod power_reset;
pub mod providers;
