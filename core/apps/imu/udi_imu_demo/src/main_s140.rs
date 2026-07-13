//! UDI example, S140 SoftDevice layout (app @ 0x26000). Same `app.rs` body as the
//! no-SoftDevice `main.rs`; only the linked flash origin differs (board feature).
#![no_std]
#![no_main]

include!("app.rs");
