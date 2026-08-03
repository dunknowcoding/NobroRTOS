//! Classic ESP32 providers. Board pin selection remains application-owned.

use core::{convert::Infallible, fmt};

use esp_hal::{
    Blocking, Cpu, DriverMode,
    cpu_control::CpuControl,
    time::Duration,
    timer::OneShotTimer,
    uart::{Error as UartError, Uart},
};
use nobro_hal::{
    BoardCapacity, BoardDesc, CapabilityProfileKind, ESP32_RUNTIME, EspLeases, EspPowerBackend,
    EspResetBackend, HalAlarm, HalByteIo, HalClock, HalCompatibility, HalLease, HardwareCapability,
    HardwareCapabilityDeclaration, HardwareCapabilitySet, HardwareCapabilityWitness, LeaseError,
    LeaseId, PlatformHal, board_catalog::EXACT_ESP_WROOM32_30PIN,
};

pub struct Esp32Providers;

impl HardwareCapabilityWitness<{ HardwareCapability::Timebase as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Deadline as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Event as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::DmaCompletion as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Gpio as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Irq as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Uart as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::ByteIo as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Adc as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pwm as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pulse as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::I2c as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Spi as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Reset as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Power as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Cache as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Multicore as u8 }> for Esp32Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Lease as u8 }> for Esp32Providers {}

impl HalCompatibility for Esp32Providers {
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
            .witnessed::<Self, { HardwareCapability::Multicore as u8 }>(
                HardwareCapability::Multicore,
            )
            .witnessed::<Self, { HardwareCapability::Lease as u8 }>(HardwareCapability::Lease);
        let inapplicable = HardwareCapabilitySet::EMPTY
            .with(HardwareCapability::Servo)
            .with(HardwareCapability::Usb);
        HardwareCapabilityDeclaration::new(
            "esp32-native-partial-v3",
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

const _: [(); 1] = [(); <Esp32Providers as HalCompatibility>::DECLARATION.is_valid() as usize];
const _: [(); 1] =
    [(); <Esp32Providers as HalCompatibility>::DECLARATION.is_exact_profile() as usize];

pub struct Esp32Board;

impl BoardDesc for Esp32Board {
    const PLATFORM_ID: &'static str = EXACT_ESP_WROOM32_30PIN.platform_id;
    const BOARD_ID: &'static str = EXACT_ESP_WROOM32_30PIN.board_id;
    const APP_FLASH_START: u32 = match EXACT_ESP_WROOM32_30PIN.app_flash_start {
        Some(address) => address,
        None => 0,
    };
    const CAPACITY: BoardCapacity = EXACT_ESP_WROOM32_30PIN.capacity;
    const LED_PIN: Option<u8> = EXACT_ESP_WROOM32_30PIN.pins.led_pin;
    const SERVO_PWM_PIN: Option<u8> = EXACT_ESP_WROOM32_30PIN.pins.servo_pwm_pin;
    const SERVO_CENTER_US: u32 = 1_500;
    const MVK_TRIGGER_PIN: Option<u8> = EXACT_ESP_WROOM32_30PIN.pins.mvk_trigger_pin;
}

impl PlatformHal for Esp32Providers {
    const PLATFORM_ID: &'static str = "esp32";
    type Board = Esp32Board;
}

pub struct Esp32Leases;

impl HalLease for Esp32Leases {
    fn acquire(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        EspLeases::acquire(ESP32_RUNTIME, resource.into(), owner).map(core::mem::forget)
    }

    fn release(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        EspLeases::release(ESP32_RUNTIME, resource.into(), owner)
    }

    fn is_held(resource: impl Into<LeaseId>) -> bool {
        EspLeases::is_held(ESP32_RUNTIME, resource.into())
    }

    fn owner(resource: impl Into<LeaseId>) -> Option<u8> {
        EspLeases::owner(ESP32_RUNTIME, resource.into())
    }

    fn release_all_for_owner(owner: u8) -> usize {
        EspLeases::recover_owner(ESP32_RUNTIME, owner)
    }
}

pub struct Esp32Clock;

/// Return a parked/faulted APP core to the exact state expected by
/// `CpuControl::start_app_core`. Dropping `AppCoreGuard` stalls the core but the
/// classic ESP32 clock-gate bit remains set, which would otherwise make a
/// generation-safe restart fail with `CoreAlreadyRunning`.
pub fn prepare_app_core_start(cpu_control: &mut CpuControl<'_>) -> bool {
    let dport = unsafe { &*esp32::DPORT::PTR };
    if dport
        .appcpu_ctrl_b()
        .read()
        .appcpu_clkgate_en()
        .bit_is_clear()
    {
        return false;
    }

    // The caller has already quiesced or faulted APP CPU. Hold it stalled and
    // in reset before removing the stale clock ownership; start_app_core then
    // owns the complete clock/reset/unpark sequence and stack replacement.
    unsafe {
        cpu_control.park_core(Cpu::AppCpu);
    }
    dport
        .appcpu_ctrl_c()
        .modify(|_, w| w.appcpu_runstall().set_bit());
    dport
        .appcpu_ctrl_a()
        .modify(|_, w| w.appcpu_resetting().set_bit());
    dport
        .appcpu_ctrl_b()
        .modify(|_, w| w.appcpu_clkgate_en().clear_bit());
    true
}

impl HalClock for Esp32Clock {
    fn now_us() -> u64 {
        esp_hal::time::now().ticks()
    }
}

/// UART0 data plane routed through the board's external USB-to-UART bridge.
/// The bridge is transport plumbing, not a native USB controller owned by the
/// ESP32 application.
pub struct Esp32BridgeUart<'d>(Uart<'d, Blocking>);

impl<'d> Esp32BridgeUart<'d> {
    pub fn new(uart: Uart<'d, Blocking>) -> Self {
        Self(uart)
    }
}

impl HalByteIo for Esp32BridgeUart<'_> {
    type Error = UartError;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        self.0.read_buffered_bytes(bytes)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.write_bytes(bytes).map(|_| ())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        nb::block!(self.0.flush())
    }
}

impl fmt::Write for Esp32BridgeUart<'_> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.write_all(value.as_bytes()).map_err(|_| fmt::Error)
    }
}

pub struct Esp32Alarm<'d, Dm> {
    timer: OneShotTimer<'d, Dm>,
    deadline_us: Option<u64>,
}

impl<'d> Esp32Alarm<'d, Blocking> {
    pub fn new(timer: OneShotTimer<'d, Blocking>) -> Self {
        Self {
            timer,
            deadline_us: None,
        }
    }
}

impl<Dm: DriverMode> HalAlarm for Esp32Alarm<'_, Dm> {
    type Error = esp_hal::timer::Error;

    fn arm_after_us(&mut self, delay_us: u64) -> Result<u64, Self::Error> {
        let delay_us = delay_us.max(1);
        self.timer.schedule(Duration::from_ticks(delay_us))?;
        let deadline = Esp32Clock::now_us().saturating_add(delay_us);
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

pub struct Esp32PowerBackend;

impl EspPowerBackend for Esp32PowerBackend {
    type Error = Infallible;

    fn cpu_sleep(&mut self) -> Result<(), Self::Error> {
        unsafe { core::arch::asm!("waiti 0") };
        Ok(())
    }
}

pub struct Esp32ResetBackend;

impl EspResetBackend for Esp32ResetBackend {
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
