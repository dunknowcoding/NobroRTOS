//! Platform-agnostic self-test snapshot types.

use crate::board_desc::{BoardCapacity, BoardDesc, BusLayout, ServoProfile};

pub const BOARD_PROFILE_REPORT_MAGIC: u32 = 0x4E42_4250; // "NBBP"
pub const BOARD_PROFILE_REPORT_VERSION: u32 = 1;
const FNV1A32_OFFSET: u32 = 0x811C_9DC5;
const FNV1A32_PRIME: u32 = 0x0100_0193;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PwmSnapshot {
    pub enabled: bool,
    pub prescaler: u8,
    pub counter_top: u16,
    pub out_pin: u8,
    pub frequency_hz: u32,
    pub pulse_us: u32,
}

impl PwmSnapshot {
    pub fn matches_profile(&self, profile: &ServoProfile) -> bool {
        self.enabled
            && self.frequency_hz == profile.frequency_hz
            && self.out_pin == profile.pin
            && self.pulse_us == profile.center_pulse_us
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoardProfileReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub platform_hash: u32,
    pub board_hash: u32,
    pub app_flash_start: u32,
    pub flash_budget_bytes: u32,
    pub ram_budget_bytes: u32,
    pub sample_pool_slots: u32,
    pub max_modules: u32,
    pub servo_pin: u32,
    pub servo_center_us: u32,
    pub led_pin: u32,
    pub mvk_trigger_pin: u32,
    pub checksum: u32,
}

impl BoardProfileReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            platform_hash: 0,
            board_hash: 0,
            app_flash_start: 0,
            flash_budget_bytes: 0,
            ram_budget_bytes: 0,
            sample_pool_slots: 0,
            max_modules: 0,
            servo_pin: 0,
            servo_center_us: 0,
            led_pin: 0,
            mvk_trigger_pin: 0,
            checksum: 0,
        }
    }

    pub fn from_board<B: BoardDesc>() -> Self {
        let capacity = B::CAPACITY;
        let mut report = Self {
            platform_hash: hash_str(B::PLATFORM_ID),
            board_hash: hash_str(B::BOARD_ID),
            app_flash_start: B::APP_FLASH_START,
            flash_budget_bytes: capacity.flash_budget_bytes,
            ram_budget_bytes: capacity.ram_budget_bytes,
            sample_pool_slots: u32::from(capacity.sample_pool_slots),
            max_modules: capacity.max_modules as u32,
            servo_pin: u32::from(B::SERVO_PWM_PIN),
            servo_center_us: B::SERVO_CENTER_US,
            led_pin: u32::from(B::LED_PIN),
            mvk_trigger_pin: u32::from(B::MVK_TRIGGER_PIN),
            ..Self::zeroed()
        };
        report.seal();
        report
    }

    pub fn seal(&mut self) {
        self.magic = BOARD_PROFILE_REPORT_MAGIC;
        self.version = BOARD_PROFILE_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == BOARD_PROFILE_REPORT_MAGIC
            && self.version == BOARD_PROFILE_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.platform_hash
            ^ self.board_hash
            ^ self.app_flash_start
            ^ self.flash_budget_bytes
            ^ self.ram_budget_bytes
            ^ self.sample_pool_slots
            ^ self.max_modules
            ^ self.servo_pin
            ^ self.servo_center_us
            ^ self.led_pin
            ^ self.mvk_trigger_pin
    }
}

pub fn hash_str(input: &str) -> u32 {
    let mut hash = FNV1A32_OFFSET;
    for byte in input.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(FNV1A32_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestBoard;

    impl BoardDesc for TestBoard {
        const PLATFORM_ID: &'static str = "test-platform";
        const BOARD_ID: &'static str = "test-board";
        const APP_FLASH_START: u32 = 0x4000;
        const CAPACITY: BoardCapacity = BoardCapacity::new(64 * 1024, 16 * 1024, 8, 4);
        const LED_PIN: u8 = 1;
        const SERVO_PWM_PIN: u8 = 2;
        const SERVO_CENTER_US: u32 = 1500;
        const MVK_TRIGGER_PIN: u8 = 3;
    }

    #[test]
    fn board_profile_report_seals_board_contract() {
        let mut report = BoardProfileReport::from_board::<TestBoard>();

        assert!(report.verify_checksum());
        assert_eq!(report.magic, BOARD_PROFILE_REPORT_MAGIC);
        assert_eq!(report.platform_hash, hash_str("test-platform"));
        assert_eq!(report.board_hash, hash_str("test-board"));
        assert_eq!(report.app_flash_start, 0x4000);
        assert_eq!(report.flash_budget_bytes, 64 * 1024);
        assert_eq!(report.ram_budget_bytes, 16 * 1024);
        assert_eq!(report.sample_pool_slots, 8);
        assert_eq!(report.max_modules, 4);
        assert_eq!(report.servo_pin, 2);

        report.max_modules += 1;
        assert!(!report.verify_checksum());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventCaptureSnapshot {
    pub channel_enabled: bool,
    pub source_wired: bool,
    pub sink_wired: bool,
}

impl EventCaptureSnapshot {
    pub fn is_wired(&self) -> bool {
        self.channel_enabled && self.source_wired && self.sink_wired
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoardParity {
    pub flash_start: u32,
    pub capacity: BoardCapacity,
    pub bus: BusLayout,
    pub servo_pin: u8,
    pub servo_center_us: u32,
}

impl BoardParity {
    pub fn from_board<B: BoardDesc>(bus: BusLayout) -> Self {
        Self {
            flash_start: B::APP_FLASH_START,
            capacity: B::CAPACITY,
            bus,
            servo_pin: B::SERVO_PWM_PIN,
            servo_center_us: B::SERVO_CENTER_US,
        }
    }

    pub fn matches_board<B: BoardDesc>(&self, expected_bus: BusLayout) -> bool {
        self.flash_start == B::APP_FLASH_START
            && self.capacity == B::CAPACITY
            && self.bus.twim0_base == expected_bus.twim0_base
            && self.bus.twim1_base == expected_bus.twim1_base
            && self.servo_pin == B::SERVO_PWM_PIN
            && self.servo_center_us == B::SERVO_CENTER_US
    }
}
