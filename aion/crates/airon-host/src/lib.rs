//! Host-side contract constants shared by scripts, tools, and documentation.

#![no_std]

pub const MAINTENANCE_CDC_MI: &str = "MI_00";
pub const USER_CDC_MI: &str = "MI_02";
pub const UPLOAD_TOUCH_BAUD: u32 = 1200;

pub const APP_START_NO_SOFTDEVICE: u32 = 0x1000;
pub const APP_START_S140_V6: u32 = 0x26000;

pub const PHASE1_EVAL_SYMBOL: &str = "AIRON_EVAL_REPORT";
pub const PHASE1_EVAL_MAGIC: u32 = 0x4152_4E31;
pub const PHASE2_EVAL_SYMBOL: &str = "AIRON_SAL_EVAL_REPORT";
pub const PHASE2_EVAL_MAGIC: u32 = 0x4152_4E32;

pub const MAX_PHASE1_JITTER_US: u32 = 10;
pub const MIN_PHASE1_DEADLINE_TICKS: u32 = 150;
pub const MIN_PHASE1_I2C_READS: u32 = 10;
pub const MAX_PHASE1_RADIO_LATENCY_US: u32 = 10;
pub const MIN_PHASE1_RADIO_SAMPLES: u32 = 16;

pub const MIN_PHASE2_SERVO_STEPS: u32 = 20;
pub const MIN_PHASE2_IMU_SAMPLES: u32 = 3;
pub const PHASE2_SERVO_READBACK_TOL_US: u32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootLayout {
    NoSoftDevice,
    SoftDeviceS140V6,
}

impl BootLayout {
    pub const fn app_start(self) -> u32 {
        match self {
            Self::NoSoftDevice => APP_START_NO_SOFTDEVICE,
            Self::SoftDeviceS140V6 => APP_START_S140_V6,
        }
    }

    pub const fn cargo_feature(self) -> &'static str {
        match self {
            Self::NoSoftDevice => "board-promicro-nosd",
            Self::SoftDeviceS140V6 => "board-nicenano-s140",
        }
    }
}

pub struct HostContract;

impl HostContract {
    pub const fn maintenance_cdc_mi() -> &'static str {
        MAINTENANCE_CDC_MI
    }

    pub const fn user_cdc_mi() -> &'static str {
        USER_CDC_MI
    }

    pub const fn upload_touch_baud() -> u32 {
        UPLOAD_TOUCH_BAUD
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_layouts_match_arduinonrf_policy() {
        assert_eq!(BootLayout::NoSoftDevice.app_start(), 0x1000);
        assert_eq!(BootLayout::SoftDeviceS140V6.app_start(), 0x26000);
        assert_eq!(
            BootLayout::NoSoftDevice.cargo_feature(),
            "board-promicro-nosd"
        );
        assert_eq!(
            BootLayout::SoftDeviceS140V6.cargo_feature(),
            "board-nicenano-s140"
        );
    }

    #[test]
    fn eval_contracts_are_stable() {
        assert_eq!(PHASE1_EVAL_SYMBOL, "AIRON_EVAL_REPORT");
        assert_eq!(PHASE1_EVAL_MAGIC, 0x4152_4E31);
        assert_eq!(PHASE2_EVAL_SYMBOL, "AIRON_SAL_EVAL_REPORT");
        assert_eq!(PHASE2_EVAL_MAGIC, 0x4152_4E32);
    }

    #[test]
    fn user_and_maintenance_ports_are_separate() {
        assert_ne!(
            HostContract::maintenance_cdc_mi(),
            HostContract::user_cdc_mi()
        );
        assert_eq!(HostContract::upload_touch_baud(), 1200);
    }
}
