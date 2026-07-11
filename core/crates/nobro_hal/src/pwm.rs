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
    ///
    /// # Safety
    /// Caller must own the PWM0 lease; `pin` must be the board's wired servo pin.
    /// Writes the static PWM_SEQ buffer PWM0 DMA-reads - exactly one PWM owner at a time.
    pub unsafe fn init_50hz(pin: u8, pulse_us: u32) -> Self {
        let prescaler: u8 = 4;
        let counter_top: u16 = 19_999; // 20 ms period at 1 MHz
        let duty = pulse_us.min(u32::from(counter_top)) as u16;

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
            pulse_us: u32::from(duty),
        }
    }

    pub fn frequency_hz(&self) -> u32 {
        let clock = PWM_BASE_CLOCK_HZ >> self.prescaler;
        clock / (u32::from(self.counter_top) + 1)
    }

    /// # Safety
    /// Requires the PWM0 this instance initialised to still be enabled; rewrites the
    /// DMA-read sequence buffer in place.
    pub unsafe fn set_pulse_us(&mut self, pulse_us: u32) {
        let duty = pulse_us.min(u32::from(self.counter_top)) as u16;
        self.pulse_us = u32::from(duty);
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
    ///
    /// # Safety
    /// PWM0 must have been initialised by [`PwmServo::init_50hz`] (reads live
    /// COUNTERTOP and rewrites the DMA sequence buffer).
    pub unsafe fn set_active_pulse_us(pulse_us: u32) {
        let top = *reg(PWM0_BASE, PWM_COUNTERTOP) as u16;
        let duty = pulse_us.min(u32::from(top)) as u16;
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

const PWM_PSEL_OUT: [u32; 4] = [0x560, 0x564, 0x568, 0x56C];
const PIN_DISCONNECTED: u32 = 0x8000_0000;

static mut PWM_BANK_SEQ: [u16; 4] = [0; 4];

/// Up to four 50 Hz servo/ESC outputs on PWM0's four channels (one shared 20 ms period,
/// per-channel duty via the individual-load decoder). Callers hold `Resource::Pwm0`.
pub struct PwmBank {
    counter_top: u16,
}

impl PwmBank {
    /// Bring up PWM0 at 50 Hz with one output pin per channel (`None` = channel unused).
    ///
    /// # Safety
    /// Caller must own the PWM0 lease; pins must be wired outputs. Mutually exclusive
    /// with [`PwmServo`] - both drive PWM0 and its static sequence memory.
    pub unsafe fn init_50hz(pins: [Option<u8>; 4], center_us: u32) -> Self {
        let counter_top: u16 = 19_999; // 20 ms at 1 MHz (prescaler 4)
                                       // raw-pointer write: no &mut to the static (2024 static_mut_refs rule); the
                                       // Safety contract above guarantees exclusive PWM0 ownership.
        let seq = ptr::addr_of_mut!(PWM_BANK_SEQ);
        for i in 0..4 {
            (*seq)[i] = pwm_seq_value(center_us.min(u32::from(counter_top)) as u16, counter_top);
        }
        *reg(PWM0_BASE, PWM_ENABLE) = 0;
        *reg(PWM0_BASE, PWM_MODE) = 0;
        *reg(PWM0_BASE, PWM_PRESCALER) = 4;
        *reg(PWM0_BASE, PWM_COUNTERTOP) = counter_top as u32;
        *reg(PWM0_BASE, PWM_DECODER) = 2; // individual load: SEQ[i] -> channel i
        *reg(PWM0_BASE, PWM_SEQ0_PTR) = ptr::addr_of!(PWM_BANK_SEQ) as u32;
        *reg(PWM0_BASE, PWM_SEQ0_CNT) = 4;
        for (ch, pin) in pins.iter().enumerate() {
            *reg(PWM0_BASE, PWM_PSEL_OUT[ch]) = match pin {
                Some(p) => u32::from(*p),
                None => PIN_DISCONNECTED,
            };
        }
        *reg(PWM0_BASE, PWM_ENABLE) = 1;
        *reg(PWM0_BASE, PWM_TASKS_SEQSTART0) = 1;
        Self { counter_top }
    }

    /// # Safety
    /// Requires the PWM0 this bank initialised to still be enabled; rewrites the
    /// channel's slot in the DMA-read sequence buffer.
    pub unsafe fn set_pulse_us(&mut self, channel: usize, pulse_us: u32) {
        if channel < 4 {
            let duty = pulse_us.min(u32::from(self.counter_top)) as u16;
            PWM_BANK_SEQ[channel] = pwm_seq_value(duty, self.counter_top);
            *reg(PWM0_BASE, PWM_TASKS_SEQSTART0) = 1;
        }
    }

    /// Decode a channel's live duty back out of the SEQ RAM the hardware is fetching.
    pub fn read_pulse_us(channel: usize) -> u32 {
        if channel >= 4 {
            return 0;
        }
        unsafe {
            let top = *reg(PWM0_BASE, PWM_COUNTERTOP) as u16;
            let raw = PWM_BANK_SEQ[channel] & 0x7FFF;
            top.saturating_sub(raw) as u32
        }
    }

    pub fn frequency_hz(&self) -> u32 {
        (PWM_BASE_CLOCK_HZ >> 4) / (u32::from(self.counter_top) + 1)
    }
}
