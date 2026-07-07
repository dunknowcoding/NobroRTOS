//! Board contract fixtures for host-side review without board feature switching.

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
pub struct BoardPackageFixture {
    pub feature: &'static str,
    pub package: BoardPackage,
}

impl BoardPackageFixture {
    pub fn report(self) -> BoardPackageReport {
        BoardPackageReport::from_package(&self.package)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardProfileFixture {
    pub feature: &'static str,
    pub platform_id: &'static str,
    pub board_id: &'static str,
    pub app_flash_start: u32,
    pub capacity: BoardCapacity,
    pub pins: BoardPins,
    pub servo_center_us: u32,
}

impl BoardProfileFixture {
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

pub const BOARD_PACKAGE_FIXTURES: [BoardPackageFixture; 6] = [
    BoardPackageFixture {
        feature: "board-promicro-nosd",
        package: PROMICRO_NRF52840_NOSD_PACKAGE,
    },
    BoardPackageFixture {
        feature: "board-nicenano-s140",
        package: PROMICRO_NRF52840_S140_PACKAGE,
    },
    BoardPackageFixture {
        feature: "board-samd21-uf2",
        package: SAMD21_UF2_PACKAGE,
    },
    BoardPackageFixture {
        feature: "board-stm32f4-generic",
        package: STM32F4_GENERIC_PACKAGE,
    },
    BoardPackageFixture {
        feature: "board-teensy4-generic",
        package: TEENSY4_GENERIC_PACKAGE,
    },
    BoardPackageFixture {
        feature: "board-cortexm-generic",
        package: CORTEX_M_GENERIC_PACKAGE,
    },
];

pub const BOARD_PROFILE_FIXTURES: [BoardProfileFixture; 6] = [
    BoardProfileFixture {
        feature: "board-promicro-nosd",
        platform_id: NRF52840_PLATFORM_ID,
        board_id: PROMICRO_NRF52840_NOSD_ID,
        app_flash_start: 0x1000,
        capacity: NRF52840_BOARD_CAPACITY,
        pins: NRF52840_BOARD_PINS,
        servo_center_us: NRF52840_SERVO_CENTER_US,
    },
    BoardProfileFixture {
        feature: "board-nicenano-s140",
        platform_id: NRF52840_PLATFORM_ID,
        board_id: PROMICRO_NRF52840_S140_ID,
        app_flash_start: 0x26000,
        capacity: NRF52840_BOARD_CAPACITY,
        pins: NRF52840_BOARD_PINS,
        servo_center_us: NRF52840_SERVO_CENTER_US,
    },
    BoardProfileFixture {
        feature: "board-samd21-uf2",
        platform_id: SAMD21_PLATFORM_ID,
        board_id: SAMD21_UF2_ID,
        app_flash_start: 0x2000,
        capacity: SAMD21_BOARD_CAPACITY,
        pins: SAMD21_BOARD_PINS,
        servo_center_us: NRF52840_SERVO_CENTER_US,
    },
    BoardProfileFixture {
        feature: "board-stm32f4-generic",
        platform_id: STM32F4_PLATFORM_ID,
        board_id: STM32F4_GENERIC_ID,
        app_flash_start: 0x0800_0000,
        capacity: STM32F4_BOARD_CAPACITY,
        pins: STM32F4_BOARD_PINS,
        servo_center_us: NRF52840_SERVO_CENTER_US,
    },
    BoardProfileFixture {
        feature: "board-teensy4-generic",
        platform_id: IMXRT1062_PLATFORM_ID,
        board_id: TEENSY4_GENERIC_ID,
        app_flash_start: 0x6000_0000,
        capacity: TEENSY4_BOARD_CAPACITY,
        pins: TEENSY4_BOARD_PINS,
        servo_center_us: NRF52840_SERVO_CENTER_US,
    },
    BoardProfileFixture {
        feature: "board-cortexm-generic",
        platform_id: CORTEX_M_PLATFORM_ID,
        board_id: CORTEX_M_GENERIC_ID,
        app_flash_start: 0x0800_0000,
        capacity: CORTEX_M_BOARD_CAPACITY,
        pins: CORTEX_M_BOARD_PINS,
        servo_center_us: NRF52840_SERVO_CENTER_US,
    },
];

pub fn fixture_for_feature(feature: &str) -> Option<BoardPackageFixture> {
    BOARD_PACKAGE_FIXTURES
        .iter()
        .copied()
        .find(|fixture| fixture.feature == feature)
}

pub fn profile_fixture_for_feature(feature: &str) -> Option<BoardProfileFixture> {
    BOARD_PROFILE_FIXTURES
        .iter()
        .copied()
        .find(|fixture| fixture.feature == feature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_package_fixtures_are_valid_and_reportable() {
        for fixture in BOARD_PACKAGE_FIXTURES {
            assert_eq!(fixture.package.validate(), Ok(()));
            let report = fixture.report();
            assert!(report.verify_checksum());
            assert_eq!(report.valid, 1);
            assert_eq!(report.error_code, 0);
        }
    }

    #[test]
    fn board_profile_fixtures_are_reportable() {
        for fixture in BOARD_PROFILE_FIXTURES {
            let report = fixture.report();
            assert!(report.verify_checksum());
            assert_eq!(report.completed, 1);
            assert_eq!(report.app_flash_start, fixture.app_flash_start);
            assert_eq!(
                report.flash_budget_bytes,
                fixture.capacity.flash_budget_bytes
            );
            assert_eq!(report.ram_budget_bytes, fixture.capacity.ram_budget_bytes);
            assert_eq!(
                report.sample_pool_slots,
                u32::from(fixture.capacity.sample_pool_slots)
            );
            assert_eq!(report.max_modules, fixture.capacity.max_modules as u32);
            assert_eq!(report.servo_pin, u32::from(fixture.pins.servo_pwm_pin));
            assert_eq!(report.servo_center_us, fixture.servo_center_us);
            assert_eq!(report.led_pin, u32::from(fixture.pins.led_pin));
            assert_eq!(
                report.mvk_trigger_pin,
                u32::from(fixture.pins.mvk_trigger_pin)
            );
        }
    }

    #[test]
    fn board_profile_and_package_fixtures_share_identity_and_budget() {
        for package_fixture in BOARD_PACKAGE_FIXTURES {
            let profile_fixture =
                profile_fixture_for_feature(package_fixture.feature).expect("profile fixture");

            assert_eq!(
                profile_fixture.platform_id,
                package_fixture.package.platform_id
            );
            assert_eq!(profile_fixture.board_id, package_fixture.package.board_id);
            assert_eq!(
                profile_fixture.app_flash_start,
                package_fixture.package.boot.app_flash_start
            );
            assert_eq!(profile_fixture.capacity, package_fixture.package.capacity);
            assert_eq!(profile_fixture.pins, package_fixture.package.pins);
        }
    }

    #[test]
    fn board_package_fixtures_cover_current_boot_layouts() {
        let nosd = fixture_for_feature("board-promicro-nosd").expect("nosd fixture");
        let s140 = fixture_for_feature("board-nicenano-s140").expect("s140 fixture");

        assert_eq!(nosd.package.boot.layout, BootLayout::NoSoftDevice);
        assert_eq!(nosd.package.boot.app_flash_start, 0x1000);
        assert_eq!(s140.package.boot.layout, BootLayout::SoftDeviceS140V6);
        assert_eq!(s140.package.boot.app_flash_start, 0x26000);
        assert_eq!(
            fixture_for_feature("board-samd21-uf2")
                .expect("samd21 fixture")
                .package
                .boot
                .app_flash_start,
            0x2000
        );
        assert_eq!(fixture_for_feature("unknown-board"), None);
        assert_eq!(profile_fixture_for_feature("unknown-board"), None);
    }
}
