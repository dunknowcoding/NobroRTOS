//! Universal Driver Interface proof (UDI): one app, one sensor, swappable backends.
//!
//! The evaluation logic lives in `app.rs` and is written **only** against
//! `nobro_sal::ImuSal` - it never names SPI, a register map, or a driver crate. Which
//! backend actually talks to the MPU-9250 is a cargo feature (backend-native /
//! backend-eh / backend-arduino), and which bootloader layout is a board feature.
//! This bin is the no-SoftDevice layout (app @ 0x1000); `main_s140.rs` is the S140
//! layout (app @ 0x26000). Both share the same `app.rs` body.
#![no_std]
#![no_main]

include!("app.rs");
