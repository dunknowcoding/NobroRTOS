//! Board-level constants portable across MCUs.

/// Static board description selected at compile time via Cargo features.
pub trait BoardDesc {
    /// SoC / HAL backend id, e.g. `"nrf52840"`, `"rp2040"`, `"esp32s3"`.
    const PLATFORM_ID: &'static str;
    /// Board package id, e.g. `"promicro_nrf52840_nosd"`.
    const BOARD_ID: &'static str;
    /// Application image flash origin (after bootloader / partition table).
    const APP_FLASH_START: u32;
    const LED_PIN: u8;
    const SERVO_PWM_PIN: u8;
    const SERVO_CENTER_US: u32;
    const MVK_TRIGGER_PIN: u8;
}

/// Expected servo PWM profile for self-test / Arduino parity (scene D).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServoProfile {
    pub frequency_hz: u32,
    pub center_pulse_us: u32,
    pub pin: u8,
}

impl ServoProfile {
    pub const fn new(frequency_hz: u32, center_pulse_us: u32, pin: u8) -> Self {
        Self {
            frequency_hz,
            center_pulse_us,
            pin,
        }
    }
}

/// Bus peripheral base addresses exposed for parity checks (platform-specific layout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BusLayout {
    pub twim0_base: u32,
    pub twim1_base: u32,
}
