//! Portable HAL provider contract on the ESP32-C3.
//!
//! Implements the same portable `nobro_hal` provider traits as the nRF52840
//! deep HAL: a real RISC-V (rv32imc) backend, not an nRF-shaped placeholder.
//! The foundational timebase provider is implemented against esp-hal's tested
//! systimer (`esp_hal::time::now()`, a 1 MHz monotonic instant), so kernel
//! code generic over `HalClock`/`HalTimebaseProvider` runs unchanged here.
//!
//! Scope: timebase, USB, and compatibility/identity providers are implemented.
//! Deadline, event, PWM, I2C/SPI, and lease providers remain unavailable on this port.

use nobro_hal::traits::{HalClock, HalCompatibility, HalTimebaseProvider, PlatformHal};
use nobro_hal::board_catalog::EXACT_ESP32C3_SUPERMINI;
use nobro_hal::{
    BoardCapacity, BoardDesc, CapabilityProfileKind, HardwareCapability,
    HardwareCapabilityDeclaration, HardwareCapabilitySet, HardwareCapabilityWitness,
};

/// The ESP32-C3 portable platform backend.
pub struct Esp32C3;

impl HardwareCapabilityWitness<{ HardwareCapability::Timebase as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Usb as u8 }> for Esp32C3 {}

/// Minimal board descriptor so `PlatformHal` has an associated `Board`.
pub struct Esp32C3Board;

impl BoardDesc for Esp32C3Board {
    const PLATFORM_ID: &'static str = EXACT_ESP32C3_SUPERMINI.platform_id;
    const BOARD_ID: &'static str = EXACT_ESP32C3_SUPERMINI.board_id;
    // App image runs from memory-mapped flash after the 2nd-stage bootloader.
    const APP_FLASH_START: u32 = match EXACT_ESP32C3_SUPERMINI.app_flash_start {
        Some(start) => start,
        None => 0,
    };
    // 400 KiB SRAM / 4 MiB flash: a conservative share for the software budget.
    const CAPACITY: BoardCapacity = EXACT_ESP32C3_SUPERMINI.capacity;
    const LED_PIN: Option<u8> = EXACT_ESP32C3_SUPERMINI.pins.led_pin;
    const SERVO_PWM_PIN: Option<u8> = EXACT_ESP32C3_SUPERMINI.pins.servo_pwm_pin;
    const SERVO_CENTER_US: u32 = 1500;
    const MVK_TRIGGER_PIN: Option<u8> = EXACT_ESP32C3_SUPERMINI.pins.mvk_trigger_pin;
}

impl HalClock for Esp32C3 {
    fn now_us() -> u64 {
        // esp-hal's systimer instant is already a 1 MHz monotonic microsecond
        // clock; use the vendor-tested path rather than hand-coded MMIO.
        esp_hal::time::now().duration_since_epoch().to_micros()
    }
}

impl HalTimebaseProvider for Esp32C3 {
    /// # Safety
    /// esp-hal starts the systimer during `esp_hal::init`; nothing to do here
    /// (kept for contract parity with backends that own their timer).
    unsafe fn init_timebase() {}
}

impl HalCompatibility for Esp32C3 {
    const DECLARATION: HardwareCapabilityDeclaration = {
        let witnesses = HardwareCapabilitySet::EMPTY
            .witnessed::<Self, { HardwareCapability::Timebase as u8 }>(
                HardwareCapability::Timebase,
            )
            .witnessed::<Self, { HardwareCapability::Usb as u8 }>(HardwareCapability::Usb);
        let supported = witnesses;
        HardwareCapabilityDeclaration::new(
            "provider-esp32c3-v2",
            CapabilityProfileKind::Constrained,
            supported,
            supported,
            HardwareCapabilitySet::EMPTY,
            HardwareCapabilitySet::ALL.without(supported),
            witnesses,
        )
    };
}

const _: [(); 1] =
    [(); <Esp32C3 as HalCompatibility>::DECLARATION.is_valid() as usize];
const _: [(); 1] =
    [(); <Esp32C3 as HalCompatibility>::DECLARATION.is_exact_profile() as usize];

impl PlatformHal for Esp32C3 {
    const PLATFORM_ID: &'static str = "esp32c3";
    type Board = Esp32C3Board;
}

/// Live self-check of the portable timebase provider: the monotonic clock must
/// advance across a short busy wait and satisfy the compatibility contract.
pub fn verify_timebase_provider() -> bool {
    let t0 = Esp32C3::now_us();
    while Esp32C3::now_us().wrapping_sub(t0) < 50 {
        core::hint::spin_loop();
    }
    let t1 = Esp32C3::now_us();
    let required = HardwareCapabilitySet::EMPTY.with(HardwareCapability::Timebase);
    t1 > t0 && <Esp32C3 as HalCompatibility>::supports(required)
}
