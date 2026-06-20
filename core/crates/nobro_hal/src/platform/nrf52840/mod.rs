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
    HalBus, HalClock, HalDeadline, HalEventCapture, HalLease, HalServoPwm, PlatformHal,
};

pub mod inspect;

pub struct Nrf52840;

pub const fn bus_layout() -> BusLayout {
    BusLayout {
        twim0_base: TWIM0_BASE,
        twim1_base: TWIM1_BASE,
    }
}

impl PlatformHal for Nrf52840 {
    const PLATFORM_ID: &'static str = "nrf52840";
    type Board = board::Board;

    fn servo_profile() -> ServoProfile {
        ServoProfile::new(50, board::SERVO_CENTER_US, board::SERVO_PWM_PIN)
    }

    unsafe fn init_timebase() {
        MicroTimer::init();
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
    fn acquire(resource: Resource, owner: u8) -> Result<(), LeaseError> {
        ResourceLease::acquire(resource, owner)
    }

    fn release(resource: Resource, owner: u8) -> Result<(), LeaseError> {
        ResourceLease::release(resource, owner)
    }

    fn is_held(resource: Resource) -> bool {
        ResourceLease::is_held(resource)
    }

    fn acquire_guard(resource: Resource, owner: u8) -> Result<LeaseGuard, LeaseError> {
        ResourceLease::acquire_guard(resource, owner)
    }
}

impl HalDeadline for Nrf52840 {
    unsafe fn init() {
        DeadlineTimer::init();
    }

    fn enable_interrupt() {
        DeadlineTimer::enable_irq();
    }

    fn on_interrupt() {
        DeadlineTimer::on_isr();
    }

    fn poll_compare(on_tick: impl FnOnce(u64)) {
        unsafe {
            let t = nrf52840_pac::TIMER1::ptr();
            if (*t).events_compare[0].read().bits() != 0 {
                (*t).events_compare[0].reset();
                on_tick(MicroTimer::now_us());
            }
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

/// Compile-time selected active backend.
pub type Active = Nrf52840;
