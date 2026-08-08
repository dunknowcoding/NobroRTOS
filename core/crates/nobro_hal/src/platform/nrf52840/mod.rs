//! nRF52840 platform backend and first NobroRTOS HAL port.

use crate::board;
use crate::board_desc::{BoardDesc, BusLayout, ServoProfile};
use crate::bus::{BusError, TwimBus, TWIM0_BASE, TWIM1_BASE};
use crate::deadline_timer::DeadlineTimer;
use crate::lease::{LeaseError, LeaseGuard, Resource, ResourceLease};
use crate::radio_sim::RadioRxSim;
use crate::snapshots::EventCaptureSnapshot;
use crate::timer::MicroTimer;
use crate::traits::{
    CapabilityProfileKind, HalBus, HalClock, HalCompatibility, HalDeadline, HalEventCapture,
    HalI2c, HalLease, HalSchedulingProvider, HalServoPwm, HalSpi, HalTimebaseProvider,
    HardwareCapability, HardwareCapabilityDeclaration, HardwareCapabilitySet,
    HardwareCapabilityWitness, LeaseClass, LeaseId, PlatformHal, TransferMode,
};

/// Exact native composition for a ProMicro nRF52840 without a resident
/// SoftDevice. Optional PSP/PendSV time slicing is valid only for this type.
pub struct Nrf52840NoSoftDevice;

