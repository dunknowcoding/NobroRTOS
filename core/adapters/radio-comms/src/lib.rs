//! Framed radio comms as a **managed resource** (`StreamSal` over the nRF RADIO).
//!
//! Creating a `RadioComms` takes the `Resource::Radio` exclusive lease, so the kernel
//! arbitrates radio ownership exactly like TWIM/SPIM; dropping/`release`-ing returns it.
//! `write_frame`/`read_frame`/`poll` map onto the radio HAL's send/recv. This closes the
//! M26 radio's integration into NobroRTOS's resource management (lease + SAL trait), and
//! pairs with `Capability::Radio` for capability-gated access.
#![no_std]

use nobro_hal::{
    lease::{LeaseError, Resource},
    traits::HalLease,
    ActivePlatform as Hal, Radio,
};
use nobro_sal::StreamSal;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RadioCommsError {
    /// The radio lease could not be acquired/released (e.g. already held).
    Lease(LeaseError),
    /// The radio did not complete a transmission within its window.
    TxTimeout,
}

/// Owns the `Resource::Radio` lease and frames the radio as a `StreamSal`.
pub struct RadioComms {
    owner: u8,
    rx: [u8; 32],
    rx_len: usize,
}

impl RadioComms {
    /// Acquire the radio as a managed resource (takes the `Resource::Radio` lease),
    /// then bring up the RADIO peripheral. Fails if another owner holds the radio.
    pub fn acquire(owner: u8) -> Result<Self, RadioCommsError> {
        Hal::acquire(Resource::Radio, owner).map_err(RadioCommsError::Lease)?;
        unsafe {
            Radio::init();
        }
        Ok(RadioComms {
            owner,
            rx: [0; 32],
            rx_len: 0,
        })
    }

    /// Release the radio lease back to the kernel.
    pub fn release(self) -> Result<(), RadioCommsError> {
        Hal::release(Resource::Radio, self.owner).map_err(RadioCommsError::Lease)
    }
}

impl StreamSal for RadioComms {
    type Error = RadioCommsError;

    fn poll(&mut self) -> Result<Option<usize>, Self::Error> {
        match Radio::recv(&mut self.rx, 50_000) {
            Some(n) => {
                self.rx_len = n;
                Ok(Some(n))
            }
            None => Ok(None),
        }
    }

    fn read_frame(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
        if self.rx_len > 0 {
            let n = self.rx_len.min(buf.len());
            buf[..n].copy_from_slice(&self.rx[..n]);
            self.rx_len = 0;
            return Ok(Some(n));
        }
        Ok(Radio::recv(buf, 200_000))
    }

    fn write_frame(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        Radio::send(buf).map_err(|_| RadioCommsError::TxTimeout)
    }
}
