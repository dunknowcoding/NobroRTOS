//! Platform backend selector. Enable exactly one `platform-*` feature per build.
//!
//! Adding a new MCU:
//! 1. Create `platform/<soc>/mod.rs` implementing `traits::PlatformHal`.
//! 2. Add `[features] platform-<soc> = []` in `nobro-hal/Cargo.toml`.
//! 3. Add board features that depend on the platform feature.
//! 4. Provide `memory.x` + flash script under `boards/<board>/`.

#[cfg(feature = "platform-nrf52840")]
pub mod nrf52840;

#[cfg(feature = "platform-nrf52840")]
pub use nrf52840::Active as ActivePlatform;

#[cfg(all(
    not(feature = "platform-nrf52840"),
    not(feature = "contract-only"),
))]
compile_error!("nobro-hal: enable one platform feature (e.g. platform-nrf52840)");
