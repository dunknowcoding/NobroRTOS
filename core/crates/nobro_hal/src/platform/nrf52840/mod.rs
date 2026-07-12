//! nRF52840 platform backend and first NobroRTOS HAL port.

use crate::board;
use crate::board_desc::{BusLayout, ServoProfile};
use crate::bus::{BusError, TwimBus, TWIM0_BASE, TWIM1_BASE};
use crate::deadline_timer::DeadlineTimer;
use crate::lease::{LeaseError, LeaseGuard, Resource, ResourceLease};
use crate::radio_sim::RadioRxSim;
use crate::snapshots::EventCaptureSnapshot;
use crate::timer::MicroTimer;
use crate::traits::{
    HalBus, HalClock, HalCompatibility, HalDeadline, HalEventCapture, HalI2c, HalLease,
    HalSchedulingProvider, HalServoPwm, HalSpi, HalTimebaseProvider, HardwareCapability,
    HardwareCapabilitySet, LeaseClass, LeaseId, PlatformHal, TransferMode,
};

pub mod inspect;

pub struct Nrf52840;

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

impl HalCompatibility for Nrf52840 {
    const CAPABILITIES: HardwareCapabilitySet = HardwareCapabilitySet::EMPTY
        .with(HardwareCapability::Timebase)
        .with(HardwareCapability::ResourceLease)
        .with(HardwareCapability::DeadlineTimer)
        .with(HardwareCapability::EventCapture)
        .with(HardwareCapability::ServoPwm)
        .with(HardwareCapability::Bus)
        .with(HardwareCapability::I2c)
        .with(HardwareCapability::Spi)
        .with(HardwareCapability::SelfTest);
}

impl PlatformHal for Nrf52840 {
    const PLATFORM_ID: &'static str = "nrf52840";
    type Board = board::Board;
}

impl HalTimebaseProvider for Nrf52840 {
    unsafe fn init_timebase() {
        MicroTimer::init();
    }
}

impl HalSchedulingProvider for Nrf52840 {
    fn servo_profile() -> ServoProfile {
        ServoProfile::new(50, board::SERVO_CENTER_US, board::SERVO_PWM_PIN)
    }

    unsafe fn init_scheduling_demo(profile: ServoProfile) {
        MicroTimer::init();
        DeadlineTimer::init();
        RadioRxSim::init();
        let _ = crate::pwm::PwmServo::init_50hz(profile.pin, profile.center_pulse_us);
    }
}

impl HalClock for Nrf52840 {
    fn now_us() -> u64 {
        MicroTimer::now_us()
    }
}

impl HalLease for Nrf52840 {
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
        (LeaseClass::Timer, 2) => Ok(Resource::Rtc2),
        (LeaseClass::Timer, 1) => Ok(Resource::Timer1),
        (LeaseClass::I2c, 0) => Ok(Resource::Twim0),
        (LeaseClass::I2c, 1) => Ok(Resource::Twim1),
        (LeaseClass::Spi, 0) => Ok(Resource::Spim0),
        (LeaseClass::Radio, 0) => Ok(Resource::Radio),
        (LeaseClass::Pwm, 0) => Ok(Resource::Pwm0),
        (LeaseClass::EventRouter, 0) => Ok(Resource::Ppi),
        (LeaseClass::SoftwareEvent, 0) => Ok(Resource::Egu0),
        _ => Err(LeaseError::Unsupported),
    }
}

impl HalDeadline for Nrf52840 {
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

impl HalServoPwm for Nrf52840 {
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

impl HalEventCapture for Nrf52840 {
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
        EventCaptureSnapshot::capture_radio_channel(channel)
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

/// Compile-time selected active backend.
pub type Active = Nrf52840;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nrf52840_declares_demo_hardware_capabilities() {
        let required = HardwareCapabilitySet::EMPTY
            .with(HardwareCapability::Timebase)
            .with(HardwareCapability::ResourceLease)
            .with(HardwareCapability::DeadlineTimer)
            .with(HardwareCapability::EventCapture)
            .with(HardwareCapability::ServoPwm)
            .with(HardwareCapability::Bus)
            .with(HardwareCapability::I2c)
            .with(HardwareCapability::Spi)
            .with(HardwareCapability::SelfTest);

        assert!(Nrf52840::supports(required));
        assert_eq!(Nrf52840::CAPABILITIES.missing(required).bits(), 0);
        assert_eq!(<TwimBus as HalI2c>::TRANSFER_MODE, TransferMode::Polling);
        assert_eq!(
            <crate::spim_hw::Spim0 as HalSpi>::TRANSFER_MODE,
            TransferMode::Dma
        );
        assert_eq!(
            map_lease(LeaseId::new(LeaseClass::Spi, 7)),
            Err(LeaseError::Unsupported)
        );
    }
}
