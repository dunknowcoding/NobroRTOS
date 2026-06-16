//! TWIM bus stub with ResourceLease (ArduinoNRF TWIM0 @ 0x40003000).

use crate::lease::{LeaseError, Resource, ResourceLease};

pub const TWIM0_BASE: u32 = 0x4000_3000;
pub const TWIM1_BASE: u32 = 0x4000_4000;
pub const SPIM0_BASE: u32 = 0x4000_3000; // shared IRQ block with TWIM0 on nRF52

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusError {
    LeaseDenied,
    Timeout,
}

pub struct TwimBus {
    owner: u8,
    resource: Resource,
}

impl TwimBus {
    pub fn new_twim0(owner: u8) -> Result<Self, LeaseError> {
        ResourceLease::acquire(Resource::Twim0, owner)?;
        Ok(Self {
            owner,
            resource: Resource::Twim0,
        })
    }

    /// Stub read — exercises lease + simulates 32-byte burst cap.
    pub fn read_stub(&self, addr: u8, buf: &mut [u8]) -> Result<(), BusError> {
        if buf.len() > 32 {
            return Err(BusError::Timeout);
        }
        for (i, b) in buf.iter_mut().enumerate() {
            *b = addr.wrapping_add(i as u8);
        }
        Ok(())
    }
}

impl Drop for TwimBus {
    fn drop(&mut self) {
        let _ = ResourceLease::release(self.resource, self.owner);
    }
}
