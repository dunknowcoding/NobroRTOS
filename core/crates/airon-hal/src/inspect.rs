//! Register readback facade that delegates to `platform::<soc>/inspect`.

#[cfg(feature = "platform-nrf52840")]
pub use crate::platform::nrf52840::inspect::scene_d_pass;
