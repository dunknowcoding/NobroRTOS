//! ProMicro / nice!nano nRF52840 board constants.
//!
//! `board-promicro-nosd`: app at 0x1000 (board1-board3, J-Link).
//! `board-nicenano-s140`: app at 0x26000 (board5, UF2 DFU).

use crate::board_desc::{
    BoardCapacity, BoardDesc, BoardPackage, BoardPins, BootLayout, BootProfile,
};

#[cfg(feature = "board-nicenano-s140")]
pub const APP_FLASH_START: u32 = 0x26000;
#[cfg(not(feature = "board-nicenano-s140"))]
pub const APP_FLASH_START: u32 = 0x1000;

#[cfg(feature = "board-nicenano-s140")]
pub const APP_FLASH_LEN_BYTES: u32 = 798_720;
#[cfg(not(feature = "board-nicenano-s140"))]
pub const APP_FLASH_LEN_BYTES: u32 = 1020 * 1024;

#[cfg(feature = "board-nicenano-s140")]
pub const RAM_START: u32 = 0x2000_6000;
#[cfg(not(feature = "board-nicenano-s140"))]
pub const RAM_START: u32 = 0x2000_0000;

#[cfg(feature = "board-nicenano-s140")]
pub const RAM_LEN_BYTES: u32 = 0x3A000;
#[cfg(not(feature = "board-nicenano-s140"))]
pub const RAM_LEN_BYTES: u32 = 256 * 1024;

#[cfg(feature = "board-nicenano-s140")]
pub const BOOT_LAYOUT: BootLayout = BootLayout::SoftDeviceS140V6;
#[cfg(not(feature = "board-nicenano-s140"))]
pub const BOOT_LAYOUT: BootLayout = BootLayout::NoSoftDevice;

pub const LED_PIN: u8 = 15;
pub const I2C_SDA_PIN: u8 = 32; // P1.00 / D6
pub const I2C_SCL_PIN: u8 = 11; // P0.11 / D7
pub const SERVO_PWM_PIN: u8 = 24;
pub const SERVO_CENTER_US: u32 = 1500;
pub const MVK_TRIGGER_PIN: u8 = 17;
pub const NOBRO_FLASH_BUDGET_BYTES: u32 = 80 * 1024;
pub const NOBRO_RAM_BUDGET_BYTES: u32 = 32 * 1024;
pub const NOBRO_SAMPLE_POOL_SLOTS: u16 = 8;
pub const NOBRO_MAX_MODULES: usize = 16;
pub const BOOT_PROFILE: BootProfile = BootProfile::new(
    BOOT_LAYOUT,
    APP_FLASH_START,
    APP_FLASH_LEN_BYTES,
    RAM_START,
    RAM_LEN_BYTES,
);
pub const BOARD_PINS: BoardPins = BoardPins::new(LED_PIN, SERVO_PWM_PIN, MVK_TRIGGER_PIN);
pub const ACTIVE_BOARD_PACKAGE: BoardPackage = BoardPackage::new(
    Board::PLATFORM_ID,
    Board::BOARD_ID,
    BOOT_PROFILE,
    Board::CAPACITY,
    BOARD_PINS,
);

pub struct Board;

impl Board {
    pub const APP_START: u32 = APP_FLASH_START;

    pub fn name() -> &'static str {
        Board::BOARD_ID
    }

    pub const fn package() -> BoardPackage {
        ACTIVE_BOARD_PACKAGE
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
        NOBRO_FLASH_BUDGET_BYTES,
        NOBRO_RAM_BUDGET_BYTES,
        NOBRO_SAMPLE_POOL_SLOTS,
        NOBRO_MAX_MODULES,
    );
    const LED_PIN: u8 = 15;
    const SERVO_PWM_PIN: u8 = 24;
    const SERVO_CENTER_US: u32 = 1500;
    const MVK_TRIGGER_PIN: u8 = 17;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_board_package_is_valid() {
        let package = Board::package();

        assert_eq!(package.platform_id, Board::PLATFORM_ID);
        assert_eq!(package.board_id, Board::BOARD_ID);
        assert_eq!(package.boot.app_flash_start, APP_FLASH_START);
        assert_eq!(package.boot.app_flash_len_bytes, APP_FLASH_LEN_BYTES);
        assert_eq!(package.boot.ram_start, RAM_START);
        assert_eq!(package.boot.ram_len_bytes, RAM_LEN_BYTES);
        assert_eq!(package.capacity, Board::CAPACITY);
        assert_eq!(package.pins, BOARD_PINS);
        assert_eq!(package.validate(), Ok(()));
    }

    #[test]
    fn active_board_package_matches_selected_boot_layout() {
        #[cfg(feature = "board-nicenano-s140")]
        assert_eq!(Board::package().boot.layout, BootLayout::SoftDeviceS140V6);

        #[cfg(not(feature = "board-nicenano-s140"))]
        assert_eq!(Board::package().boot.layout, BootLayout::NoSoftDevice);
    }

    #[test]
    fn active_board_package_matches_fixture() {
        #[cfg(feature = "board-nicenano-s140")]
        let fixture = crate::board_fixtures::fixture_for_feature("board-nicenano-s140")
            .expect("s140 fixture");

        #[cfg(not(feature = "board-nicenano-s140"))]
        let fixture = crate::board_fixtures::fixture_for_feature("board-promicro-nosd")
            .expect("nosd fixture");

        assert_eq!(Board::package(), fixture.package);
    }

    #[test]
    fn active_board_profile_matches_fixture() {
        #[cfg(feature = "board-nicenano-s140")]
        let fixture = crate::board_fixtures::profile_fixture_for_feature("board-nicenano-s140")
            .expect("s140 fixture");

        #[cfg(not(feature = "board-nicenano-s140"))]
        let fixture = crate::board_fixtures::profile_fixture_for_feature("board-promicro-nosd")
            .expect("nosd fixture");

        assert_eq!(
            crate::snapshots::BoardProfileReport::from_board::<Board>(),
            fixture.report()
        );
    }
}
