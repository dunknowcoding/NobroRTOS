//! SAMD21 reset evidence and CPU-idle policy.
//!
//! The portable provider exposes only CPU sleep. It deliberately does not
//! advertise standby or deep sleep until every selected USB/SERCOM/EIC wake
//! source has a physically verified retention contract.

use portable_atomic::{AtomicU32, Ordering};

use nobro_hal::{HalPower, HalReset, IdleMode};

use crate::lease::{Samd21LeaseGuard, Samd21Leases, POWER_LEASE};

static VETOES: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerError {
    Lease(nobro_hal::LeaseError),
    InvalidVeto,
    Vetoed(u32),
    InterruptsMasked,
}

pub struct PowerVeto {
    mask: u32,
}

impl Drop for PowerVeto {
    fn drop(&mut self) {
        VETOES.fetch_and(!self.mask, Ordering::AcqRel);
    }
}

pub struct Samd21Power {
    _lease: Samd21LeaseGuard,
}

impl Samd21Power {
    pub fn try_new(owner: u8) -> Result<Self, PowerError> {
        Ok(Self {
            _lease: Samd21Leases::acquire_guard(POWER_LEASE, owner).map_err(PowerError::Lease)?,
        })
    }

    pub fn veto(reason: u8) -> Result<PowerVeto, PowerError> {
        if reason >= 32 {
            return Err(PowerError::InvalidVeto);
        }
        let mask = 1u32 << reason;
        VETOES.fetch_or(mask, Ordering::AcqRel);
        Ok(PowerVeto { mask })
    }

    pub fn active_vetoes() -> u32 {
        VETOES.load(Ordering::Acquire)
    }
}

impl HalPower for Samd21Power {
    type Error = PowerError;

    fn idle(&mut self, mode: IdleMode) -> Result<(), Self::Error> {
        self._lease.ensure_live().map_err(PowerError::Lease)?;
        let vetoes = Self::active_vetoes();
        if vetoes != 0 {
            return Err(PowerError::Vetoed(vetoes));
        }
        match mode {
            IdleMode::CpuSleep => {
                #[cfg(target_arch = "arm")]
                {
                    if cortex_m::register::primask::read().is_active() {
                        return Err(PowerError::InterruptsMasked);
                    }
                    cortex_m::asm::wfi();
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResetCause(u8);

impl ResetCause {
    pub const POWER_ON: u8 = 1 << 0;
    pub const BROWN_OUT_12: u8 = 1 << 1;
    pub const BROWN_OUT_33: u8 = 1 << 2;
    pub const EXTERNAL: u8 = 1 << 4;
    pub const WATCHDOG: u8 = 1 << 5;
    pub const SYSTEM: u8 = 1 << 6;
    pub const BACKUP: u8 = 1 << 7;

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, cause: u8) -> bool {
        self.0 & cause != 0
    }
}

pub struct Samd21Reset;

impl HalReset for Samd21Reset {
    type Cause = ResetCause;

    fn reset_cause() -> Self::Cause {
        #[cfg(target_arch = "arm")]
        unsafe {
            // PM.RCAUSE is an 8-bit reset-cause register at PM + 0x38.
            ResetCause((0x4000_0438 as *const u8).read_volatile())
        }
        #[cfg(not(target_arch = "arm"))]
        ResetCause(ResetCause::POWER_ON)
    }

    fn system_reset() -> ! {
        #[cfg(target_arch = "arm")]
        cortex_m::peripheral::SCB::sys_reset();
        #[cfg(not(target_arch = "arm"))]
        panic!("SAMD21 system reset is target-only")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn veto_lifetime_is_bounded() {
        let _lock = lock();
        assert_eq!(Samd21Power::active_vetoes(), 0);
        {
            let _veto = Samd21Power::veto(3).unwrap();
            assert_eq!(Samd21Power::active_vetoes(), 1 << 3);
        }
        assert_eq!(Samd21Power::active_vetoes(), 0);
    }

    #[test]
    fn cpu_idle_fails_closed_while_transport_is_active() {
        let _lock = lock();
        let mut power = Samd21Power::try_new(2).unwrap();
        let _veto = Samd21Power::veto(5).unwrap();
        assert_eq!(
            power.idle(IdleMode::CpuSleep),
            Err(PowerError::Vetoed(1 << 5))
        );
    }

    #[test]
    fn reset_cause_bits_remain_composable() {
        let cause = ResetCause(ResetCause::EXTERNAL | ResetCause::WATCHDOG);
        assert!(cause.contains(ResetCause::EXTERNAL));
        assert!(cause.contains(ResetCause::WATCHDOG));
        assert!(!cause.contains(ResetCause::SYSTEM));
    }
}
