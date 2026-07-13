//! RoboServo-style ActuatorSal using the nRF52840 PWM backend.
//!
//! Maps RoboServo semantics (`set_pulse_us` @ 50 Hz) onto `nobro-hal` servo PWM.

#![no_std]

use nobro_hal::{traits::HalServoPwm, ActivePlatform as Hal};
use nobro_kernel::{
    Capability, CapabilitySet, Criticality, DeadlineContract, MemoryBudget, ModuleId, ModuleSpec,
};
use nobro_sal::AdapterManifest;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoboServoError {
    InvalidChannel,
    PulseOutOfRange,
}

pub struct RoboServoAdapter {
    pin: u8,
    min_us: u32,
    max_us: u32,
}

impl RoboServoAdapter {
    pub fn new(pin: u8) -> Self {
        Self {
            pin,
            min_us: 500,
            max_us: 2500,
        }
    }

    /// # Safety
    /// The caller must hold the PWM0 lease and ensure this pin is configured as the
    /// exclusive servo output for the lifetime of the active PWM peripheral.
    pub unsafe fn attach_50hz(&self, center_us: u32) -> Result<(), RoboServoError> {
        if center_us < self.min_us || center_us > self.max_us {
            return Err(RoboServoError::PulseOutOfRange);
        }
        <Hal as HalServoPwm>::init_50hz(self.pin, center_us);
        Ok(())
    }
}

impl AdapterManifest for RoboServoAdapter {
    fn module_spec() -> ModuleSpec {
        ModuleSpec::new(ModuleId::Actuator, Criticality::HardRealtime)
            .requires(CapabilitySet::empty().with(Capability::DeadlineTimer))
            .owns(CapabilitySet::empty().with(Capability::ServoPwm))
            .memory(MemoryBudget::new(5 * 1024, 512, 0))
            .deadline(DeadlineContract::new(20_000, 10))
    }
}

impl nobro_sal::ActuatorSal for RoboServoAdapter {
    type Error = RoboServoError;

    fn set_duty_us(
        &mut self,
        channel: u8,
        pulse_us: u32,
        _deadline_us: u64,
    ) -> Result<(), Self::Error> {
        if channel != 0 {
            return Err(RoboServoError::InvalidChannel);
        }
        if pulse_us < self.min_us || pulse_us > self.max_us {
            return Err(RoboServoError::PulseOutOfRange);
        }
        unsafe {
            <Hal as HalServoPwm>::set_active_pulse_us(pulse_us);
        }
        Ok(())
    }
}

pub fn module_spec() -> ModuleSpec {
    RoboServoAdapter::module_spec()
}
