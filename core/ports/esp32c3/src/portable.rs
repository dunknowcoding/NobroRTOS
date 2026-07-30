//! Portable HAL provider contract on the ESP32-C3.
//!
//! Implements the same portable `nobro_hal` provider traits as the nRF52840
//! deep HAL: a real RISC-V (rv32imc) backend, not an nRF-shaped placeholder.
//! The foundational timebase provider is implemented against esp-hal's tested
//! systimer (`esp_hal::time::now()`, a 1 MHz monotonic instant), so kernel
//! code generic over `HalClock`/`HalTimebaseProvider` runs unchanged here.
//!
//! Resource construction remains board-owned. The shared ESP contracts provide
//! bounded GPIO/IRQ, deadline/event, DMA, ADC, LEDC/RMT, bus, byte-I/O,
//! cache/power/reset and generation-safe lease ownership without pretending the
//! single-core C3 has a second application core.

use core::{convert::Infallible, fmt};

use esp_hal::{
    time::Duration, timer::OneShotTimer, usb_serial_jtag::UsbSerialJtag, Blocking, DriverMode,
};
use nobro_hal::board_catalog::EXACT_ESP32C3_SUPERMINI;
use nobro_hal::traits::{
    HalAlarm, HalByteIo, HalClock, HalCompatibility, HalLease, HalTimebaseProvider, PlatformHal,
};
use nobro_hal::{
    BoardCapacity, BoardDesc, CapabilityProfileKind, EspLeases, EspPowerBackend, EspResetBackend,
    HardwareCapability, HardwareCapabilityDeclaration, HardwareCapabilitySet,
    HardwareCapabilityWitness, LeaseError, LeaseId, ESP32C3_RUNTIME,
};

/// The ESP32-C3 portable platform backend.
pub struct Esp32C3;

impl HardwareCapabilityWitness<{ HardwareCapability::Timebase as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Deadline as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Event as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::DmaCompletion as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Gpio as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Irq as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Uart as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::ByteIo as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Adc as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pwm as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pulse as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::I2c as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Spi as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Usb as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Reset as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Power as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Cache as u8 }> for Esp32C3 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Lease as u8 }> for Esp32C3 {}

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
            .witnessed::<Self, { HardwareCapability::Reset as u8 }>(HardwareCapability::Reset)
            .witnessed::<Self, { HardwareCapability::Power as u8 }>(HardwareCapability::Power)
            .witnessed::<Self, { HardwareCapability::Cache as u8 }>(HardwareCapability::Cache)
            .witnessed::<Self, { HardwareCapability::Lease as u8 }>(HardwareCapability::Lease)
            .witnessed::<Self, { HardwareCapability::Usb as u8 }>(HardwareCapability::Usb);
        let supported = witnesses;
        let inapplicable = HardwareCapabilitySet::EMPTY.with(HardwareCapability::Multicore);
        HardwareCapabilityDeclaration::new(
            "deep-esp32c3-v2",
            CapabilityProfileKind::Deep,
            supported,
            supported,
            inapplicable,
            HardwareCapabilitySet::ALL
                .without(supported)
                .without(inapplicable),
            witnesses,
        )
    };
}

const _: [(); 1] = [(); <Esp32C3 as HalCompatibility>::DECLARATION.is_valid() as usize];
const _: [(); 1] = [(); <Esp32C3 as HalCompatibility>::DECLARATION.is_exact_profile() as usize];

impl PlatformHal for Esp32C3 {
    const PLATFORM_ID: &'static str = "esp32c3";
    type Board = Esp32C3Board;
}

pub struct Esp32C3Leases;

impl HalLease for Esp32C3Leases {
    fn acquire(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        EspLeases::acquire(ESP32C3_RUNTIME, resource.into(), owner).map(|guard| {
            core::mem::forget(guard);
        })
    }

    fn release(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        EspLeases::release(ESP32C3_RUNTIME, resource.into(), owner)
    }

    fn is_held(resource: impl Into<LeaseId>) -> bool {
        EspLeases::is_held(ESP32C3_RUNTIME, resource.into())
    }

    fn owner(resource: impl Into<LeaseId>) -> Option<u8> {
        EspLeases::owner(ESP32C3_RUNTIME, resource.into())
    }

    fn release_all_for_owner(owner: u8) -> usize {
        EspLeases::recover_owner(ESP32C3_RUNTIME, owner)
    }
}

pub struct Esp32C3Alarm<'d, Dm> {
    timer: OneShotTimer<'d, Dm>,
    deadline_us: Option<u64>,
}

impl<'d> Esp32C3Alarm<'d, Blocking> {
    pub fn new(timer: OneShotTimer<'d, Blocking>) -> Self {
        Self {
            timer,
            deadline_us: None,
        }
    }
}

impl<Dm: DriverMode> HalAlarm for Esp32C3Alarm<'_, Dm> {
    type Error = esp_hal::timer::Error;

    fn arm_after_us(&mut self, delay_us: u64) -> Result<u64, Self::Error> {
        let delay_us = delay_us.max(1);
        self.timer.schedule(Duration::from_ticks(delay_us))?;
        let deadline = Esp32C3::now_us().saturating_add(delay_us);
        self.deadline_us = Some(deadline);
        Ok(deadline)
    }

    fn cancel(&mut self) {
        self.timer.stop();
        self.timer.clear_interrupt();
        self.deadline_us = None;
    }

    fn deadline_us(&self) -> Option<u64> {
        self.deadline_us
    }

    fn poll_due(&mut self, now_us: u64) -> bool {
        if self.deadline_us.is_some_and(|deadline| now_us >= deadline) {
            self.cancel();
            true
        } else {
            false
        }
    }
}

pub struct Esp32C3Usb<'d>(UsbSerialJtag<'d, Blocking>);

impl<'d> Esp32C3Usb<'d> {
    pub fn new(usb: UsbSerialJtag<'d, Blocking>) -> Self {
        Self(usb)
    }
}

impl HalByteIo for Esp32C3Usb<'_> {
    type Error = Infallible;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        let mut count = 0;
        while count < bytes.len() {
            match self.0.read_byte() {
                Ok(byte) => {
                    bytes[count] = byte;
                    count += 1;
                }
                Err(nb::Error::WouldBlock) => break,
                Err(nb::Error::Other(error)) => return Err(error),
            }
        }
        Ok(count)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.write_bytes(bytes)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.0.flush_tx()
    }
}

impl fmt::Write for Esp32C3Usb<'_> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.write_all(value.as_bytes()).map_err(|_| fmt::Error)
    }
}

pub struct Esp32C3PowerBackend;

impl EspPowerBackend for Esp32C3PowerBackend {
    type Error = Infallible;

    fn cpu_sleep(&mut self) -> Result<(), Self::Error> {
        // CPU sleep only: this deliberately does not enter light/deep sleep,
        // so mounted USB and peripheral clocks are not silently invalidated.
        unsafe { core::arch::asm!("wfi") };
        Ok(())
    }
}

pub struct Esp32C3ResetBackend;

impl EspResetBackend for Esp32C3ResetBackend {
    type Cause = Option<esp_hal::rtc_cntl::SocResetReason>;

    fn reset_cause() -> Self::Cause {
        esp_hal::reset::reset_reason()
    }

    fn system_reset() -> ! {
        esp_hal::reset::software_reset();
        loop {
            core::hint::spin_loop();
        }
    }
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
