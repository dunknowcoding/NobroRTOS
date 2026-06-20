//! Board package fixtures for host-side review without board feature switching.

use crate::{
    board_desc::{BoardCapacity, BoardPackage, BoardPins, BootLayout, BootProfile},
    snapshots::BoardPackageReport,
};

pub const NRF52840_PLATFORM_ID: &str = "nrf52840";
pub const PROMICRO_NRF52840_NOSD_ID: &str = "promicro_nrf52840_nosd";
pub const PROMICRO_NRF52840_S140_ID: &str = "promicro_nrf52840_s140";

pub const NRF52840_BOARD_CAPACITY: BoardCapacity = BoardCapacity::new(80 * 1024, 32 * 1024, 8, 16);
pub const NRF52840_BOARD_PINS: BoardPins = BoardPins::new(15, 24, 17);

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

pub const BOARD_PACKAGE_FIXTURES: [BoardPackageFixture; 2] = [
    BoardPackageFixture {
        feature: "board-promicro-nosd",
        package: PROMICRO_NRF52840_NOSD_PACKAGE,
    },
    BoardPackageFixture {
        feature: "board-nicenano-s140",
        package: PROMICRO_NRF52840_S140_PACKAGE,
    },
];

pub fn fixture_for_feature(feature: &str) -> Option<BoardPackageFixture> {
    BOARD_PACKAGE_FIXTURES
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
    fn board_package_fixtures_cover_current_boot_layouts() {
        let nosd = fixture_for_feature("board-promicro-nosd").expect("nosd fixture");
        let s140 = fixture_for_feature("board-nicenano-s140").expect("s140 fixture");

        assert_eq!(nosd.package.boot.layout, BootLayout::NoSoftDevice);
        assert_eq!(nosd.package.boot.app_flash_start, 0x1000);
        assert_eq!(s140.package.boot.layout, BootLayout::SoftDeviceS140V6);
        assert_eq!(s140.package.boot.app_flash_start, 0x26000);
        assert_eq!(fixture_for_feature("unknown-board"), None);
    }
}
