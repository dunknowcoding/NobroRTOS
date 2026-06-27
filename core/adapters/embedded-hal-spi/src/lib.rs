//! `embedded-hal` 1.0 `SpiDevice` adapter over the NobroRTOS SPIM0 bus.
//!
//! Companion to the I2C adapter: it implements `embedded_hal::spi::SpiDevice` on top of
//! NobroRTOS's `Spim0`, so unmodified `embedded-hal` SPI device drivers (IMUs, flash,
//! displays, ADCs, ...) run under NobroRTOS without change. The whole transaction is
//! framed by one software chip-select (with the setup/recovery delays `Spim0` needs),
//! and every operation is a bounded, no-heap EasyDMA transfer. The caller owns the
//! `Resource::Spim0` lease. Verified on board1 against an MPU-9250 over SPI.
#![no_std]

use embedded_hal::spi::{Error, ErrorKind, ErrorType, Operation, SpiDevice};
use nobro_hal::spim_hw::SPIM_XFER_MAX;
use nobro_hal::{BusError, Spim0};

/// Error wrapper so the HAL's `BusError` satisfies `embedded_hal::spi::Error`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct NobroSpiError(pub BusError);

impl Error for NobroSpiError {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Other
    }
}

fn spin(cycles: u32) {
    for _ in 0..cycles {
        cortex_m::asm::nop();
    }
}

/// `embedded-hal` SPI device backed by NobroRTOS SPIM0 with software chip-select.
pub struct NobroSpiDevice {
    spim: Spim0,
}

impl NobroSpiDevice {
    /// Configure SPIM0 on the given raw nRF pins (mode 3, software CS).
    ///
    /// # Safety
    /// The caller must own the `Resource::Spim0` lease; the pins must be the board's
    /// wired SPI pins.
    pub unsafe fn new(sck: u8, mosi: u8, miso: u8, cs: u8) -> Self {
        NobroSpiDevice {
            spim: Spim0::init(sck, mosi, miso, cs),
        }
    }
}

impl ErrorType for NobroSpiDevice {
    type Error = NobroSpiError;
}

impl SpiDevice<u8> for NobroSpiDevice {
    fn transaction(&mut self, operations: &mut [Operation<'_, u8>]) -> Result<(), Self::Error> {
        // One chip-select around the whole transaction, with the slave's setup/recovery
        // time (the MPU-9250 needs it between the address and data phases).
        self.spim.select();
        spin(2_000);
        let mut outcome: Result<(), BusError> = Ok(());
        for op in operations.iter_mut() {
            let r = match op {
                Operation::Read(buf) => {
                    let tx = [0u8; SPIM_XFER_MAX];
                    if buf.len() > SPIM_XFER_MAX {
                        Err(BusError::Timeout)
                    } else {
                        self.spim.transfer_held(&tx[..buf.len()], buf)
                    }
                }
                Operation::Write(buf) => {
                    let mut rx = [0u8; SPIM_XFER_MAX];
                    if buf.len() > SPIM_XFER_MAX {
                        Err(BusError::Timeout)
                    } else {
                        self.spim.transfer_held(buf, &mut rx[..buf.len()])
                    }
                }
                Operation::Transfer(read, write) => {
                    let n = read.len().max(write.len());
                    if n > SPIM_XFER_MAX {
                        Err(BusError::Timeout)
                    } else {
                        let mut tx = [0u8; SPIM_XFER_MAX];
                        let mut rx = [0u8; SPIM_XFER_MAX];
                        tx[..write.len()].copy_from_slice(write);
                        let res = self.spim.transfer_held(&tx[..n], &mut rx[..n]);
                        if res.is_ok() {
                            read.copy_from_slice(&rx[..read.len()]);
                        }
                        res
                    }
                }
                Operation::TransferInPlace(buf) => {
                    if buf.len() > SPIM_XFER_MAX {
                        Err(BusError::Timeout)
                    } else {
                        let mut tx = [0u8; SPIM_XFER_MAX];
                        let mut rx = [0u8; SPIM_XFER_MAX];
                        tx[..buf.len()].copy_from_slice(buf);
                        let res = self.spim.transfer_held(&tx[..buf.len()], &mut rx[..buf.len()]);
                        if res.is_ok() {
                            buf.copy_from_slice(&rx[..buf.len()]);
                        }
                        res
                    }
                }
                Operation::DelayNs(ns) => {
                    // ~64 MHz core: ~16 ns/cycle.
                    spin(*ns / 16 + 1);
                    Ok(())
                }
            };
            if let Err(e) = r {
                outcome = Err(e);
                break;
            }
        }
        spin(2_000);
        self.spim.deselect();
        spin(2_000);
        outcome.map_err(NobroSpiError)
    }
}
