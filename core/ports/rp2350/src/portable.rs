//! Portable HAL provider contract on the RP2350.
//!
//! This implements the SAME portable `nobro_hal` provider traits the nRF52840
//! deep HAL implements: a real Cortex-M33 backend, not an nRF-shaped
//! placeholder. The foundational provider (a monotonic microsecond timebase)
//! is implemented here against the RP2350 TIMER0 block, so kernel code that is
//! generic over `HalClock`/`HalTimebaseProvider` runs unchanged on this chip.
//!
//! Scope: timebase and compatibility/identity providers are implemented. Deadline
//! capture, servo PWM, I2C/SPI, and lease providers remain unavailable on this port.

use nobro_hal::traits::{HalClock, HalCompatibility, HalTimebaseProvider, PlatformHal};
use nobro_hal::board_catalog::EXACT_RP2350_PICO2W;
use nobro_hal::{
    BoardCapacity, BoardDesc, CapabilityProfileKind, HardwareCapability,
    HardwareCapabilityDeclaration, HardwareCapabilitySet, HardwareCapabilityWitness,
};

/// RP2350 TIMER0 register block. Offsets from RP2350 datasheet section 12.8.
/// The timer increments once per microsecond (the watchdog tick divides the
/// system clock to 1 MHz during `init_clocks_and_plls`).
const TIMER0_BASE: usize = 0x400b_0000;
const TIMELR: usize = 0x0c; // low word; reading it LATCHES the high word
const TIMEHR: usize = 0x08; // high word, valid immediately after a TIMELR read
const TIMERAWL: usize = 0x28; // un-latched low word (liveness probe)

#[inline]
fn reg(offset: usize) -> *const u32 {
    (TIMER0_BASE + offset) as *const u32
}

/// The RP2350 portable platform backend.
pub struct Rp2350;

impl HardwareCapabilityWitness<{ HardwareCapability::Timebase as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::DmaCompletion as u8 }> for Rp2350 {}

/// Minimal board descriptor so `PlatformHal` has an associated `Board`.
pub struct Rp2350Board;

impl BoardDesc for Rp2350Board {
    const PLATFORM_ID: &'static str = EXACT_RP2350_PICO2W.platform_id;
    const BOARD_ID: &'static str = EXACT_RP2350_PICO2W.board_id;
    // App image runs from XIP flash after the boot2 + image-def block.
    const APP_FLASH_START: u32 = match EXACT_RP2350_PICO2W.app_flash_start {
        Some(start) => start,
        None => 0,
    };
    // 520 KiB SRAM / 4 MiB flash: a generous share for the software budget.
    const CAPACITY: BoardCapacity = EXACT_RP2350_PICO2W.capacity;
    // Pico 2 W routes its LED through the wireless device rather than a direct
    // RP2350 GPIO, so the board-level pin is intentionally absent.
    const LED_PIN: Option<u8> = EXACT_RP2350_PICO2W.pins.led_pin;
    const SERVO_PWM_PIN: Option<u8> = EXACT_RP2350_PICO2W.pins.servo_pwm_pin;
    const SERVO_CENTER_US: u32 = 1500;
    const MVK_TRIGGER_PIN: Option<u8> = EXACT_RP2350_PICO2W.pins.mvk_trigger_pin;
}

impl HalClock for Rp2350 {
    fn now_us() -> u64 {
        // Latched 64-bit read: reading TIMELR copies the current high word into
        // TIMEHR, so the pair is consistent without a retry loop (datasheet
        // section 12.8.2, "Reading the counter").
        // SAFETY: read-only access to fixed, always-mapped timer MMIO.
        unsafe {
            let lo = core::ptr::read_volatile(reg(TIMELR));
            let hi = core::ptr::read_volatile(reg(TIMEHR));
            (u64::from(hi) << 32) | u64::from(lo)
        }
    }
}

impl HalTimebaseProvider for Rp2350 {
    /// # Safety
    /// The RP2350 timer is started by `init_clocks_and_plls` before this
    /// backend is used; nothing to initialize here (kept for contract parity).
    unsafe fn init_timebase() {}
}

impl HalCompatibility for Rp2350 {
    const DECLARATION: HardwareCapabilityDeclaration = {
        let witnesses = HardwareCapabilitySet::EMPTY
            .witnessed::<Self, { HardwareCapability::Timebase as u8 }>(
                HardwareCapability::Timebase,
            )
            .witnessed::<Self, { HardwareCapability::DmaCompletion as u8 }>(
                HardwareCapability::DmaCompletion,
            );
        let supported = witnesses;
        HardwareCapabilityDeclaration::new(
            "provider-rp2-v2",
            CapabilityProfileKind::Constrained,
            supported,
            supported,
            HardwareCapabilitySet::EMPTY,
            HardwareCapabilitySet::ALL.without(supported),
            witnesses,
        )
    };
}

const _: [(); 1] = [(); <Rp2350 as HalCompatibility>::DECLARATION.is_valid() as usize];
const _: [(); 1] =
    [(); <Rp2350 as HalCompatibility>::DECLARATION.is_exact_profile() as usize];

impl PlatformHal for Rp2350 {
    const PLATFORM_ID: &'static str = "rp2350";
    type Board = Rp2350Board;
}

/// Live self-check of the portable timebase provider: the monotonic clock must
/// be advancing. Returns true when a second sample is strictly greater than the
/// first across a short busy wait, rejecting a constant-returning stub.
pub fn verify_timebase_provider() -> bool {
    let t0 = Rp2350::now_us();
    // Busy-wait on the un-latched raw low word so the compiler cannot elide it.
    // SAFETY: read-only timer MMIO.
    let start_raw = unsafe { core::ptr::read_volatile(reg(TIMERAWL)) };
    while unsafe { core::ptr::read_volatile(reg(TIMERAWL)) }.wrapping_sub(start_raw) < 50 {
        core::hint::spin_loop();
    }
    let t1 = Rp2350::now_us();
    let required = HardwareCapabilitySet::EMPTY.with(HardwareCapability::Timebase);
    t1 > t0 && <Rp2350 as HalCompatibility>::supports(required)
}
