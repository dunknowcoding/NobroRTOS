//! Board package template — copy when adding a new board (no runtime code yet).
//!
//! Future layout:
//! ```text
//! boards/
//!   promicro-nrf52840/   memory.x, board.json, feature flags
//!   rp2040-pico/
//! ```
//! Each board crate re-exports `BoardDesc` constants and selects `platform-*` in `Cargo.toml`.
