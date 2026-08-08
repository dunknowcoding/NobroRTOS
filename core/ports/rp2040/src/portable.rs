//! Exact RP2040/Pico binding for the shared RP-series contract.

use nobro_hal::board_catalog::EXACT_RASPBERRY_PI_PICO_RP2040;
use nobro_hal::{
    BoardCapacity, BoardDesc, CapabilityProfileKind, HalClock, HalCompatibility,
    HalTimebaseProvider, HardwareCapability, HardwareCapabilityDeclaration, HardwareCapabilitySet,
    HardwareCapabilityWitness, PlatformHal, Rp2PowerBackend, Rp2ResetBackend, RP2040_RUNTIME,
};

const TIMER_BASE: usize = 0x4005_4000;
const TIMEHR: usize = 0x08;
const TIMELR: usize = 0x0c;
const TIMERAWL: usize = 0x28;

#[inline]
fn timer_reg(offset: usize) -> *const u32 {
    (TIMER_BASE + offset) as *const u32
}

pub struct Rp2040;

pub struct Rp2040Power;

impl Rp2PowerBackend for Rp2040Power {
    type Error = core::convert::Infallible;

    fn cpu_sleep(&mut self) -> Result<(), Self::Error> {
        cortex_m::asm::dsb();
        cortex_m::asm::wfi();
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rp2040ResetCause {
    Watchdog,
    Software,
    PowerOnOrExternal,
}

pub struct Rp2040Reset;

impl Rp2ResetBackend for Rp2040Reset {
    type Cause = Rp2040ResetCause;

    fn reset_cause() -> Self::Cause {
        let reason = unsafe { &*rp2040_hal::pac::WATCHDOG::PTR }.reason().read();
        if reason.timer().bit_is_set() {
            Self::Cause::Watchdog
        } else if reason.force().bit_is_set() {
            Self::Cause::Software
        } else {
            Self::Cause::PowerOnOrExternal
        }
    }

    fn system_reset() -> ! {
        rp2040_hal::reset()
    }
}

impl HardwareCapabilityWitness<{ HardwareCapability::Timebase as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Deadline as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Event as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::DmaCompletion as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Gpio as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Irq as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Uart as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::ByteIo as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Adc as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pwm as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pulse as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::I2c as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Spi as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Usb as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Watchdog as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Rtc as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Flash as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Reset as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Power as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Cache as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Multicore as u8 }> for Rp2040 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Lease as u8 }> for Rp2040 {}

pub struct PicoBoard;

impl BoardDesc for PicoBoard {
    const PLATFORM_ID: &'static str = EXACT_RASPBERRY_PI_PICO_RP2040.platform_id;
    const BOARD_ID: &'static str = EXACT_RASPBERRY_PI_PICO_RP2040.board_id;
    const APP_FLASH_START: u32 = match EXACT_RASPBERRY_PI_PICO_RP2040.app_flash_start {
        Some(start) => start,
        None => 0,
    };
    const CAPACITY: BoardCapacity = EXACT_RASPBERRY_PI_PICO_RP2040.capacity;
    const LED_PIN: Option<u8> = EXACT_RASPBERRY_PI_PICO_RP2040.pins.led_pin;
    const SERVO_PWM_PIN: Option<u8> = EXACT_RASPBERRY_PI_PICO_RP2040.pins.servo_pwm_pin;
    const SERVO_CENTER_US: u32 = 1_500;
    const MVK_TRIGGER_PIN: Option<u8> = EXACT_RASPBERRY_PI_PICO_RP2040.pins.mvk_trigger_pin;
}

impl HalClock for Rp2040 {
    fn now_us() -> u64 {
        // TIMELR latches TIMEHR, producing one coherent 64-bit sample.
        unsafe {
            let low = timer_reg(TIMELR).read_volatile();
            let high = timer_reg(TIMEHR).read_volatile();
            (u64::from(high) << 32) | u64::from(low)
        }
    }
}

impl HalTimebaseProvider for Rp2040 {
    unsafe fn init_timebase() {}
}

impl HalCompatibility for Rp2040 {
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
            .witnessed::<Self, { HardwareCapability::Pulse as u8 }>(HardwareCapability::Pulse)
            .witnessed::<Self, { HardwareCapability::I2c as u8 }>(HardwareCapability::I2c)
            .witnessed::<Self, { HardwareCapability::Spi as u8 }>(HardwareCapability::Spi)
            .witnessed::<Self, { HardwareCapability::Usb as u8 }>(HardwareCapability::Usb)
            .witnessed::<Self, { HardwareCapability::Watchdog as u8 }>(HardwareCapability::Watchdog)
            .witnessed::<Self, { HardwareCapability::Rtc as u8 }>(HardwareCapability::Rtc)
            .witnessed::<Self, { HardwareCapability::Flash as u8 }>(HardwareCapability::Flash)
            .witnessed::<Self, { HardwareCapability::Reset as u8 }>(HardwareCapability::Reset)
            .witnessed::<Self, { HardwareCapability::Power as u8 }>(HardwareCapability::Power)
            .witnessed::<Self, { HardwareCapability::Cache as u8 }>(HardwareCapability::Cache)
            .witnessed::<Self, { HardwareCapability::Multicore as u8 }>(
                HardwareCapability::Multicore,
            )
            .witnessed::<Self, { HardwareCapability::Lease as u8 }>(HardwareCapability::Lease);
        let inapplicable = HardwareCapabilitySet::EMPTY.with(HardwareCapability::Servo);
        HardwareCapabilityDeclaration::new(
            "rp2040-native-deep-v4",
            CapabilityProfileKind::Deep,
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

const _: [(); 1] = [(); <Rp2040 as HalCompatibility>::DECLARATION.is_valid() as usize];
const _: [(); 1] = [(); <Rp2040 as HalCompatibility>::DECLARATION.is_exact_profile() as usize];
const _: [(); 1] = [(); (RP2040_RUNTIME.cores == 2) as usize];

impl PlatformHal for Rp2040 {
    const PLATFORM_ID: &'static str = "rp2040";
    type Board = PicoBoard;
}

pub fn verify_timebase_provider() -> bool {
    let start = Rp2040::now_us();
    let raw = unsafe { timer_reg(TIMERAWL).read_volatile() };
    while unsafe { timer_reg(TIMERAWL).read_volatile() }.wrapping_sub(raw) < 50 {
        core::hint::spin_loop();
    }
    Rp2040::now_us() > start
}
