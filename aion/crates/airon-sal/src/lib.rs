//! AIRON service abstraction layer with six generic capability traits.

#![no_std]

use airon_kernel::{Criticality, KernelError, ModuleId, ModuleSpec, Sample, SystemBudget};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterDescriptor {
    pub module: ModuleId,
    pub criticality: Criticality,
    pub requires_bits: u32,
    pub owns_bits: u32,
    pub budget: SystemBudget,
}

impl AdapterDescriptor {
    pub const fn from_module_spec(spec: ModuleSpec) -> Self {
        Self {
            module: spec.id,
            criticality: spec.criticality,
            requires_bits: spec.requires.bits(),
            owns_bits: spec.owns.bits(),
            budget: SystemBudget::from_memory(spec.memory),
        }
    }
}

/// Static adapter admission data used by app assembly and boot checks.
pub trait AdapterManifest {
    fn module_spec() -> ModuleSpec;

    fn descriptor() -> AdapterDescriptor {
        AdapterDescriptor::from_module_spec(Self::module_spec())
    }
}

/// I2C / SPI / UART bus transactions with lease guard.
pub trait BusSal {
    type Error;

    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<(), Self::Error>;
    fn write(&mut self, addr: u8, buf: &[u8]) -> Result<(), Self::Error>;
}

/// Host-facing byte streams (IronEngine, INA JSONL, debug).
pub trait StreamSal {
    type Error;

    fn poll(&mut self) -> Result<Option<usize>, Self::Error>;
    fn read_frame(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error>;
    fn write_frame(&mut self, buf: &[u8]) -> Result<(), Self::Error>;
}

/// BLE / 802.15.4 radio pump with compile-time exclusive backends.
pub trait RadioSal {
    type Error;

    fn process(&mut self) -> Result<(), Self::Error>;
    fn rx_available(&self) -> bool;
}

/// Actuators: servo PWM, motor duty with deadline.
pub trait ActuatorSal {
    type Error;

    fn set_duty_us(
        &mut self,
        channel: u8,
        pulse_us: u32,
        deadline_us: u64,
    ) -> Result<(), Self::Error>;
}

/// Sensors return optional Sample tickets.
pub trait SensorSal {
    type Error;

    fn poll(&mut self) -> Result<Option<Sample>, Self::Error>;
}

/// Crypto hardware/software backend.
pub trait CryptoSal {
    type Error;

    fn random(&mut self, dest: &mut [u8]) -> Result<(), Self::Error>;
}

/// Map kernel errors to actions (registered per adapter in later phases).
pub fn default_action(err: &KernelError) -> airon_kernel::Action {
    use airon_kernel::Action::*;
    match err {
        KernelError::LeaseConflict => Ignore,
        KernelError::BusTimeout => RetryDelay(1000),
        KernelError::RadioTxFail => RetryDelay(1000),
        KernelError::SensorReadFail => Ignore,
        KernelError::DeadlineMissed => NotifyUserTask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use airon_kernel::{Capability, CapabilitySet, MemoryBudget};

    struct FakeAdapter;

    impl AdapterManifest for FakeAdapter {
        fn module_spec() -> ModuleSpec {
            ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
                .requires(CapabilitySet::empty().with(Capability::SamplePool))
                .owns(CapabilitySet::empty().with(Capability::Bus0))
                .memory(MemoryBudget::new(2048, 512, 2))
        }
    }

    #[test]
    fn adapter_descriptor_is_derived_from_module_spec() {
        let descriptor = FakeAdapter::descriptor();

        assert_eq!(descriptor.module, ModuleId::Sensor);
        assert_eq!(descriptor.criticality, Criticality::Driver);
        assert_eq!(descriptor.requires_bits, Capability::SamplePool.bit());
        assert_eq!(descriptor.owns_bits, Capability::Bus0.bit());
        assert_eq!(descriptor.budget, SystemBudget::new(2048, 512, 2));
    }
}
