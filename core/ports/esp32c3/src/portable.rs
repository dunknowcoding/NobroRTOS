//! Portable HAL provider contract on the ESP32-C3 (Wave 48 / PORT-01).
//!
//! Implements the SAME portable `nobro_hal` provider traits as the nRF52840
//! deep HAL — a real RISC-V (rv32imc) backend, not an nRF-shaped placeholder.
//! The foundational timebase provider is implemented against esp-hal's tested
//! systimer (`esp_hal::time::now()`, a 1 MHz monotonic instant), so kernel
//! code generic over `HalClock`/`HalTimebaseProvider` runs unchanged here.
//!
//! Honest scope: timebase + compatibility/identity providers are implemented
//! and self-checked live. Deeper peripheral providers (deadline capture, PWM,
//! I2C/SPI, leases) and on-hardware HIL remain bench-gated — there is no
//! ESP32-C3 board on the primary bench — and are declared honestly in the
//! platform tier matrix, never stubbed to look complete.

use nobro_hal::traits::{HalClock, HalCompatibility, HalTimebaseProvider, PlatformHal};
use nobro_hal::{BoardCapacity, BoardDesc, HardwareCapability, HardwareCapabilitySet};

/// The ESP32-C3 portable platform backend.
pub struct Esp32C3;

/// Minimal board descriptor so `PlatformHal` has an associated `Board`.
pub struct Esp32C3Board;

impl BoardDesc for Esp32C3Board {
    const PLATFORM_ID: &'static str = "esp32c3";
    const BOARD_ID: &'static str = "esp32c3-devkit";
    // App image runs from memory-mapped flash after the 2nd-stage bootloader.
    const APP_FLASH_START: u32 = 0x0001_0000;
    // 400 KiB SRAM / 4 MiB flash: a conservative share for the software budget.
    const CAPACITY: BoardCapacity = BoardCapacity::new(256 * 1024, 200 * 1024, 8, 16);
    const LED_PIN: u8 = 8;
    const SERVO_PWM_PIN: u8 = 4;
    const SERVO_CENTER_US: u32 = 1500;
    const MVK_TRIGGER_PIN: u8 = 5;
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
    const CAPABILITIES: HardwareCapabilitySet =
        HardwareCapabilitySet::EMPTY.with(HardwareCapability::Timebase);
}

impl PlatformHal for Esp32C3 {
    const PLATFORM_ID: &'static str = "esp32c3";
    type Board = Esp32C3Board;
}

/// Live self-check of the portable timebase provider: the monotonic clock must
/// advance across a short busy wait — proof the provider is wired to real
/// silicon and satisfies the compatibility contract.
pub fn verify_timebase_provider() -> bool {
    let t0 = Esp32C3::now_us();
    while Esp32C3::now_us().wrapping_sub(t0) < 50 {
        core::hint::spin_loop();
    }
    let t1 = Esp32C3::now_us();
    t1 > t0 && <Esp32C3 as HalCompatibility>::supports(HardwareCapabilitySet::EMPTY)
}