/// Exact native composition for a ProMicro nRF52840 with S140 v6 resident.
/// Its application IRQ ceiling preserves the SoftDevice-owned priority bands.
pub struct Nrf52840S140V6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrfPreemptionMode {
    OptionalCortexMSlice,
    CooperativeOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NrfRuntimeContract {
    pub boot_layout: crate::board_desc::BootLayout,
    pub app_flash_start: u32,
    pub ram_start: u32,
    pub irq_ceiling_logical: u8,
    pub preemption: NrfPreemptionMode,
    /// NobroRTOS USB-capable firmware uses bounded System-ON idle; it never
    /// enters SYSTEMOFF as an implicit idle action.
    pub system_on_idle_only: bool,
    /// The USBD backend is singleton-owned and participates in the power veto
    /// and bounded re-enumeration lifecycle.
    pub usb_lifecycle_guarded: bool,
}

pub trait ExactNrf52840 {
    const RUNTIME: NrfRuntimeContract;
}

impl ExactNrf52840 for Nrf52840NoSoftDevice {
    const RUNTIME: NrfRuntimeContract = NrfRuntimeContract {
        boot_layout: crate::board_catalog::PROMICRO_NRF52840_NOSD_PACKAGE
            .boot
            .layout,
        app_flash_start: crate::board_catalog::PROMICRO_NRF52840_NOSD_PACKAGE
            .boot
            .app_flash_start,
        ram_start: crate::board_catalog::PROMICRO_NRF52840_NOSD_PACKAGE
            .boot
            .ram_start,
        irq_ceiling_logical: 3,
        preemption: NrfPreemptionMode::OptionalCortexMSlice,
        system_on_idle_only: true,
        usb_lifecycle_guarded: true,
    };
}

impl ExactNrf52840 for Nrf52840S140V6 {
    const RUNTIME: NrfRuntimeContract = NrfRuntimeContract {
        boot_layout: crate::board_catalog::PROMICRO_NRF52840_S140_PACKAGE
            .boot
            .layout,
        app_flash_start: crate::board_catalog::PROMICRO_NRF52840_S140_PACKAGE
            .boot
            .app_flash_start,
        ram_start: crate::board_catalog::PROMICRO_NRF52840_S140_PACKAGE
            .boot
            .ram_start,
        irq_ceiling_logical: 6,
        preemption: NrfPreemptionMode::CooperativeOnly,
        system_on_idle_only: true,
        usb_lifecycle_guarded: true,
    };
}

#[cfg(feature = "board-promicro-s140")]
pub type Nrf52840 = Nrf52840S140V6;
#[cfg(not(feature = "board-promicro-s140"))]
pub type Nrf52840 = Nrf52840NoSoftDevice;

/// Compile-time selected exact backend retained for source compatibility.
pub type Active = Nrf52840;

trait ActiveNrf52840Backend: ExactNrf52840 {}
#[cfg(not(feature = "board-promicro-s140"))]
impl ActiveNrf52840Backend for Nrf52840NoSoftDevice {}
#[cfg(feature = "board-promicro-s140")]
impl ActiveNrf52840Backend for Nrf52840S140V6 {}

impl HardwareCapabilityWitness<{ HardwareCapability::Timebase as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Deadline as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Event as u8 }> for Nrf52840NoSoftDevice {}
// The DMA-completion witness is provided by the interrupt-driven,
// cancellation-safe SPIM EasyDMA path. `nrf-twim-async` adds the corresponding
// TWIM provider but is not required for this composition-level capability.
impl HardwareCapabilityWitness<{ HardwareCapability::DmaCompletion as u8 }>
    for Nrf52840NoSoftDevice
{
}
impl HardwareCapabilityWitness<{ HardwareCapability::Gpio as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Irq as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Uart as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::ByteIo as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Adc as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pwm as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Servo as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pulse as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::I2c as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Spi as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Usb as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Watchdog as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Rtc as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Flash as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Reset as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Power as u8 }> for Nrf52840NoSoftDevice {}
impl HardwareCapabilityWitness<{ HardwareCapability::Lease as u8 }> for Nrf52840NoSoftDevice {}

impl HardwareCapabilityWitness<{ HardwareCapability::Timebase as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Deadline as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Event as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::DmaCompletion as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Gpio as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Irq as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Uart as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::ByteIo as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Adc as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pwm as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Servo as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pulse as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::I2c as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Spi as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Usb as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Watchdog as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Rtc as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Flash as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Reset as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Power as u8 }> for Nrf52840S140V6 {}
impl HardwareCapabilityWitness<{ HardwareCapability::Lease as u8 }> for Nrf52840S140V6 {}

const PPI_BASE: u32 = 0x4001_F000;
const PPI_CHEN: u32 = 0x500;
const PPI_CH0_EEP: u32 = 0x510;
const PPI_CH0_TEP: u32 = 0x514;
const TIMER0_CAPTURE2_TASK: u32 = 0x4000_8048;

unsafe fn capture_event_channel(channel: usize) -> EventCaptureSnapshot {
    let read = |offset: u32| unsafe { core::ptr::read_volatile((PPI_BASE + offset) as *const u32) };
    let channel_enabled = read(PPI_CHEN) & (1 << channel) != 0;
    let source = read(PPI_CH0_EEP + channel as u32 * 8);
    let sink = read(PPI_CH0_TEP + channel as u32 * 8);
    EventCaptureSnapshot {
        channel_enabled,
        source_wired: source != 0,
        sink_wired: sink == TIMER0_CAPTURE2_TASK,
    }
}

/// Coherent scheduling-demo authority: clock, deadline, PWM, software event, and
/// event router are acquired as one all-or-nothing generation-checked session.
pub struct NrfSchedulingSession {
    timer: LeaseGuard,
    deadline: LeaseGuard,
    pwm: LeaseGuard,
    software_event: LeaseGuard,
    event_router: LeaseGuard,
}

impl NrfSchedulingSession {
    /// # Safety
    /// The profile pin must match the board wiring and the peripherals must be idle.
    pub unsafe fn acquire(owner: u8, profile: ServoProfile) -> Result<Self, LeaseError> {
        let timer = ResourceLease::acquire_guard(Resource::Timer0, owner)?;
        let deadline = ResourceLease::acquire_guard(Resource::Timer1, owner)?;
        let pwm = ResourceLease::acquire_guard(Resource::Pwm0, owner)?;
        let software_event = ResourceLease::acquire_guard(Resource::Egu0, owner)?;
        let event_router = ResourceLease::acquire_guard(Resource::Ppi, owner)?;
        Nrf52840::init_scheduling_demo(profile);
        Ok(Self {
            timer,
            deadline,
            pwm,
            software_event,
            event_router,
        })
    }

    pub fn now_us(&self) -> Result<u64, LeaseError> {
        self.timer.ensure_live()?;
        Ok(MicroTimer::now_us())
    }

    pub fn poll_compare(&self, on_tick: impl FnOnce(u64)) -> Result<(), LeaseError> {
        self.deadline.ensure_live()?;
        unsafe { Nrf52840::poll_compare(on_tick) };
        Ok(())
    }

    /// Bounded provider half for `Scheduler::reconfigure_tick_period`.
    pub fn set_deadline_period_us(&self, period_us: u32) -> Result<(), LeaseError> {
        self.deadline.ensure_live()?;
        unsafe { DeadlineTimer::set_period_us(period_us) }
    }

    pub fn trigger_and_latency_us(&self) -> Result<Option<u32>, LeaseError> {
        self.timer.ensure_live()?;
        self.software_event.ensure_live()?;
        self.event_router.ensure_live()?;
        Ok(unsafe { Nrf52840::trigger_and_latency_us() })
    }

