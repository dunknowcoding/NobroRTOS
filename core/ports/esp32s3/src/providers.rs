//! Reusable ESP32-S3 providers. Board pin selection stays with the application.

use core::{convert::Infallible, fmt};

use embedded_hal::{i2c::I2c, pwm::SetDutyCycle, spi::SpiBus};
use esp_hal::{
    cpu_control::CpuControl, time::Duration, timer::OneShotTimer, usb_serial_jtag::UsbSerialJtag,
    Blocking, Cpu, DriverMode,
};
use nobro_hal::{
    board_catalog::EXACT_ESP32S3_UNO, BoardCapacity, BoardDesc, CapabilityProfileKind, EspLeases,
    EspPowerBackend, EspResetBackend, HalAlarm, HalByteIo, HalClock, HalCompatibility, HalI2c,
    HalLease, HalPwmChannel, HalSpi, HardwareCapability, HardwareCapabilityDeclaration,
    HardwareCapabilitySet, HardwareCapabilityWitness, LeaseError, LeaseId, PlatformHal,
    TransferMode, ESP32S3_RUNTIME,
};

pub struct Esp32S3Providers;

impl HardwareCapabilityWitness<{ HardwareCapability::Timebase as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Deadline as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Event as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::DmaCompletion as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Gpio as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Irq as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Uart as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::ByteIo as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Adc as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pwm as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pulse as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::I2c as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Spi as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Usb as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Reset as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Power as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Cache as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Multicore as u8 }> for Esp32S3Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Lease as u8 }> for Esp32S3Providers {}

impl HalCompatibility for Esp32S3Providers {
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
            .witnessed::<Self, { HardwareCapability::Usb as u8 }>(HardwareCapability::Usb)
            .witnessed::<Self, { HardwareCapability::Reset as u8 }>(HardwareCapability::Reset)
            .witnessed::<Self, { HardwareCapability::Power as u8 }>(HardwareCapability::Power)
            .witnessed::<Self, { HardwareCapability::Cache as u8 }>(HardwareCapability::Cache)
            .witnessed::<Self, { HardwareCapability::Multicore as u8 }>(
                HardwareCapability::Multicore,
            )
            .witnessed::<Self, { HardwareCapability::Lease as u8 }>(HardwareCapability::Lease);
        let supported = witnesses;
        let inapplicable = HardwareCapabilitySet::EMPTY.with(HardwareCapability::Servo);
        HardwareCapabilityDeclaration::new(
            "esp32s3-native-partial-v3",
            CapabilityProfileKind::Constrained,
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

const _: [(); 1] = [(); <Esp32S3Providers as HalCompatibility>::DECLARATION.is_valid() as usize];
const _: [(); 1] =
    [(); <Esp32S3Providers as HalCompatibility>::DECLARATION.is_exact_profile() as usize];

pub struct Esp32S3Board;

impl BoardDesc for Esp32S3Board {
    const PLATFORM_ID: &'static str = EXACT_ESP32S3_UNO.platform_id;
    const BOARD_ID: &'static str = EXACT_ESP32S3_UNO.board_id;
    const APP_FLASH_START: u32 = 0;
    const CAPACITY: BoardCapacity = EXACT_ESP32S3_UNO.capacity;
    const LED_PIN: Option<u8> = EXACT_ESP32S3_UNO.pins.led_pin;
    const SERVO_PWM_PIN: Option<u8> = EXACT_ESP32S3_UNO.pins.servo_pwm_pin;
    const SERVO_CENTER_US: u32 = 1_500;
    const MVK_TRIGGER_PIN: Option<u8> = EXACT_ESP32S3_UNO.pins.mvk_trigger_pin;
}

impl PlatformHal for Esp32S3Providers {
    const PLATFORM_ID: &'static str = "esp32s3";
    type Board = Esp32S3Board;
}

pub struct Esp32S3Leases;

impl HalLease for Esp32S3Leases {
    fn acquire(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        EspLeases::acquire(ESP32S3_RUNTIME, resource.into(), owner).map(|guard| {
            core::mem::forget(guard);
        })
    }

    fn release(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        EspLeases::release(ESP32S3_RUNTIME, resource.into(), owner)
    }

    fn is_held(resource: impl Into<LeaseId>) -> bool {
        EspLeases::is_held(ESP32S3_RUNTIME, resource.into())
    }

    fn owner(resource: impl Into<LeaseId>) -> Option<u8> {
        EspLeases::owner(ESP32S3_RUNTIME, resource.into())
    }

    fn release_all_for_owner(owner: u8) -> usize {
        EspLeases::recover_owner(ESP32S3_RUNTIME, owner)
    }
}

pub struct Esp32S3Clock;

impl HalClock for Esp32S3Clock {
    fn now_us() -> u64 {
        esp_hal::time::now().ticks()
    }
}

/// Quiesce an APP core state left active by a debugger/reset path.
///
/// `esp-hal` treats an enabled APP-core clock gate as proof that core 1 is
/// already owned. A debugger reset can leave that bit set even though no Rust
/// closure is installed, making an otherwise clean application cold start
/// fail with `CoreAlreadyRunning`. Call this immediately before the
/// application's first `CpuControl::start_app_core`; it does nothing on a
/// normal reset.
pub fn prepare_app_core_start(cpu_control: &mut CpuControl<'_>) -> bool {
    let system = unsafe { &*esp_hal::peripherals::SYSTEM::PTR };
    let control = system.core_1_control_0();
    if control.read().control_core_1_clkgate_en().bit_is_clear() {
        return false;
    }

    // The caller has not started core 1 yet, so parking APP CPU cannot stall
    // the executing PRO CPU. Hold APP CPU stalled and in reset before removing
    // its stale clock. `start_app_core` then owns the complete clock/reset/
    // unpark sequence and cannot inherit a debugger-created half-started state.
    unsafe {
        cpu_control.park_core(Cpu::AppCpu);
    }
    control.modify(|_, w| w.control_core_1_runstall().set_bit());
    control.modify(|_, w| w.control_core_1_reseting().set_bit());
    control.modify(|_, w| w.control_core_1_clkgate_en().clear_bit());
    true
}

pub struct Esp32S3Alarm<'d, Dm> {
    timer: OneShotTimer<'d, Dm>,
    deadline_us: Option<u64>,
}

impl<'d> Esp32S3Alarm<'d, Blocking> {
    pub fn new(timer: OneShotTimer<'d, Blocking>) -> Self {
        Self {
            timer,
            deadline_us: None,
        }
    }
}

impl<Dm: DriverMode> HalAlarm for Esp32S3Alarm<'_, Dm> {
    type Error = esp_hal::timer::Error;

