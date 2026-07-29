//! ProMicro nRF52840 board constants.
//!
//! `board-promicro-nosd`: no-SoftDevice layout, app at 0x1000.
//! `board-promicro-s140`: S140 SoftDevice layout, app at 0x26000.

use crate::board_catalog::{PROMICRO_NRF52840_NOSD_PACKAGE, PROMICRO_NRF52840_S140_PACKAGE};
use crate::board_desc::{BoardCapacity, BoardDesc, BoardPackage, BoardPins, BootLayout};

pub const LED_PIN: u8 = 15;
pub const I2C_SDA_PIN: u8 = 32; // P1.00 / D6
pub const I2C_SCL_PIN: u8 = 11; // P0.11 / D7
pub const SERVO_PWM_PIN: u8 = 24;
pub const SERVO_CENTER_US: u32 = 1500;
pub const MVK_TRIGGER_PIN: u8 = 17;
// Reference SPI IMU wiring: SCK=P0.17(D2), MISO=P0.20(D3), MOSI=P0.22(D4),
// CS=P0.24(D5); INT=P0.11(D7), FSYNC=P1.00(D6) for ISR-based sampling. (CS shares the
// servo pin; the SPI IMU demo does not drive the servo.)
pub const SPI_SCK_PIN: u8 = 17;
pub const SPI_MISO_PIN: u8 = 20;
pub const SPI_MOSI_PIN: u8 = 22;
pub const SPI_CS_PIN: u8 = 24;
pub const IMU_INT_PIN: u8 = 11;
pub const IMU_FSYNC_PIN: u8 = 32;

/// Exact ProMicro composition with a bootloader but no resident SoftDevice.
pub struct ProMicroNrf52840NoSoftDevice;

/// Exact ProMicro composition with the S140 v6 SoftDevice-reserved layout.
pub struct ProMicroNrf52840S140V6;

macro_rules! impl_promicro_board {
    ($board:ty, $package:expr) => {
        impl $board {
            pub const APP_START: u32 = $package.boot.app_flash_start;

            pub fn name() -> &'static str {
                <$board as BoardDesc>::BOARD_ID
            }

            pub const fn package() -> BoardPackage {
                $package
            }
        }

        impl BoardDesc for $board {
            const PLATFORM_ID: &'static str = $package.platform_id;
            const BOARD_ID: &'static str = $package.board_id;
            const APP_FLASH_START: u32 = $package.boot.app_flash_start;
            const CAPACITY: BoardCapacity = $package.capacity;
            const LED_PIN: Option<u8> = $package.pins.led_pin;
            const SERVO_PWM_PIN: Option<u8> = $package.pins.servo_pwm_pin;
            const SERVO_CENTER_US: u32 = SERVO_CENTER_US;
            const MVK_TRIGGER_PIN: Option<u8> = $package.pins.mvk_trigger_pin;
        }
    };
}

impl_promicro_board!(ProMicroNrf52840NoSoftDevice, PROMICRO_NRF52840_NOSD_PACKAGE);
impl_promicro_board!(ProMicroNrf52840S140V6, PROMICRO_NRF52840_S140_PACKAGE);

#[cfg(feature = "board-promicro-s140")]
pub type Board = ProMicroNrf52840S140V6;
#[cfg(not(feature = "board-promicro-s140"))]
pub type Board = ProMicroNrf52840NoSoftDevice;

pub const ACTIVE_BOARD_PACKAGE: BoardPackage = Board::package();
pub const APP_FLASH_START: u32 = ACTIVE_BOARD_PACKAGE.boot.app_flash_start;
pub const APP_FLASH_LEN_BYTES: u32 = ACTIVE_BOARD_PACKAGE.boot.app_flash_len_bytes;
pub const RAM_START: u32 = ACTIVE_BOARD_PACKAGE.boot.ram_start;
pub const RAM_LEN_BYTES: u32 = ACTIVE_BOARD_PACKAGE.boot.ram_len_bytes;
pub const BOOT_LAYOUT: BootLayout = ACTIVE_BOARD_PACKAGE.boot.layout;
pub const NOBRO_FLASH_BUDGET_BYTES: u32 = ACTIVE_BOARD_PACKAGE.capacity.flash_budget_bytes;
pub const NOBRO_RAM_BUDGET_BYTES: u32 = ACTIVE_BOARD_PACKAGE.capacity.ram_budget_bytes;
pub const NOBRO_SAMPLE_POOL_SLOTS: u16 = ACTIVE_BOARD_PACKAGE.capacity.sample_pool_slots;
pub const NOBRO_MAX_MODULES: usize = ACTIVE_BOARD_PACKAGE.capacity.max_modules;
pub const BOARD_PINS: BoardPins = ACTIVE_BOARD_PACKAGE.pins;

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
        #[cfg(feature = "board-promicro-s140")]
        assert_eq!(Board::package().boot.layout, BootLayout::SoftDeviceS140V6);

        #[cfg(not(feature = "board-promicro-s140"))]
        assert_eq!(Board::package().boot.layout, BootLayout::NoSoftDevice);
    }

    #[test]
    fn both_exact_packages_remain_available_without_feature_switching() {
        assert_eq!(
            ProMicroNrf52840NoSoftDevice::package(),
            PROMICRO_NRF52840_NOSD_PACKAGE
        );
        assert_eq!(
            ProMicroNrf52840S140V6::package(),
            PROMICRO_NRF52840_S140_PACKAGE
        );
        assert_ne!(
            ProMicroNrf52840NoSoftDevice::APP_START,
            ProMicroNrf52840S140V6::APP_START
        );
    }

    #[test]
    fn active_board_package_matches_entry() {
        #[cfg(feature = "board-promicro-s140")]
        let entry =
            crate::board_catalog::package_for_feature("board-promicro-s140").expect("s140 entry");

        #[cfg(not(feature = "board-promicro-s140"))]
        let entry =
            crate::board_catalog::package_for_feature("board-promicro-nosd").expect("nosd entry");

        assert_eq!(Board::package(), entry.package);
    }

    #[test]
    fn active_board_profile_matches_entry() {
        #[cfg(feature = "board-promicro-s140")]
        let entry =
            crate::board_catalog::profile_for_feature("board-promicro-s140").expect("s140 entry");

        #[cfg(not(feature = "board-promicro-s140"))]
        let entry =
            crate::board_catalog::profile_for_feature("board-promicro-nosd").expect("nosd entry");

        assert_eq!(
            crate::snapshots::BoardProfileReport::from_board::<Board>(),
            entry.report()
        );
    }
}
