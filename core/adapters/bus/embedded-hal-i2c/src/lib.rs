//! `embedded-hal` 1.0 I2C adapter over the NobroRTOS TWIM bus.
//!
//! This is the compatibility bridge to the wider Rust embedded ecosystem: it
//! implements `embedded_hal::i2c::I2c` on top of NobroRTOS's `Twim0`, so the large
//! universe of unmodified `embedded-hal` device drivers (sensors, displays, fuel
//! gauges, IO expanders, ...) runs under NobroRTOS without change. It is a thin,
//! bounded, no-heap adapter - the kernel and its principles are untouched. The
//! mounting owns the bus lease (`Resource::Twim0`) and initializes the pins;
//! callers must not pre-acquire the same physical block.
#![no_std]

use embedded_hal::i2c::{Error, ErrorKind, ErrorType, I2c, Operation};
use nobro_hal::{BusError, TwimBus, TwimFrequency};

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
pub struct NobroI2c {
    // `Option` permits recovery to drop the old generation before acquiring
    // the replacement. Acquiring first would correctly fail as AlreadyHeld.
    bus: Option<TwimBus>,
    frequency: TwimFrequency,
}

impl NobroI2c {
    pub fn new(owner: u8, sda: u8, scl: u8) -> Result<Self, NobroI2cError> {
        Self::new_with_frequency(owner, sda, scl, TwimFrequency::default())
    }

    pub fn new_with_frequency(
        owner: u8,
        sda: u8,
        scl: u8,
        frequency: TwimFrequency,
    ) -> Result<Self, NobroI2cError> {
        let bus = TwimBus::new_twim0(owner).map_err(|_| NobroI2cError(BusError::LeaseDenied))?;
        bus.init_pins_with_frequency(sda, scl, frequency)
            .map_err(NobroI2cError)?;
        Ok(Self {
            bus: Some(bus),
            frequency,
        })
    }

    /// Count responding devices without exposing the nRF-specific bus object.
    ///
    /// Portable drivers should normally probe only their documented addresses.
    /// This method exists for the retained nRF diagnostic application.
    pub fn scan_device_count(&self) -> Result<u8, NobroI2cError> {
        self.bus()?.scan(|_| {}).map_err(NobroI2cError)
    }

    /// Reacquire and initialize the same logical bus after provider recovery.
    pub fn recover(&mut self, owner: u8, sda: u8, scl: u8) -> Result<(), NobroI2cError> {
        // Dropping an active guard quiesces/releases it. Dropping a stale guard
        // after supervisor revocation is a no-op, so it cannot revoke a newer
        // generation. Stay unmounted if reacquisition or pin init fails.
        drop(self.bus.take());
        let replacement = Self::new_with_frequency(owner, sda, scl, self.frequency)?;
        self.bus = replacement.bus;
        Ok(())
    }

    fn bus(&self) -> Result<&TwimBus, NobroI2cError> {
        self.bus
            .as_ref()
            .ok_or(NobroI2cError(BusError::LeaseDenied))
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
        if let [Operation::Write(bytes), Operation::Read(buffer)] = operations {
            return self
                .bus()?
                .write_read(address, bytes, buffer)
                .map_err(NobroI2cError);
        }
        // Preserve the common register-read repeated-start path above. Other
        // transactions remain bounded and map one operation at a time.
        for op in operations {
            match op {
                Operation::Write(bytes) => {
                    self.bus()?.write(address, bytes).map_err(NobroI2cError)?;
                }
                Operation::Read(buffer) => {
                    self.bus()?.read(address, buffer).map_err(NobroI2cError)?;
                }
            }
        }
        Ok(())
    }
}