    fn arm_after_us(&mut self, delay_us: u64) -> Result<u64, Self::Error> {
        let delay_us = delay_us.max(1);
        self.timer.schedule(Duration::from_ticks(delay_us))?;
        let deadline = Esp32S3Clock::now_us().saturating_add(delay_us);
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

pub struct I2cProvider<T>(pub T);

impl<T: I2c> HalI2c for I2cProvider<T> {
    type Error = T::Error;
    const TRANSFER_MODE: TransferMode = TransferMode::Polling;

    fn write(&mut self, address: u8, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.write(address, bytes)
    }

    fn read(&mut self, address: u8, bytes: &mut [u8]) -> Result<(), Self::Error> {
        self.0.read(address, bytes)
    }

    fn write_read(
        &mut self,
        address: u8,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<(), Self::Error> {
        self.0.write_read(address, write, read)
    }
}

pub struct SpiProvider<T>(pub T);

impl<T: SpiBus<u8>> HalSpi for SpiProvider<T> {
    type Error = T::Error;
    const TRANSFER_MODE: TransferMode = TransferMode::Polling;

    fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> Result<(), Self::Error> {
        self.0.transfer(read, write)
    }
}

pub struct PwmProvider<T>(pub T);

impl<T: SetDutyCycle> HalPwmChannel for PwmProvider<T> {
    type Error = T::Error;

    fn max_duty(&self) -> u16 {
        self.0.max_duty_cycle()
    }

    fn set_duty(&mut self, duty: u16) -> Result<(), Self::Error> {
        self.0.set_duty_cycle(duty.min(self.0.max_duty_cycle()))
    }
}

pub struct Esp32S3Usb<'d>(UsbSerialJtag<'d, Blocking>);

impl<'d> Esp32S3Usb<'d> {
    pub fn new(usb: UsbSerialJtag<'d, Blocking>) -> Self {
        Self(usb)
    }
}

impl HalByteIo for Esp32S3Usb<'_> {
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

impl fmt::Write for Esp32S3Usb<'_> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.write_all(value.as_bytes()).map_err(|_| fmt::Error)
    }
}

pub struct Esp32S3PowerBackend;

impl EspPowerBackend for Esp32S3PowerBackend {
    type Error = Infallible;

    fn cpu_sleep(&mut self) -> Result<(), Self::Error> {
        // CPU-only wait preserves the admitted USB/peripheral composition.
        unsafe { core::arch::asm!("waiti 0") };
        Ok(())
    }
}

pub struct Esp32S3ResetBackend;

impl EspResetBackend for Esp32S3ResetBackend {
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
