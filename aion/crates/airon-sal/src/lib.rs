//! AIRON service abstraction layer with six generic capability traits.

#![no_std]

use airon_kernel::{KernelError, ModuleSpec, Sample};

/// Static adapter admission data used by app assembly and boot checks.
pub trait AdapterManifest {
    fn module_spec() -> ModuleSpec;
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
