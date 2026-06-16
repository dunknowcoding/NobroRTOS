//! ProMicro nRF52840 board constants (board1–board3 no-SoftDevice layout).

use crate::board_desc::BoardDesc;

/// Application flash origin for no-SoftDevice ProMicro clones.
pub const APP_FLASH_START: u32 = 0x1000;

/// Built-in LED P0.15 (active high).
pub const LED_PIN: u8 = 15;

/// Demo servo PWM: D5 / P0.24.
pub const SERVO_PWM_PIN: u8 = 24;

/// Standard 50 Hz servo center pulse (µs).
pub const SERVO_CENTER_US: u32 = 1500;

/// MVK trigger input — D2 silk, P0.17.
pub const MVK_TRIGGER_PIN: u8 = 17;

pub struct Board;

impl Board {
    pub const APP_START: u32 = APP_FLASH_START;

    pub fn name() -> &'static str {
        Board::BOARD_ID
    }
}

impl BoardDesc for Board {
    const PLATFORM_ID: &'static str = "nrf52840";
    const BOARD_ID: &'static str = "promicro_nrf52840_nosd";
    const APP_FLASH_START: u32 = 0x1000;
    const LED_PIN: u8 = 15;
    const SERVO_PWM_PIN: u8 = 24;
    const SERVO_CENTER_US: u32 = 1500;
    const MVK_TRIGGER_PIN: u8 = 17;
}
