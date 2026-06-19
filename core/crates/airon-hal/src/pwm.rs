//! nRF52840 PWM for 50 Hz servo-style output aligned with ArduinoNRF wiring constants.

use core::ptr;

use crate::board;

const PWM0_BASE: u32 = 0x4001C000;
const PWM_TASKS_SEQSTART0: u32 = 0x008;
const PWM_ENABLE: u32 = 0x500;
const PWM_MODE: u32 = 0x504;
const PWM_COUNTERTOP: u32 = 0x508;
const PWM_PRESCALER: u32 = 0x50C;
const PWM_DECODER: u32 = 0x510;
const PWM_SEQ0_PTR: u32 = 0x520;
const PWM_SEQ0_CNT: u32 = 0x524;
const PWM_PSEL_OUT0: u32 = 0x560;

const PWM_BASE_CLOCK_HZ: u32 = 16_000_000;
const PWM_POLARITY_INVERTED: u16 = 0x8000;

/// Demo servo pin: D5 / P0.24 (Arduino pin index 5).
pub const SERVO_PIN: u8 = 24;

static mut PWM_SEQ: [u16; 4] = [0; 4];

fn reg(base: u32, off: u32) -> *mut u32 {
    (base + off) as *mut u32
}

fn pwm_seq_value(duty: u16, top: u16) -> u16 {
    let duty = duty.min(top);
    (top - duty) | PWM_POLARITY_INVERTED
}

pub struct PwmServo {
    counter_top: u16,
    prescaler: u8,
    pulse_us: u32,
}

impl PwmServo {
    /// 50 Hz at prescaler 4 (1 MHz tick), around 1500 us center pulse.
    pub unsafe fn init_50hz(pin: u8, pulse_us: u32) -> Self {
        let prescaler: u8 = 4;
        let counter_top: u16 = 19_999; // 20 ms period at 1 MHz
        let duty = (pulse_us as u16).min(counter_top);

        PWM_SEQ[0] = pwm_seq_value(duty, counter_top);
        PWM_SEQ[1] = 0;
        PWM_SEQ[2] = 0;
        PWM_SEQ[3] = 0;

        *reg(PWM0_BASE, PWM_ENABLE) = 0;
        *reg(PWM0_BASE, PWM_MODE) = 0; // up
        *reg(PWM0_BASE, PWM_PRESCALER) = prescaler as u32;
        *reg(PWM0_BASE, PWM_COUNTERTOP) = counter_top as u32;
        *reg(PWM0_BASE, PWM_DECODER) = 2; // individual load
        *reg(PWM0_BASE, PWM_SEQ0_PTR) = ptr::addr_of!(PWM_SEQ) as u32;
        *reg(PWM0_BASE, PWM_SEQ0_CNT) = 1;
        *reg(PWM0_BASE, PWM_PSEL_OUT0) = pin as u32;

        *reg(PWM0_BASE, PWM_ENABLE) = 1;
        *reg(PWM0_BASE, PWM_TASKS_SEQSTART0) = 1;

        Self {
            counter_top,
            prescaler,
            pulse_us,
        }
    }

    pub fn frequency_hz(&self) -> u32 {
        let clock = PWM_BASE_CLOCK_HZ >> self.prescaler;
        clock / (u32::from(self.counter_top) + 1)
    }

    pub unsafe fn set_pulse_us(&mut self, pulse_us: u32) {
        self.pulse_us = pulse_us;
        let duty = (pulse_us as u16).min(self.counter_top);
        PWM_SEQ[0] = pwm_seq_value(duty, self.counter_top);
        *reg(PWM0_BASE, PWM_TASKS_SEQSTART0) = 1;
    }

    pub fn pulse_us(&self) -> u32 {
        self.pulse_us
    }

    /// Decode live SEQ[0] duty in microseconds for self-test readback.
    pub fn read_pulse_us() -> u32 {
        unsafe {
            let top = *reg(PWM0_BASE, PWM_COUNTERTOP) as u16;
            let raw = PWM_SEQ[0] & 0x7FFF;
            top.saturating_sub(raw) as u32
        }
    }
    /// Update pulse on an already-initialised PWM0 servo output.
    pub unsafe fn set_active_pulse_us(pulse_us: u32) {
        let top = *reg(PWM0_BASE, PWM_COUNTERTOP) as u16;
        let duty = (pulse_us as u16).min(top);
        PWM_SEQ[0] = pwm_seq_value(duty, top);
        *reg(PWM0_BASE, PWM_TASKS_SEQSTART0) = 1;
    }
}

impl Default for PwmServo {
    fn default() -> Self {
        Self {
            counter_top: 19_999,
            prescaler: 4,
            pulse_us: board::SERVO_CENTER_US,
        }
    }
}
