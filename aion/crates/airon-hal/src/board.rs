//! ProMicro / nice!nano nRF52840 board constants.
//!
//! `board-promicro-nosd`: app at 0x1000 (board1-board3, J-Link).
//! `board-nicenano-s140`: app at 0x26000 (board5, UF2 DFU).

use crate::board_desc::{BoardCapacity, BoardDesc};

#[cfg(feature = "board-nicenano-s140")]
pub const APP_FLASH_START: u32 = 0x26000;
#[cfg(not(feature = "board-nicenano-s140"))]
pub const APP_FLASH_START: u32 = 0x1000;

pub const LED_PIN: u8 = 15;
pub const I2C_SDA_PIN: u8 = 32; // P1.00 / D6
pub const I2C_SCL_PIN: u8 = 11; // P0.11 / D7
pub const SERVO_PWM_PIN: u8 = 24;
pub const SERVO_CENTER_US: u32 = 1500;
pub const MVK_TRIGGER_PIN: u8 = 17;
pub const AIRON_FLASH_BUDGET_BYTES: u32 = 80 * 1024;
pub const AIRON_RAM_BUDGET_BYTES: u32 = 32 * 1024;
pub const AIRON_SAMPLE_POOL_SLOTS: u16 = 8;
pub const AIRON_MAX_MODULES: usize = 16;

pub struct Board;

impl Board {
    pub const APP_START: u32 = APP_FLASH_START;

    pub fn name() -> &'static str {
        Board::BOARD_ID
    }
}

impl BoardDesc for Board {
    const PLATFORM_ID: &'static str = "nrf52840";
    #[cfg(feature = "board-nicenano-s140")]
    const BOARD_ID: &'static str = "promicro_nrf52840_s140";
    #[cfg(not(feature = "board-nicenano-s140"))]
    const BOARD_ID: &'static str = "promicro_nrf52840_nosd";
    const APP_FLASH_START: u32 = APP_FLASH_START;
    const CAPACITY: BoardCapacity = BoardCapacity::new(
        AIRON_FLASH_BUDGET_BYTES,
        AIRON_RAM_BUDGET_BYTES,
        AIRON_SAMPLE_POOL_SLOTS,
        AIRON_MAX_MODULES,
    );
    const LED_PIN: u8 = 15;
    const SERVO_PWM_PIN: u8 = 24;
    const SERVO_CENTER_US: u32 = 1500;
    const MVK_TRIGGER_PIN: u8 = 17;
}
