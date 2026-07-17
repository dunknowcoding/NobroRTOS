//! TWIM bus with ResourceLease (ArduinoNRF TWIM0 @ 0x40003000).

use crate::lease::{LeaseError, LeaseGuard, Resource, ResourceLease};
#[cfg(feature = "nrf-twim-async")]
use crate::priority_ceiling::CompletionInterruptPriority;
use crate::twim_hw::Twim0;

pub const TWIM0_BASE: u32 = 0x4000_3000;
pub const TWIM1_BASE: u32 = 0x4000_4000;
pub const SPIM0_BASE: u32 = 0x4000_3000; // shared IRQ block with TWIM0 on nRF52

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusError {
    LeaseDenied,
    Busy,
    Timeout,
    Nack,
    LengthMismatch,
    InvalidAddress,
    EmptyTransfer,
    TransferTooLong,
}

pub struct TwimBus {
    lease: LeaseGuard,
    #[cfg(feature = "nrf-twim-async")]
    interrupt_priority: CompletionInterruptPriority,
}

impl TwimBus {
    pub fn new_twim0(owner: u8) -> Result<Self, LeaseError> {
        #[cfg(feature = "nrf-twim-async")]
        {
            Self::new_twim0_with_priority(owner, CompletionInterruptPriority::board_default())
        }
        #[cfg(not(feature = "nrf-twim-async"))]
        {
            Ok(Self {
                lease: ResourceLease::acquire_guard(Resource::Twim0, owner)?,
            })
        }
    }

    /// Acquire TWIM0 with the completion-ISR priority admitted for its reactor
    /// domain.
    #[cfg(feature = "nrf-twim-async")]
    pub fn new_twim0_with_priority(
        owner: u8,
        interrupt_priority: CompletionInterruptPriority,
    ) -> Result<Self, LeaseError> {
        Ok(Self {
            lease: ResourceLease::acquire_guard(Resource::Twim0, owner)?,
            interrupt_priority,
        })
    }

    #[cfg(feature = "nrf-twim-async")]
    pub const fn interrupt_priority(&self) -> CompletionInterruptPriority {
        self.interrupt_priority
    }

    pub fn init_pins(&self, sda: u8, scl: u8) -> Result<(), BusError> {
        self.ensure_sync_ready()?;
        unsafe { Self::init_pins_unchecked(sda, scl) }
        Ok(())
    }

    /// Legacy integration boundary for runtimes that hold a lease outside this type.
    ///
    /// # Safety
    /// The caller must prove a live TWIM0 lease and prevent recovery/reassignment for
    /// the duration of every operation. New Rust adapters must use [`Self::init_pins`].
    pub unsafe fn init_pins_unchecked(sda: u8, scl: u8) {
        Twim0::init(sda, scl);
    }

    pub fn probe(&self, addr: u8) -> Result<bool, BusError> {
        self.ensure_sync_ready()?;
        Self::ensure_address(addr)?;
        Ok(unsafe { Twim0::probe(addr) })
    }

    pub fn scan<F: FnMut(u8)>(&self, found: F) -> Result<u8, BusError> {
        self.ensure_sync_ready()?;
        Ok(unsafe { Twim0::scan(found) })
    }

    pub fn read_reg(&self, addr: u8, reg: u8) -> Result<u8, BusError> {
        self.ensure_sync_ready()?;
        Self::ensure_address(addr)?;
        unsafe { Twim0::read_reg(addr, reg) }.map_err(|e| match e {
            BusError::Timeout => BusError::Nack,
            other => other,
        })
    }

    pub fn write_reg(&self, addr: u8, reg: u8, val: u8) -> Result<(), BusError> {
        self.ensure_sync_ready()?;
        Self::ensure_address(addr)?;
        unsafe { Twim0::write_reg(addr, reg, val) }.map_err(|e| match e {
            BusError::Timeout => BusError::Nack,
            other => other,
        })
    }

    pub fn write_read(&self, addr: u8, tx: &[u8], rx: &mut [u8]) -> Result<(), BusError> {
        self.ensure_sync_ready()?;
        Self::ensure_address(addr)?;
        unsafe { Twim0::write_read(addr, tx, rx) }.map_err(|e| match e {
            BusError::Timeout => BusError::Nack,
            other => other,
        })
    }

    pub fn write(&self, addr: u8, bytes: &[u8]) -> Result<(), BusError> {
        self.ensure_sync_ready()?;
        Self::ensure_address(addr)?;
        unsafe { Twim0::write_bytes(addr, bytes) }.map_err(|error| match error {
            BusError::Timeout => BusError::Nack,
            other => other,
        })
    }

    pub fn read(&self, addr: u8, bytes: &mut [u8]) -> Result<(), BusError> {
        self.ensure_sync_ready()?;
        Self::ensure_address(addr)?;
        unsafe { Twim0::read_bytes(addr, bytes) }.map_err(|error| match error {
            BusError::Timeout => BusError::Nack,
            other => other,
        })
    }

    /// Stub read kept for Phase 1 lease demo compatibility.
    pub fn read_stub(&self, addr: u8, buf: &mut [u8]) -> Result<(), BusError> {
        self.ensure_live()?;
        Self::ensure_address(addr)?;
        if buf.len() > 32 {
            return Err(BusError::TransferTooLong);
        }
        for (i, b) in buf.iter_mut().enumerate() {
            *b = addr.wrapping_add(i as u8);
        }
        Ok(())
    }

    pub(crate) fn ensure_live(&self) -> Result<(), BusError> {
        self.lease.ensure_live().map_err(|_| BusError::LeaseDenied)
    }

    fn ensure_sync_ready(&self) -> Result<(), BusError> {
        self.ensure_live()?;
        if crate::twim_hw::async_busy() {
            Err(BusError::Busy)
        } else {
            Ok(())
        }
    }

    fn ensure_address(address: u8) -> Result<(), BusError> {
        if address <= 0x7f {
            Ok(())
        } else {
            Err(BusError::InvalidAddress)
        }
    }
}