    pub fn set_servo_pulse_us(&self, pulse_us: u32) -> Result<(), LeaseError> {
        self.pwm.ensure_live()?;
        unsafe { Nrf52840::set_active_pulse_us(pulse_us) };
        Ok(())
    }
}

pub const fn bus_layout() -> BusLayout {
    BusLayout {
        twim0_base: TWIM0_BASE,
        twim1_base: TWIM1_BASE,
    }
}

impl HalCompatibility for Nrf52840NoSoftDevice {
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
            .witnessed::<Self, { HardwareCapability::Servo as u8 }>(HardwareCapability::Servo)
            .witnessed::<Self, { HardwareCapability::Pulse as u8 }>(HardwareCapability::Pulse)
            .witnessed::<Self, { HardwareCapability::I2c as u8 }>(HardwareCapability::I2c)
            .witnessed::<Self, { HardwareCapability::Spi as u8 }>(HardwareCapability::Spi)
            .witnessed::<Self, { HardwareCapability::Usb as u8 }>(HardwareCapability::Usb)
            .witnessed::<Self, { HardwareCapability::Watchdog as u8 }>(HardwareCapability::Watchdog)
            .witnessed::<Self, { HardwareCapability::Rtc as u8 }>(HardwareCapability::Rtc)
            .witnessed::<Self, { HardwareCapability::Flash as u8 }>(HardwareCapability::Flash)
            .witnessed::<Self, { HardwareCapability::Reset as u8 }>(HardwareCapability::Reset)
            .witnessed::<Self, { HardwareCapability::Power as u8 }>(HardwareCapability::Power)
            .witnessed::<Self, { HardwareCapability::Lease as u8 }>(HardwareCapability::Lease);
        let supported = witnesses;
        let inapplicable = HardwareCapabilitySet::EMPTY
            .with(HardwareCapability::Cache)
            .with(HardwareCapability::Multicore);
        HardwareCapabilityDeclaration::new(
            "nrf52840-native-nosd-deep-v4",
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

impl HalCompatibility for Nrf52840S140V6 {
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
            .witnessed::<Self, { HardwareCapability::Servo as u8 }>(HardwareCapability::Servo)
            .witnessed::<Self, { HardwareCapability::Pulse as u8 }>(HardwareCapability::Pulse)
            .witnessed::<Self, { HardwareCapability::I2c as u8 }>(HardwareCapability::I2c)
            .witnessed::<Self, { HardwareCapability::Spi as u8 }>(HardwareCapability::Spi)
            .witnessed::<Self, { HardwareCapability::Usb as u8 }>(HardwareCapability::Usb)
            .witnessed::<Self, { HardwareCapability::Watchdog as u8 }>(HardwareCapability::Watchdog)
            .witnessed::<Self, { HardwareCapability::Rtc as u8 }>(HardwareCapability::Rtc)
            .witnessed::<Self, { HardwareCapability::Flash as u8 }>(HardwareCapability::Flash)
            .witnessed::<Self, { HardwareCapability::Reset as u8 }>(HardwareCapability::Reset)
            .witnessed::<Self, { HardwareCapability::Power as u8 }>(HardwareCapability::Power)
            .witnessed::<Self, { HardwareCapability::Lease as u8 }>(HardwareCapability::Lease);
        let supported = witnesses;
        let inapplicable = HardwareCapabilitySet::EMPTY
            .with(HardwareCapability::Cache)
            .with(HardwareCapability::Multicore);
        HardwareCapabilityDeclaration::new(
            "nrf52840-native-s140-deep-v4",
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

const _: [(); 1] =
    [(); <Nrf52840NoSoftDevice as HalCompatibility>::DECLARATION.is_valid() as usize];
const _: [(); 1] =
    [(); <Nrf52840NoSoftDevice as HalCompatibility>::DECLARATION.is_exact_profile() as usize];
const _: [(); 1] = [(); <Nrf52840S140V6 as HalCompatibility>::DECLARATION.is_valid() as usize];
const _: [(); 1] =
    [(); <Nrf52840S140V6 as HalCompatibility>::DECLARATION.is_exact_profile() as usize];

#[cfg(not(feature = "board-promicro-s140"))]
impl PlatformHal for Nrf52840NoSoftDevice {
    const PLATFORM_ID: &'static str = "nrf52840";
    type Board = board::ProMicroNrf52840NoSoftDevice;
}

#[cfg(feature = "board-promicro-s140")]
impl PlatformHal for Nrf52840S140V6 {
    const PLATFORM_ID: &'static str = "nrf52840";
    type Board = board::ProMicroNrf52840S140V6;
}

impl<T: ActiveNrf52840Backend> HalTimebaseProvider for T {
    unsafe fn init_timebase() {
        MicroTimer::init();
    }
}

impl<T: ActiveNrf52840Backend + PlatformHal> HalSchedulingProvider for T {
    fn servo_profile() -> ServoProfile {
        ServoProfile::new(
            50,
            <T::Board as BoardDesc>::SERVO_CENTER_US,
            <T::Board as BoardDesc>::SERVO_PWM_PIN
                .expect("exact nRF52840 composition must select a servo pin"),
        )
    }

    unsafe fn init_scheduling_demo(profile: ServoProfile) {
        MicroTimer::init();
        DeadlineTimer::init();
        RadioRxSim::init();
        let _ = crate::pwm::PwmServo::init_50hz(profile.pin, profile.center_pulse_us);
    }
}

impl<T: ActiveNrf52840Backend> HalClock for T {
    fn now_us() -> u64 {
        MicroTimer::now_us()
    }
}

impl<T: ActiveNrf52840Backend> HalLease for T {
    fn acquire(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        ResourceLease::acquire(map_lease(resource.into())?, owner)
    }

    fn release(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        ResourceLease::release(map_lease(resource.into())?, owner)
    }

    fn is_held(resource: impl Into<LeaseId>) -> bool {
        map_lease(resource.into()).is_ok_and(ResourceLease::is_held)
    }

    fn owner(resource: impl Into<LeaseId>) -> Option<u8> {
        map_lease(resource.into())
            .ok()
            .and_then(ResourceLease::owner)
    }

    fn release_all_for_owner(owner: u8) -> usize {
        ResourceLease::release_all_for_owner(owner)
    }
}

fn map_lease(resource: LeaseId) -> Result<Resource, LeaseError> {
    match (resource.class, resource.instance) {
        (LeaseClass::Timer, 0) => Ok(Resource::Timer0),
        (LeaseClass::Timer, 2) | (LeaseClass::Rtc, 0) => Ok(Resource::Rtc2),
        (LeaseClass::Timer, 1) => Ok(Resource::Timer1),
        (LeaseClass::I2c, 0) => Ok(Resource::Twim0),
        (LeaseClass::I2c, 1) => Ok(Resource::Twim1),
        (LeaseClass::Spi, 0) => Ok(Resource::Spim0),
        (LeaseClass::Radio, 0) => Ok(Resource::Radio),
        (LeaseClass::Pwm, 0) => Ok(Resource::Pwm0),
        (LeaseClass::EventRouter, 0) => Ok(Resource::Ppi),
        (LeaseClass::SoftwareEvent, 0) => Ok(Resource::Egu0),
        (LeaseClass::Gpio, 0) => Ok(Resource::Gpio),
        (LeaseClass::Irq, 0) => Ok(Resource::Gpiote),
        (LeaseClass::Uart, 0) => Ok(Resource::Uarte0),
        (LeaseClass::Adc, 0) => Ok(Resource::Saadc),
        // Reset:1 was the pre-contract-v2 spelling. Keep it as a compatibility
        // alias while every new receipt uses the cross-platform flash class.
        (LeaseClass::Reset, 1) | (LeaseClass::Flash, 0) => Ok(Resource::Nvmc),
        (LeaseClass::Pulse, 0) => Ok(Resource::Timer2),
        _ => Err(LeaseError::Unsupported),
    }
}

impl<T: ActiveNrf52840Backend> HalDeadline for T {
    unsafe fn init() {
        DeadlineTimer::init();
    }

    unsafe fn enable_interrupt() {
        DeadlineTimer::enable_irq();
    }

    unsafe fn on_interrupt() {
        DeadlineTimer::on_isr();
    }

    unsafe fn poll_compare(on_tick: impl FnOnce(u64)) {
        let t = nrf52840_pac::TIMER1::ptr();
        if (*t).events_compare[0].read().bits() != 0 {
            (*t).events_compare[0].reset();
            on_tick(MicroTimer::now_us());
        }
    }
}

impl<T: ActiveNrf52840Backend> HalServoPwm for T {
    unsafe fn init_50hz(pin: u8, pulse_us: u32) {
        let _ = crate::pwm::PwmServo::init_50hz(pin, pulse_us);
    }

    unsafe fn set_active_pulse_us(pulse_us: u32) {
        crate::pwm::PwmServo::set_active_pulse_us(pulse_us);
    }

    fn read_pulse_us() -> u32 {
        crate::pwm::PwmServo::read_pulse_us()
    }
}

impl<T: ActiveNrf52840Backend> HalEventCapture for T {
    unsafe fn init() {
        RadioRxSim::init();
    }

    unsafe fn trigger_and_latency_us() -> Option<u32> {
        RadioRxSim::trigger_and_latency_us()
    }

    fn latency_stats() -> (u32, u32) {
        RadioRxSim::latency_stats()
    }

    unsafe fn capture_snapshot(channel: usize) -> EventCaptureSnapshot {
        capture_event_channel(channel)
    }
}

impl HalBus for TwimBus {
    type Error = BusError;

    fn acquire_twim0(owner: u8) -> Result<Self, LeaseError> {
        TwimBus::new_twim0(owner)
    }

    fn read_stub(&self, addr: u8, buf: &mut [u8]) -> Result<(), Self::Error> {
        TwimBus::read_stub(self, addr, buf)
    }
}

impl HalI2c for TwimBus {
    type Error = BusError;
    const TRANSFER_MODE: TransferMode = TransferMode::Polling;

    fn write(&mut self, address: u8, bytes: &[u8]) -> Result<(), Self::Error> {
        TwimBus::write(self, address, bytes)
    }

    fn read(&mut self, address: u8, bytes: &mut [u8]) -> Result<(), Self::Error> {
        TwimBus::read(self, address, bytes)
    }

    fn write_read(
        &mut self,
        address: u8,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<(), Self::Error> {
        TwimBus::write_read(self, address, write, read)
    }
}

impl HalSpi for crate::spim_hw::Spim0 {
    type Error = BusError;
    const TRANSFER_MODE: TransferMode = TransferMode::Dma;

    fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> Result<(), Self::Error> {
        crate::spim_hw::Spim0::transfer(self, write, read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nrf52840_declares_demo_hardware_capabilities() {
        let required = HardwareCapabilitySet::EMPTY
            .with(HardwareCapability::Timebase)
            .with(HardwareCapability::Lease)
            .with(HardwareCapability::Deadline)
            .with(HardwareCapability::Event)
            .with(HardwareCapability::DmaCompletion)
            .with(HardwareCapability::Servo)
            .with(HardwareCapability::I2c)
            .with(HardwareCapability::Spi)
            .with(HardwareCapability::Usb);

        assert!(Nrf52840::supports(required));
        assert_eq!(Nrf52840::CAPABILITIES.missing(required).bits(), 0);
        assert!(Nrf52840::DECLARATION.is_valid());
        assert!(Nrf52840::DECLARATION.is_exact_profile());
        assert_eq!(<TwimBus as HalI2c>::TRANSFER_MODE, TransferMode::Polling);
        assert_eq!(
            <crate::spim_hw::Spim0 as HalSpi>::TRANSFER_MODE,
            TransferMode::Dma
        );
        assert_eq!(
            map_lease(LeaseId::new(LeaseClass::Spi, 7)),
            Err(LeaseError::Unsupported)
        );
        assert_eq!(map_lease(LeaseId::SYSTEM_RTC), Ok(Resource::Rtc2));
        assert_eq!(map_lease(LeaseId::APPLICATION_FLASH), Ok(Resource::Nvmc));
        assert_eq!(LeaseId::from(Resource::Nvmc), LeaseId::APPLICATION_FLASH);
    }

    #[test]
    fn exact_boot_compositions_publish_distinct_runtime_contracts() {
        let nosd = Nrf52840NoSoftDevice::RUNTIME;
        let s140 = Nrf52840S140V6::RUNTIME;

        assert_eq!(
            nosd.boot_layout,
            crate::board_desc::BootLayout::NoSoftDevice
        );
        assert_eq!(
            s140.boot_layout,
            crate::board_desc::BootLayout::SoftDeviceS140V6
        );
        assert_eq!(nosd.irq_ceiling_logical, 3);
        assert_eq!(s140.irq_ceiling_logical, 6);
        assert_eq!(nosd.preemption, NrfPreemptionMode::OptionalCortexMSlice);
        assert_eq!(s140.preemption, NrfPreemptionMode::CooperativeOnly);
        assert!(nosd.system_on_idle_only && s140.system_on_idle_only);
        assert!(nosd.usb_lifecycle_guarded && s140.usb_lifecycle_guarded);
        assert_ne!(nosd.app_flash_start, s140.app_flash_start);
        assert_ne!(nosd.ram_start, s140.ram_start);
    }
}
