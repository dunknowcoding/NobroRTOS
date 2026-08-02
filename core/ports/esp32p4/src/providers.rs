//! ESP32-P4 providers. The two HP cores are symmetric application cores; the
//! separate LP core has its own image/runtime and is not exposed as HP core 2.

use core::{convert::Infallible, fmt};

use esp_hal::{
    Blocking, DriverMode,
    time::Duration,
    timer::OneShotTimer,
    uart::{RxError as UartRxError, TxError as UartTxError, Uart},
    usb::usb_serial_jtag::UsbSerialJtag,
};
use nobro_hal::{
    BoardCapacity, BoardDesc, CapabilityProfileKind, ESP32P4_RUNTIME, EspLeases, EspPowerBackend,
    EspResetBackend, HalAlarm, HalByteIo, HalClock, HalCompatibility, HalLease, HardwareCapability,
    HardwareCapabilityDeclaration, HardwareCapabilitySet, HardwareCapabilityWitness, LeaseError,
    LeaseId, PlatformHal, board_catalog::EXACT_ESP32P4_PICO,
};

pub struct Esp32P4Providers;

impl HardwareCapabilityWitness<{ HardwareCapability::Timebase as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Deadline as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Event as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::DmaCompletion as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Gpio as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Irq as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Uart as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::ByteIo as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Adc as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pwm as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::I2c as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Spi as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Usb as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Reset as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Power as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Cache as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Multicore as u8 }> for Esp32P4Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Lease as u8 }> for Esp32P4Providers {}

impl HalCompatibility for Esp32P4Providers {
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
            .witnessed::<Self, { HardwareCapability::I2c as u8 }>(HardwareCapability::I2c)
            .witnessed::<Self, { HardwareCapability::Spi as u8 }>(HardwareCapability::Spi)
            .witnessed::<Self, { HardwareCapability::Usb as u8 }>(HardwareCapability::Usb)
            .witnessed::<Self, { HardwareCapability::Reset as u8 }>(HardwareCapability::Reset)
            .witnessed::<Self, { HardwareCapability::Power as u8 }>(HardwareCapability::Power)
            .witnessed::<Self, { HardwareCapability::Cache as u8 }>(HardwareCapability::Cache)
            .witnessed::<Self, { HardwareCapability::Multicore as u8 }>(
                HardwareCapability::Multicore,
            )
            .witnessed::<Self, { HardwareCapability::Lease as u8 }>(HardwareCapability::Lease);
        let inapplicable = HardwareCapabilitySet::EMPTY.with(HardwareCapability::Servo);
        HardwareCapabilityDeclaration::new(
            "esp32p4-native-partial-v3",
            CapabilityProfileKind::Constrained,
            witnesses,
            witnesses,
            inapplicable,
            HardwareCapabilitySet::ALL
                .without(witnesses)
                .without(inapplicable),
            witnesses,
        )
    };
}

const _: [(); 1] = [(); <Esp32P4Providers as HalCompatibility>::DECLARATION.is_valid() as usize];
const _: [(); 1] =
    [(); <Esp32P4Providers as HalCompatibility>::DECLARATION.is_exact_profile() as usize];

pub struct Esp32P4Board;

impl BoardDesc for Esp32P4Board {
    const PLATFORM_ID: &'static str = EXACT_ESP32P4_PICO.platform_id;
    const BOARD_ID: &'static str = EXACT_ESP32P4_PICO.board_id;
    const APP_FLASH_START: u32 = match EXACT_ESP32P4_PICO.app_flash_start {
        Some(address) => address,
        None => 0,
    };
    const CAPACITY: BoardCapacity = EXACT_ESP32P4_PICO.capacity;
    const LED_PIN: Option<u8> = EXACT_ESP32P4_PICO.pins.led_pin;
    const SERVO_PWM_PIN: Option<u8> = EXACT_ESP32P4_PICO.pins.servo_pwm_pin;
    const SERVO_CENTER_US: u32 = 1_500;
    const MVK_TRIGGER_PIN: Option<u8> = EXACT_ESP32P4_PICO.pins.mvk_trigger_pin;
}

impl PlatformHal for Esp32P4Providers {
    const PLATFORM_ID: &'static str = "esp32p4";
    type Board = Esp32P4Board;
}

pub struct Esp32P4Leases;

impl HalLease for Esp32P4Leases {
    fn acquire(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        EspLeases::acquire(ESP32P4_RUNTIME, resource.into(), owner).map(core::mem::forget)
    }

    fn release(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        EspLeases::release(ESP32P4_RUNTIME, resource.into(), owner)
    }

    fn is_held(resource: impl Into<LeaseId>) -> bool {
        EspLeases::is_held(ESP32P4_RUNTIME, resource.into())
    }

