//! `embedded-hal` 1.0 I2C adapter over the NobroRTOS TWIM bus.
//!
//! This is the compatibility bridge to the wider Rust embedded ecosystem: it
//! implements `embedded_hal::i2c::I2c` on top of NobroRTOS's `Twim0`, so the large
//! universe of unmodified `embedded-hal` device drivers (sensors, displays, fuel
//! gauges, IO expanders, ...) runs under NobroRTOS without change. It is a thin,
//! bounded, no-heap adapter - the kernel and its principles are untouched. The
//! caller owns the bus lease (`Resource::Twim0`) and initializes the pins
//! (`TwimBus::init_pins`) before use, exactly like any other SAL adapter.
#![no_std]

use embedded_hal::i2c::{Error, ErrorKind, ErrorType, I2c, Operation};
use nobro_hal::{BusError, Twim0};

/// Error wrapper so the HAL's `BusError` satisfies `embedded_hal::i2c::Error`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct NobroI2cError(pub BusError);

impl Error for NobroI2cError {
    fn kind(&self) -> ErrorKind {
        // The TWIM HAL reports a single bounded failure mode; surface it as Other so
        // drivers can branch on Result without a richer (heap-y) error taxonomy.
        ErrorKind::Other
    }
}

/// `embedded-hal` I2C bus backed by NobroRTOS TWIM0.
#[derive(Default)]
pub struct NobroI2c;

impl NobroI2c {
    pub fn new() -> Self {
        NobroI2c
    }
}

impl ErrorType for NobroI2c {
    type Error = NobroI2cError;
}

impl I2c for NobroI2c {
    fn transaction(
        &mut self,
        address: u8,
        operations: &mut [Operation<'_>],
    ) -> Result<(), Self::Error> {
        // Each operation maps to a bounded TWIM transfer. Matches the HAL's existing
        // stop-start framing (see Twim0::write_read), which the supported sensors use.
        for op in operations {
            match op {
                Operation::Write(bytes) => {
                    Twim0::write_bytes(address, bytes).map_err(NobroI2cError)?;
                }
                Operation::Read(buffer) => {
                    Twim0::read_bytes(address, buffer).map_err(NobroI2cError)?;
                }
            }
        }
        Ok(())
    }
}
