//! nRF52840 register readback for Phase 1 self-test (scene D).

use crate::board;
use crate::board_desc::ServoProfile;
use crate::platform::nrf52840::Nrf52840;
use crate::pwm::{self, SERVO_PIN};
use crate::snapshots::{BoardParity, EventCaptureSnapshot, PwmSnapshot};
use crate::traits::HalSelfTest;

const PWM0_BASE: u32 = 0x4001_C000;
const PWM_ENABLE: u32 = 0x500;
const PWM_COUNTERTOP: u32 = 0x508;
const PWM_PRESCALER: u32 = 0x50C;
const PWM_PSEL_OUT0: u32 = 0x560;

const PPI_BASE: u32 = 0x4001_F000;
const PPI_CHEN: u32 = 0x500;
const PPI_CH0_EEP: u32 = 0x510;
const PPI_CH0_TEP: u32 = 0x514;

const TIMER0_BASE: u32 = 0x4000_8000;
const TIMER_TASKS_CAPTURE2: u32 = 0x048;

fn reg(base: u32, off: u32) -> u32 {
    unsafe { core::ptr::read_volatile((base + off) as *const u32) }
}

impl PwmSnapshot {
    pub unsafe fn capture() -> Self {
        let prescaler = reg(PWM0_BASE, PWM_PRESCALER) as u8;
        let counter_top = reg(PWM0_BASE, PWM_COUNTERTOP) as u16;
        let enabled = reg(PWM0_BASE, PWM_ENABLE) != 0;
        let out_pin = reg(PWM0_BASE, PWM_PSEL_OUT0) as u8;
        let clock = 16_000_000u32 >> prescaler;
        let frequency_hz = clock / (u32::from(counter_top) + 1);
        let pulse_us = pwm::PwmServo::read_pulse_us();
        Self {
            enabled,
            prescaler,
            counter_top,
            out_pin,
            frequency_hz,
            pulse_us,
        }
    }

    pub fn matches_arduino_nrf(&self, expected_pulse_us: u32) -> bool {
        self.enabled
            && self.prescaler == 4
            && self.counter_top == 19_999
            && self.frequency_hz == 50
            && self.out_pin == SERVO_PIN
            && self.pulse_us == expected_pulse_us
            && expected_pulse_us == board::SERVO_CENTER_US
    }
}

impl EventCaptureSnapshot {
    pub unsafe fn capture_radio_channel(ch: usize) -> Self {
        let chen = reg(PPI_BASE, PPI_CHEN);
        let channel_enabled = (chen & (1 << ch)) != 0;
        let eep = reg(PPI_BASE, PPI_CH0_EEP + (ch as u32 * 8));
        let tep = reg(PPI_BASE, PPI_CH0_TEP + (ch as u32 * 8));
        let expected_tep = TIMER0_BASE + TIMER_TASKS_CAPTURE2;
        Self {
            channel_enabled,
            source_wired: eep != 0,
            sink_wired: tep == expected_tep,
        }
    }
}

impl HalSelfTest<board::Board> for Nrf52840 {
    unsafe fn scene_d_pass(profile: ServoProfile) -> (bool, PwmSnapshot, BoardParity) {
        let pwm = PwmSnapshot::capture();
        let parity =
            BoardParity::from_board::<board::Board>(crate::platform::nrf52840::bus_layout());
        let capture = EventCaptureSnapshot::capture_radio_channel(1);
        let pwm_ok =
            pwm.matches_profile(&profile) && pwm.matches_arduino_nrf(profile.center_pulse_us);
        let parity_ok =
            parity.matches_board::<board::Board>(crate::platform::nrf52840::bus_layout());
        let pass = pwm_ok && parity_ok && capture.is_wired();
        (pass, pwm, parity)
    }
}

/// Scene D entry (legacy path for apps).
pub unsafe fn scene_d_pass(expected_pulse_us: u32) -> (bool, PwmSnapshot, BoardParity) {
    Nrf52840::scene_d_pass(ServoProfile::new(
        50,
        expected_pulse_us,
        board::SERVO_PWM_PIN,
    ))
}
