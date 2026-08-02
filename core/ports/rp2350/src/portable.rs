//! Exact RP2350/Pico 2 W binding for the shared RP-series contract.

use nobro_hal::board_catalog::EXACT_RP2350_PICO2W;
use nobro_hal::{
    BoardCapacity, BoardDesc, CapabilityProfileKind, HalClock, HalCompatibility,
    HalTimebaseProvider, HardwareCapability, HardwareCapabilityDeclaration, HardwareCapabilitySet,
    HardwareCapabilityWitness, PlatformHal, Rp2Cyw43Backend, Rp2PowerBackend, Rp2ResetBackend,
    RP2350_RUNTIME,
};

#[cfg(all(feature = "cyw43-pio", feature = "cyw43-vendor"))]
compile_error!("select exactly one Pico 2 W CYW43439 backend");
#[cfg(not(any(feature = "cyw43-pio", feature = "cyw43-vendor")))]
compile_error!("select exactly one Pico 2 W CYW43439 backend");

#[cfg(feature = "cyw43-pio")]
pub const CYW43439_BACKEND: Rp2Cyw43Backend = Rp2Cyw43Backend::PioSpi;
#[cfg(feature = "cyw43-vendor")]
pub const CYW43439_BACKEND: Rp2Cyw43Backend = Rp2Cyw43Backend::Vendor;

const TIMER0_BASE: usize = 0x400b_0000;
const TIMEHR: usize = 0x08;
const TIMELR: usize = 0x0c;
const TIMERAWL: usize = 0x28;

#[inline]
fn timer_reg(offset: usize) -> *const u32 {
    (TIMER0_BASE + offset) as *const u32
}

pub struct Rp2350;

pub struct Rp2350Power;

impl Rp2PowerBackend for Rp2350Power {
    type Error = core::convert::Infallible;

    fn cpu_sleep(&mut self) -> Result<(), Self::Error> {
        cortex_m::asm::dsb();
        cortex_m::asm::wfi();
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rp2350ResetCause {
    Watchdog,
    Software,
    PowerOnExternalOrDebugger,
}

pub struct Rp2350Reset;

impl Rp2ResetBackend for Rp2350Reset {
    type Cause = Rp2350ResetCause;

    fn reset_cause() -> Self::Cause {
        let reason = unsafe { &*rp235x_hal::pac::WATCHDOG::PTR }.reason().read();
        if reason.timer().bit_is_set() {
            Self::Cause::Watchdog
        } else if reason.force().bit_is_set() {
            Self::Cause::Software
        } else {
            Self::Cause::PowerOnExternalOrDebugger
        }
    }

    fn system_reset() -> ! {
        rp235x_hal::reset()
    }
}

impl HardwareCapabilityWitness<{ HardwareCapability::Timebase as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Deadline as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Event as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::DmaCompletion as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Gpio as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Irq as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Uart as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::ByteIo as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Adc as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pwm as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::I2c as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Spi as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Usb as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Reset as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Power as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Multicore as u8 }> for Rp2350 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Lease as u8 }> for Rp2350 {}

pub struct Pico2WBoard;

impl BoardDesc for Pico2WBoard {
    const PLATFORM_ID: &'static str = EXACT_RP2350_PICO2W.platform_id;
    const BOARD_ID: &'static str = EXACT_RP2350_PICO2W.board_id;
    const APP_FLASH_START: u32 = match EXACT_RP2350_PICO2W.app_flash_start {
        Some(start) => start,
        None => 0,
    };
    const CAPACITY: BoardCapacity = EXACT_RP2350_PICO2W.capacity;
    const LED_PIN: Option<u8> = EXACT_RP2350_PICO2W.pins.led_pin;
    const SERVO_PWM_PIN: Option<u8> = EXACT_RP2350_PICO2W.pins.servo_pwm_pin;
    const SERVO_CENTER_US: u32 = 1_500;
    const MVK_TRIGGER_PIN: Option<u8> = EXACT_RP2350_PICO2W.pins.mvk_trigger_pin;
}

impl HalClock for Rp2350 {
    fn now_us() -> u64 {
        unsafe {
            let low = timer_reg(TIMELR).read_volatile();
            let high = timer_reg(TIMEHR).read_volatile();
            (u64::from(high) << 32) | u64::from(low)
        }
    }
}

impl HalTimebaseProvider for Rp2350 {
    unsafe fn init_timebase() {}
}

impl HalCompatibility for Rp2350 {
    const DECLARATION: HardwareCapabilityDeclaration = {
        let supported = HardwareCapabilitySet::EMPTY
            .witnessed::<Self, { HardwareCapability::Timebase as u8 }>(HardwareCapability::Timebase)
            .witnessed::<Self, { HardwareCapability::Deadline as u8 }>(HardwareCapability::Deadline)
            .witnessed::<Self, { HardwareCapability::Event as u8 }>(HardwareCapability::Event)
            .witnessed::<Self, { HardwareCapability::DmaCompletion as u8 }>(
                HardwareCapability::DmaCompletion,
            )
            .witnessed::<Self, { HardwareCapability::Gpio as u8 }>(HardwareCapability::Gpio)
            .witnessed::<Self, { HardwareCapability::Irq as u8 }>(HardwareCapability::Irq)
            .witnessed::<Self, { HardwareCapability::Uart as u8 }>(HardwareCapability::Uart)
            .witnessed::<Self, { HardwareCapability::ByteIo as u8 }>(HardwareCapability::ByteIo)
            .witnessed::<Self, { HardwareCapability::Adc as u8 }>(HardwareCapability::Adc)
            .witnessed::<Self, { HardwareCapability::Pwm as u8 }>(HardwareCapability::Pwm)
            .witnessed::<Self, { HardwareCapability::I2c as u8 }>(HardwareCapability::I2c)
            .witnessed::<Self, { HardwareCapability::Spi as u8 }>(HardwareCapability::Spi)
            .witnessed::<Self, { HardwareCapability::Usb as u8 }>(HardwareCapability::Usb)
            .witnessed::<Self, { HardwareCapability::Reset as u8 }>(HardwareCapability::Reset)
            .witnessed::<Self, { HardwareCapability::Power as u8 }>(HardwareCapability::Power)
            .witnessed::<Self, { HardwareCapability::Multicore as u8 }>(
                HardwareCapability::Multicore,
            )
            .witnessed::<Self, { HardwareCapability::Lease as u8 }>(HardwareCapability::Lease);
        let inapplicable = HardwareCapabilitySet::EMPTY.with(HardwareCapability::Servo);
        HardwareCapabilityDeclaration::new(
            "rp2-native-partial-v3",
            CapabilityProfileKind::Constrained,
            supported,
            supported,
            inapplicable,
            HardwareCapabilitySet::ALL
                .without(supported)
                .without(inapplicable),
            supported,
        )
    };
}

const _: [(); 1] = [(); <Rp2350 as HalCompatibility>::DECLARATION.is_valid() as usize];
const _: [(); 1] = [(); <Rp2350 as HalCompatibility>::DECLARATION.is_exact_profile() as usize];
const _: [(); 1] = [(); (RP2350_RUNTIME.cores == 2) as usize];

impl PlatformHal for Rp2350 {
    const PLATFORM_ID: &'static str = "rp2350";
    type Board = Pico2WBoard;
}

pub fn verify_timebase_provider() -> bool {
    let start = Rp2350::now_us();
    let raw = unsafe { timer_reg(TIMERAWL).read_volatile() };
    while unsafe { timer_reg(TIMERAWL).read_volatile() }.wrapping_sub(raw) < 50 {
        core::hint::spin_loop();
    }
    Rp2350::now_us() > start
}
