//! Board contract entries for host-side review without board feature switching.

use crate::{
    board_desc::{BoardCapacity, BoardPackage, BoardPins, BootLayout, BootProfile},
    snapshots::{BoardPackageReport, BoardProfileReport},
};

pub const NRF52840_PLATFORM_ID: &str = "nrf52840";
pub const SAMD21_PLATFORM_ID: &str = "samd21";
pub const STM32F4_PLATFORM_ID: &str = "stm32f4";
pub const IMXRT1062_PLATFORM_ID: &str = "imxrt1062";
pub const CORTEX_M_PLATFORM_ID: &str = "cortex_m";
pub const PROMICRO_NRF52840_NOSD_ID: &str = "promicro_nrf52840_nosd";
pub const PROMICRO_NRF52840_S140_ID: &str = "promicro_nrf52840_s140";
pub const SAMD21_UF2_ID: &str = "samd21_uf2_generic";
pub const STM32F4_GENERIC_ID: &str = "stm32f4_generic";
pub const TEENSY4_GENERIC_ID: &str = "teensy4_generic";
pub const CORTEX_M_GENERIC_ID: &str = "cortex_m_generic";

pub const NRF52840_BOARD_CAPACITY: BoardCapacity = BoardCapacity::new(80 * 1024, 32 * 1024, 8, 16);
pub const NRF52840_BOARD_PINS: BoardPins = BoardPins::new(15, 24, 17);
pub const NRF52840_SERVO_CENTER_US: u32 = 1500;
pub const SAMD21_BOARD_CAPACITY: BoardCapacity = BoardCapacity::new(48 * 1024, 12 * 1024, 4, 8);
pub const SAMD21_BOARD_PINS: BoardPins = BoardPins::new(13, 5, 6);
pub const STM32F4_BOARD_CAPACITY: BoardCapacity = BoardCapacity::new(128 * 1024, 64 * 1024, 8, 16);
pub const STM32F4_BOARD_PINS: BoardPins = BoardPins::new(13, 6, 7);
pub const TEENSY4_BOARD_CAPACITY: BoardCapacity =
    BoardCapacity::new(256 * 1024, 128 * 1024, 16, 24);
pub const TEENSY4_BOARD_PINS: BoardPins = BoardPins::new(13, 2, 3);
pub const CORTEX_M_BOARD_CAPACITY: BoardCapacity = BoardCapacity::new(32 * 1024, 8 * 1024, 4, 6);
pub const CORTEX_M_BOARD_PINS: BoardPins = BoardPins::new(1, 2, 3);

pub const PROMICRO_NRF52840_NOSD_PACKAGE: BoardPackage = BoardPackage::new(
    NRF52840_PLATFORM_ID,
    PROMICRO_NRF52840_NOSD_ID,
    BootProfile::new(
        BootLayout::NoSoftDevice,
        0x1000,
        1020 * 1024,
        0x2000_0000,
        256 * 1024,
    ),
    NRF52840_BOARD_CAPACITY,
    NRF52840_BOARD_PINS,
);

pub const PROMICRO_NRF52840_S140_PACKAGE: BoardPackage = BoardPackage::new(
    NRF52840_PLATFORM_ID,
    PROMICRO_NRF52840_S140_ID,
    BootProfile::new(
        BootLayout::SoftDeviceS140V6,
        0x26000,
        798_720,
        0x2000_6000,
        0x3A000,
    ),
    NRF52840_BOARD_CAPACITY,
    NRF52840_BOARD_PINS,
);

pub const SAMD21_UF2_PACKAGE: BoardPackage = BoardPackage::new(
    SAMD21_PLATFORM_ID,
    SAMD21_UF2_ID,
    BootProfile::new(
        BootLayout::Custom,
        0x2000,
        248 * 1024,
        0x2000_0000,
        32 * 1024,
    ),
    SAMD21_BOARD_CAPACITY,
    SAMD21_BOARD_PINS,
);

pub const STM32F4_GENERIC_PACKAGE: BoardPackage = BoardPackage::new(
    STM32F4_PLATFORM_ID,
    STM32F4_GENERIC_ID,
    BootProfile::new(
        BootLayout::Custom,
        0x0800_0000,
        512 * 1024,
        0x2000_0000,
        128 * 1024,
    ),
    STM32F4_BOARD_CAPACITY,
    STM32F4_BOARD_PINS,
);

