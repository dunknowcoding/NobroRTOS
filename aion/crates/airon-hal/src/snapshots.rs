//! Platform-agnostic self-test snapshot types.

use crate::board_desc::{BoardCapacity, BoardDesc, BusLayout, ServoProfile};

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
