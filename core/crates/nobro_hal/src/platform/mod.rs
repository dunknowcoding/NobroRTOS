//! Platform backend selector. Enable exactly one `platform-*` feature per build.
//!
//! Adding a new MCU:
//! 1. Create `platform/<soc>/mod.rs` implementing `traits::PlatformHal`.
//! 2. Add `[features] platform-<soc> = []` in `airon-hal/Cargo.toml`.
//! 3. Add board features that depend on the platform feature.
//! 4. Provide `memory.x` + flash script under `boards/<board>/`.

#[cfg(any(
    all(feature = "platform-nrf52840", feature = "platform-esp32"),
    all(feature = "platform-nrf52840", feature = "platform-rp2040"),
    all(feature = "platform-nrf52840", feature = "platform-stm32"),
    all(feature = "platform-esp32", feature = "platform-rp2040"),
    all(feature = "platform-esp32", feature = "platform-stm32"),
    all(feature = "platform-rp2040", feature = "platform-stm32"),
))]
compile_error!("airon-hal: enable exactly one platform-* feature");

#[cfg(feature = "platform-nrf52840")]
pub mod nrf52840;

#[cfg(feature = "platform-nrf52840")]
pub use nrf52840::Active as ActivePlatform;

#[cfg(all(not(feature = "platform-nrf52840"), feature = "platform-esp32"))]
compile_error!("platform-esp32 is reserved for a future port; implement platform/esp32/ first");

#[cfg(all(not(feature = "platform-nrf52840"), feature = "platform-rp2040"))]
compile_error!("platform-rp2040 is reserved for a future port; implement platform/rp2040/ first");

#[cfg(all(not(feature = "platform-nrf52840"), feature = "platform-stm32"))]
compile_error!("platform-stm32 is reserved for a future port; implement platform/stm32/ first");

#[cfg(all(
    not(feature = "platform-nrf52840"),
    not(feature = "platform-esp32"),
    not(feature = "platform-rp2040"),
    not(feature = "platform-stm32"),
    not(feature = "contract-only"),
))]
compile_error!("airon-hal: enable one platform feature (e.g. platform-nrf52840)");