pub const TEENSY4_GENERIC_PACKAGE: BoardPackage = BoardPackage::new(
    IMXRT1062_PLATFORM_ID,
    TEENSY4_GENERIC_ID,
    BootProfile::new(
        BootLayout::Custom,
        0x6000_0000,
        2 * 1024 * 1024,
        0x2020_0000,
        512 * 1024,
    ),
    TEENSY4_BOARD_CAPACITY,
    TEENSY4_BOARD_PINS,
);

pub const CORTEX_M_GENERIC_PACKAGE: BoardPackage = BoardPackage::new(
    CORTEX_M_PLATFORM_ID,
    CORTEX_M_GENERIC_ID,
    BootProfile::new(
        BootLayout::Custom,
        0x0800_0000,
        128 * 1024,
        0x2000_0000,
        32 * 1024,
    ),
    CORTEX_M_BOARD_CAPACITY,
    CORTEX_M_BOARD_PINS,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardPackageDefinition {
    pub feature: &'static str,
    pub package: BoardPackage,
}

impl BoardPackageDefinition {
    pub fn report(self) -> BoardPackageReport {
        BoardPackageReport::from_package(&self.package)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardProfileDefinition {
    pub feature: &'static str,
    pub platform_id: &'static str,
    pub board_id: &'static str,
    pub app_flash_start: u32,
    pub capacity: BoardCapacity,
    pub pins: BoardPins,
    pub servo_center_us: u32,
}

impl BoardProfileDefinition {
    pub fn report(self) -> BoardProfileReport {
        BoardProfileReport::from_facts(
            self.platform_id,
            self.board_id,
            self.app_flash_start,
            self.capacity,
            self.pins,
            self.servo_center_us,
        )
    }
}

pub const BOARD_PACKAGES: [BoardPackageDefinition; 6] = [
    BoardPackageDefinition {
        feature: "board-promicro-nosd",
        package: PROMICRO_NRF52840_NOSD_PACKAGE,
    },
    BoardPackageDefinition {
        feature: "board-nicenano-s140",
        package: PROMICRO_NRF52840_S140_PACKAGE,
    },
    BoardPackageDefinition {
        feature: "board-samd21-uf2",
        package: SAMD21_UF2_PACKAGE,
    },
    BoardPackageDefinition {
        feature: "board-stm32f4-generic",
        package: STM32F4_GENERIC_PACKAGE,
    },
    BoardPackageDefinition {
        feature: "board-teensy4-generic",
        package: TEENSY4_GENERIC_PACKAGE,
    },
    BoardPackageDefinition {
        feature: "board-cortexm-generic",
        package: CORTEX_M_GENERIC_PACKAGE,
    },
];

pub const BOARD_PROFILES: [BoardProfileDefinition; 6] = [
    BoardProfileDefinition {
        feature: "board-promicro-nosd",
        platform_id: NRF52840_PLATFORM_ID,
        board_id: PROMICRO_NRF52840_NOSD_ID,
        app_flash_start: 0x1000,
        capacity: NRF52840_BOARD_CAPACITY,
        pins: NRF52840_BOARD_PINS,
        servo_center_us: NRF52840_SERVO_CENTER_US,
    },
    BoardProfileDefinition {
        feature: "board-nicenano-s140",
        platform_id: NRF52840_PLATFORM_ID,
        board_id: PROMICRO_NRF52840_S140_ID,
        app_flash_start: 0x26000,
        capacity: NRF52840_BOARD_CAPACITY,
        pins: NRF52840_BOARD_PINS,
        servo_center_us: NRF52840_SERVO_CENTER_US,
    },
    BoardProfileDefinition {
        feature: "board-samd21-uf2",
        platform_id: SAMD21_PLATFORM_ID,
        board_id: SAMD21_UF2_ID,
        app_flash_start: 0x2000,
        capacity: SAMD21_BOARD_CAPACITY,
        pins: SAMD21_BOARD_PINS,
        servo_center_us: NRF52840_SERVO_CENTER_US,
    },
    BoardProfileDefinition {
        feature: "board-stm32f4-generic",
        platform_id: STM32F4_PLATFORM_ID,
        board_id: STM32F4_GENERIC_ID,
        app_flash_start: 0x0800_0000,
        capacity: STM32F4_BOARD_CAPACITY,
        pins: STM32F4_BOARD_PINS,
        servo_center_us: NRF52840_SERVO_CENTER_US,
    },
    BoardProfileDefinition {
        feature: "board-teensy4-generic",
        platform_id: IMXRT1062_PLATFORM_ID,
        board_id: TEENSY4_GENERIC_ID,
        app_flash_start: 0x6000_0000,
        capacity: TEENSY4_BOARD_CAPACITY,
        pins: TEENSY4_BOARD_PINS,
        servo_center_us: NRF52840_SERVO_CENTER_US,
    },
    BoardProfileDefinition {
        feature: "board-cortexm-generic",
        platform_id: CORTEX_M_PLATFORM_ID,
        board_id: CORTEX_M_GENERIC_ID,
        app_flash_start: 0x0800_0000,
        capacity: CORTEX_M_BOARD_CAPACITY,
        pins: CORTEX_M_BOARD_PINS,
        servo_center_us: NRF52840_SERVO_CENTER_US,
    },
];

pub fn package_for_feature(feature: &str) -> Option<BoardPackageDefinition> {
    BOARD_PACKAGES
        .iter()
        .copied()
        .find(|entry| entry.feature == feature)
}

pub fn profile_for_feature(feature: &str) -> Option<BoardProfileDefinition> {
    BOARD_PROFILES
        .iter()
        .copied()
        .find(|entry| entry.feature == feature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_package_entries_are_valid_and_reportable() {
        for entry in BOARD_PACKAGES {
            assert_eq!(entry.package.validate(), Ok(()));
            let report = entry.report();
            assert!(report.verify_checksum());
            assert_eq!(report.valid, 1);
            assert_eq!(report.error_code, 0);
        }
    }

    #[test]
    fn board_profile_entries_are_reportable() {
        for entry in BOARD_PROFILES {
            let report = entry.report();
            assert!(report.verify_checksum());
            assert_eq!(report.completed, 1);
            assert_eq!(report.app_flash_start, entry.app_flash_start);
            assert_eq!(report.flash_budget_bytes, entry.capacity.flash_budget_bytes);
            assert_eq!(report.ram_budget_bytes, entry.capacity.ram_budget_bytes);
            assert_eq!(
                report.sample_pool_slots,
                u32::from(entry.capacity.sample_pool_slots)
            );
            assert_eq!(report.max_modules, entry.capacity.max_modules as u32);
            assert_eq!(report.servo_pin, u32::from(entry.pins.servo_pwm_pin));
            assert_eq!(report.servo_center_us, entry.servo_center_us);
            assert_eq!(report.led_pin, u32::from(entry.pins.led_pin));
            assert_eq!(
                report.mvk_trigger_pin,
                u32::from(entry.pins.mvk_trigger_pin)
            );
        }
    }

    #[test]
    fn board_profile_and_package_entries_share_identity_and_budget() {
        for package_entry in BOARD_PACKAGES {
            let profile_entry = profile_for_feature(package_entry.feature).expect("profile entry");

            assert_eq!(profile_entry.platform_id, package_entry.package.platform_id);
            assert_eq!(profile_entry.board_id, package_entry.package.board_id);
            assert_eq!(
                profile_entry.app_flash_start,
                package_entry.package.boot.app_flash_start
            );
            assert_eq!(profile_entry.capacity, package_entry.package.capacity);
            assert_eq!(profile_entry.pins, package_entry.package.pins);
        }
    }

    #[test]
    fn board_package_entries_cover_current_boot_layouts() {
        let nosd = package_for_feature("board-promicro-nosd").expect("nosd entry");
        let s140 = package_for_feature("board-nicenano-s140").expect("s140 entry");

        assert_eq!(nosd.package.boot.layout, BootLayout::NoSoftDevice);
        assert_eq!(nosd.package.boot.app_flash_start, 0x1000);
        assert_eq!(s140.package.boot.layout, BootLayout::SoftDeviceS140V6);
        assert_eq!(s140.package.boot.app_flash_start, 0x26000);
        assert_eq!(
            package_for_feature("board-samd21-uf2")
                .expect("samd21 entry")
                .package
                .boot
                .app_flash_start,
            0x2000
        );
        assert_eq!(package_for_feature("unknown-board"), None);
        assert_eq!(profile_for_feature("unknown-board"), None);
    }
}
