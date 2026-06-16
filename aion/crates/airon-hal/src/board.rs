//! ProMicro nRF52840 board constants (board1–board3 no-SoftDevice layout).

/// Application flash origin for no-SoftDevice ProMicro clones.
pub const APP_FLASH_START: u32 = 0x1000;

/// Built-in LED P0.15 (active high).
pub const LED_PIN: u8 = 15;

/// MVK trigger input — D2 silk, P0.17 (GPIOTE + PPI capture demo).
pub const MVK_TRIGGER_PIN: u8 = 17;

pub struct Board;

impl Board {
    pub const APP_START: u32 = APP_FLASH_START;
}