    fn owner(resource: impl Into<LeaseId>) -> Option<u8> {
        EspLeases::owner(ESP32P4_RUNTIME, resource.into())
    }

    fn release_all_for_owner(owner: u8) -> usize {
        EspLeases::recover_owner(ESP32P4_RUNTIME, owner)
    }
}

pub struct Esp32P4Clock;

impl HalClock for Esp32P4Clock {
    fn now_us() -> u64 {
        esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_micros()
    }
}

pub struct Esp32P4Alarm<'d, Dm: DriverMode> {
    timer: OneShotTimer<'d, Dm>,
    deadline_us: Option<u64>,
}

impl<'d> Esp32P4Alarm<'d, Blocking> {
    pub fn new(timer: OneShotTimer<'d, Blocking>) -> Self {
        Self {
            timer,
            deadline_us: None,
        }
    }
}

impl<Dm: DriverMode> HalAlarm for Esp32P4Alarm<'_, Dm> {
    type Error = esp_hal::timer::Error;

    fn arm_after_us(&mut self, delay_us: u64) -> Result<u64, Self::Error> {
        let delay_us = delay_us.max(1);
        self.timer.schedule(Duration::from_micros(delay_us))?;
        let deadline = Esp32P4Clock::now_us().saturating_add(delay_us);
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

/// Bounded access to the controller-owned USB Serial/JTAG CDC endpoint.
///
/// Keep this provider separate from the external CH343 UART bridge. In particular,
/// it must not call `esp_hal::UsbSerialJtag::flush_tx`: that API waits for a host IN
/// transaction and can therefore stop the complete application when the four-pin USB
/// cable is absent or the host has not configured CDC. `nobro_usb` polls at most once
/// per operation and reports backpressure/disconnection to the caller instead.
pub struct Esp32P4Usb<'d> {
    // The HAL token performs the chip clock/reset initialization and retains exclusive
    // ownership. Data-plane operations intentionally use only the bounded stack below.
    _controller: UsbSerialJtag<'d, Blocking>,
    stack: nobro_usb::MountedUsb,
}

impl<'d> Esp32P4Usb<'d> {
    pub fn new(controller: UsbSerialJtag<'d, Blocking>, stack: nobro_usb::MountedUsb) -> Self {
        Self {
            _controller: controller,
            stack,
        }
    }
}

impl HalByteIo for Esp32P4Usb<'_> {
    type Error = nobro_usb::UsbIoError;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        self.stack.read_available(bytes)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.stack.write_all(bytes)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.stack.flush_pending()
    }
}

impl fmt::Write for Esp32P4Usb<'_> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.write_all(value.as_bytes()).map_err(|_| fmt::Error)
    }
}

#[derive(Debug)]
pub enum Esp32P4UartError {
    Rx(UartRxError),
    Tx(UartTxError),
}

/// UART0 routed to the external CH343 bridge on the exact P4-Pico board. This
/// remains a different controller and lifecycle from USB Serial/JTAG.
pub struct Esp32P4BridgeUart<'d>(Uart<'d, Blocking>);

impl<'d> Esp32P4BridgeUart<'d> {
    pub fn new(uart: Uart<'d, Blocking>) -> Self {
        Self(uart)
    }
}

impl HalByteIo for Esp32P4BridgeUart<'_> {
    type Error = Esp32P4UartError;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        if bytes.is_empty() || !self.0.read_ready() {
            return Ok(0);
        }
        self.0.read(bytes).map_err(Esp32P4UartError::Rx)
    }

    fn write_all(&mut self, mut bytes: &[u8]) -> Result<(), Self::Error> {
        while !bytes.is_empty() {
            let written = self.0.write(bytes).map_err(Esp32P4UartError::Tx)?;
            bytes = &bytes[written..];
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.0.flush().map_err(Esp32P4UartError::Tx)
    }
}

impl fmt::Write for Esp32P4BridgeUart<'_> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.write_all(value.as_bytes()).map_err(|_| fmt::Error)
    }
}

pub struct Esp32P4PowerBackend;

impl EspPowerBackend for Esp32P4PowerBackend {
    type Error = Infallible;

    fn cpu_sleep(&mut self) -> Result<(), Self::Error> {
        riscv::asm::wfi();
        Ok(())
    }
}

pub struct Esp32P4ResetBackend;

impl EspResetBackend for Esp32P4ResetBackend {
    type Cause = Option<esp_hal::rtc_cntl::SocResetReason>;

    fn reset_cause() -> Self::Cause {
        esp_hal::system::reset_reason()
    }

    fn system_reset() -> ! {
        esp_hal::system::software_reset()
    }
}
