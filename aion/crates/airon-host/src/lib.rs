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
pub const HEALTH_REPORT_SYMBOL: &str = "AIRON_HEALTH_REPORT";
pub const HEALTH_REPORT_MAGIC: u32 = 0x4152_484C;

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

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HealthReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub module_tag: u32,
    pub total_errors: u32,
    pub consecutive_errors: u32,
    pub last_error: u32,
    pub last_action: u32,
    pub event_count: u32,
    pub dropped_events: u32,
    pub error_events: u32,
    pub fatal_events: u32,
    pub last_seen_us_lo: u32,
    pub last_seen_us_hi: u32,
    pub checksum: u32,
}

impl HealthReport {
    pub const VERSION: u32 = 1;

    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            module_tag: 0,
            total_errors: 0,
            consecutive_errors: 0,
            last_error: 0,
            last_action: 0,
            event_count: 0,
            dropped_events: 0,
            error_events: 0,
            fatal_events: 0,
            last_seen_us_lo: 0,
            last_seen_us_hi: 0,
            checksum: 0,
        }
    }

    pub fn set_last_seen_us(&mut self, last_seen_us: u64) {
        self.last_seen_us_lo = last_seen_us as u32;
        self.last_seen_us_hi = (last_seen_us >> 32) as u32;
    }

    pub fn last_seen_us(&self) -> u64 {
        (u64::from(self.last_seen_us_hi) << 32) | u64::from(self.last_seen_us_lo)
    }

    pub fn seal(&mut self) {
        self.magic = HEALTH_REPORT_MAGIC;
        self.version = Self::VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == HEALTH_REPORT_MAGIC
            && self.version == Self::VERSION
            && self.checksum == self.compute_checksum()
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.module_tag
            ^ self.total_errors
            ^ self.consecutive_errors
            ^ self.last_error
            ^ self.last_action
            ^ self.event_count
            ^ self.dropped_events
            ^ self.error_events
            ^ self.fatal_events
            ^ self.last_seen_us_lo
            ^ self.last_seen_us_hi
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
        assert_eq!(HEALTH_REPORT_SYMBOL, "AIRON_HEALTH_REPORT");
        assert_eq!(HEALTH_REPORT_MAGIC, 0x4152_484C);
    }

    #[test]
    fn user_and_maintenance_ports_are_separate() {
        assert_ne!(
            HostContract::maintenance_cdc_mi(),
            HostContract::user_cdc_mi()
        );
        assert_eq!(HostContract::upload_touch_baud(), 1200);
    }

    #[test]
    fn health_report_seals_and_verifies() {
        let mut report = HealthReport {
            module_tag: 4,
            total_errors: 7,
            consecutive_errors: 2,
            last_error: 3,
            last_action: 2,
            event_count: 12,
            dropped_events: 1,
            error_events: 2,
            fatal_events: 0,
            ..HealthReport::zeroed()
        };
        report.set_last_seen_us(0x1234_5678_9ABC_DEF0);
        report.seal();

        assert!(report.verify_checksum());
        assert_eq!(report.last_seen_us(), 0x1234_5678_9ABC_DEF0);

        report.total_errors += 1;
        assert!(!report.verify_checksum());
    }
}
