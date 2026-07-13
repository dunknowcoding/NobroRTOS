//! Framed radio comms as a managed `nobro-wireless` backend over the nRF RADIO.
//!
//! Creating a `RadioComms` takes the `Resource::Radio` exclusive lease, so the kernel
//! arbitrates radio ownership exactly like TWIM/SPIM; dropping/`release`-ing returns it.
//! The wireless domain supplies deadline/budget accounting around HAL send/recv. This closes the
//! M26 radio's integration into NobroRTOS's resource management (lease + SAL trait), and
//! pairs with `Capability::Radio` for capability-gated access.
#![no_std]

use nobro_hal::{lease::LeaseError, RadioSession};
use nobro_wireless::{LinkDescriptor, LinkState, Protocol, WirelessBackend};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RadioCommsError {
    /// The radio lease could not be acquired/released (e.g. already held).
    Lease(LeaseError),
}

/// Owns the `Resource::Radio` lease and mounts it as a wireless backend.
pub struct RadioComms {
    radio: RadioSession,
}

impl RadioComms {
    /// Acquire the radio as a managed resource (takes the `Resource::Radio` lease),
    /// then bring up the RADIO peripheral. Fails if another owner holds the radio.
    pub fn acquire(owner: u8) -> Result<Self, RadioCommsError> {
        let radio = unsafe { RadioSession::acquire(owner) }.map_err(RadioCommsError::Lease)?;
        Ok(RadioComms { radio })
    }

    /// Release the radio lease back to the kernel.
    pub fn release(self) -> Result<(), RadioCommsError> {
        drop(self);
        Ok(())
    }
}

impl WirelessBackend for RadioComms {
    fn descriptor(&self) -> LinkDescriptor {
        LinkDescriptor {
            name: "nRF proprietary managed radio",
            protocol: Protocol::Proprietary,
            mtu: 32,
            requires_join: false,
            broadcast_only: true,
        }
    }

    fn link_state(&mut self) -> LinkState {
        LinkState::Up
    }

    fn send(&mut self, payload: &[u8]) -> bool {
        payload.len() <= 32 && self.radio.send(payload).is_ok()
    }

    fn recv(&mut self, destination: &mut [u8]) -> usize {
        self.radio
            .recv(destination, 200_000)
            .ok()
            .flatten()
            .unwrap_or(0)
    }
}
