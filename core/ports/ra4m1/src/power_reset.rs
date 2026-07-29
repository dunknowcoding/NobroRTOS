//! RA4M1 reset status and state-preserving CPU sleep.
//!
//! This provider deliberately exposes ordinary CPU sleep only. Standby and deep
//! standby have different clock/retention contracts and must not be reached
//! through a generic idle path.

use core::sync::atomic::{AtomicU32, Ordering};

use nobro_hal::{HalPower, HalReset, IdleMode, LeaseError, LeaseId};

use crate::lease::{Ra4m1LeaseGuard, Ra4m1Leases};

static VETO_MASK: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerError {
    Lease(LeaseError),
    InvalidReason,
    Vetoed(u32),
    InterruptsMasked,
}

pub struct Ra4m1Power {
    _lease: Ra4m1LeaseGuard,
}

impl Ra4m1Power {
    pub fn try_new(owner: u8) -> Result<Self, PowerError> {
        let lease =
            Ra4m1Leases::acquire_guard(LeaseId::SYSTEM_POWER, owner).map_err(PowerError::Lease)?;
        Ok(Self { _lease: lease })
    }

    /// Prevent idle while one exact controller requires active servicing.
    ///
    /// Reasons are caller-owned bits so USB, a bus transfer, and a controller
    /// lifecycle can coexist without a central driver-name registry.
    pub fn veto(reason_bit: u8) -> Result<PowerVeto, PowerError> {
        if reason_bit >= 32 {
            return Err(PowerError::InvalidReason);
        }
        let mask = 1u32 << reason_bit;
        let previous = VETO_MASK.fetch_or(mask, Ordering::AcqRel);
        if previous & mask != 0 {
            return Err(PowerError::Vetoed(mask));
        }
        Ok(PowerVeto { mask, active: true })
    }

    pub fn veto_mask() -> u32 {
        VETO_MASK.load(Ordering::Acquire)
    }
}

impl HalPower for Ra4m1Power {
    type Error = PowerError;

    fn idle(&mut self, mode: IdleMode) -> Result<(), Self::Error> {
        self._lease.ensure_live().map_err(PowerError::Lease)?;
        let vetoes = Self::veto_mask();
        if vetoes != 0 {
            return Err(PowerError::Vetoed(vetoes));
        }
        #[cfg(target_arch = "arm")]
        {
            if cortex_m::register::primask::read().is_inactive()
                || cortex_m::register::faultmask::read().is_inactive()
            {
                return Err(PowerError::InterruptsMasked);
            }
            match mode {
                IdleMode::CpuSleep => unsafe {
                    // SSBY=0 selects ordinary sleep. SLEEPDEEP=0 prevents a
                    // stale debugger/application bit from escalating WFI.
                    let sbycr = read16(SBYCR) & !(1 << 15);
                    write16(SBYCR, sbycr);
                    let scr = read32(SCB_SCR) & !(1 << 2);
                    write32(SCB_SCR, scr);
                    cortex_m::asm::dsb();
                    cortex_m::asm::wfi();
                    cortex_m::asm::isb();
                },
            }
        }
        #[cfg(not(target_arch = "arm"))]
        let _ = mode;
        Ok(())
    }
}

pub struct PowerVeto {
    mask: u32,
    active: bool,
}

impl PowerVeto {
    pub fn release(mut self) {
        if self.active {
            VETO_MASK.fetch_and(!self.mask, Ordering::AcqRel);
            self.active = false;
        }
    }
}

impl Drop for PowerVeto {
    fn drop(&mut self) {
        if self.active {
            VETO_MASK.fetch_and(!self.mask, Ordering::AcqRel);
            self.active = false;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimaryResetCause {
    PowerOn,
    DeepStandby,
    Software,
    Watchdog,
    Voltage,
    Clock,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResetCause {
    pub status0: u8,
    pub status1: u16,
    pub status2: u8,
}

impl ResetCause {
    pub const fn primary(self) -> PrimaryResetCause {
        if self.status0 & 1 != 0 {
            PrimaryResetCause::PowerOn
        } else if self.status0 & (1 << 7) != 0 {
            PrimaryResetCause::DeepStandby
        } else if self.status1 & (1 << 2) != 0 {
            PrimaryResetCause::Software
        } else if self.status1 & 0x0003 != 0 {
            PrimaryResetCause::Watchdog
        } else if self.status0 & 0x000e != 0 {
            PrimaryResetCause::Voltage
        } else if self.status2 & 1 != 0 {
            PrimaryResetCause::Clock
        } else {
            PrimaryResetCause::Other
        }
    }
}

pub struct Ra4m1Reset;

impl HalReset for Ra4m1Reset {
    type Cause = ResetCause;

    fn reset_cause() -> Self::Cause {
        #[cfg(target_arch = "arm")]
        unsafe {
            return ResetCause {
                status0: read8(RSTSR0),
                status1: read16(RSTSR1),
                status2: read8(RSTSR2),
            };
        }
        #[cfg(not(target_arch = "arm"))]
        ResetCause {
            status0: 0,
            status1: 0,
            status2: 0,
        }
    }

    fn system_reset() -> ! {
        #[cfg(target_arch = "arm")]
        cortex_m::peripheral::SCB::sys_reset();
        #[cfg(not(target_arch = "arm"))]
        panic!("RA4M1 reset is unavailable on the host");
    }
}

#[cfg(target_arch = "arm")]
const SBYCR: usize = 0x4001_E00c;
#[cfg(target_arch = "arm")]
const RSTSR1: usize = 0x4001_E0c0;
#[cfg(target_arch = "arm")]
const RSTSR0: usize = 0x4001_E410;
#[cfg(target_arch = "arm")]
const RSTSR2: usize = 0x4001_E411;
#[cfg(target_arch = "arm")]
const SCB_SCR: usize = 0xE000_ED10;

#[cfg(target_arch = "arm")]
unsafe fn read8(address: usize) -> u8 {
    (address as *const u8).read_volatile()
}

#[cfg(target_arch = "arm")]
unsafe fn read16(address: usize) -> u16 {
    (address as *const u16).read_volatile()
}

#[cfg(target_arch = "arm")]
unsafe fn write16(address: usize, value: u16) {
    (address as *mut u16).write_volatile(value);
}

#[cfg(target_arch = "arm")]
unsafe fn read32(address: usize) -> u32 {
    (address as *const u32).read_volatile()
}

#[cfg(target_arch = "arm")]
unsafe fn write32(address: usize, value: u32) {
    (address as *mut u32).write_volatile(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vetoes_compose_and_release_independently() {
        let _lock = crate::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(Ra4m1Power::veto_mask(), 0);
        let usb = Ra4m1Power::veto(0).unwrap();
        let controller = Ra4m1Power::veto(4).unwrap();
        assert_eq!(Ra4m1Power::veto_mask(), 0x11);
        assert_eq!(Ra4m1Power::veto(4).err(), Some(PowerError::Vetoed(1 << 4)));
        assert_eq!(Ra4m1Power::veto(32).err(), Some(PowerError::InvalidReason));
        drop(usb);
        assert_eq!(Ra4m1Power::veto_mask(), 0x10);
        controller.release();
        assert_eq!(Ra4m1Power::veto_mask(), 0);
    }

    #[test]
    fn power_provider_rejects_sleep_while_any_owner_vetoes() {
        let _lock = crate::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut power = Ra4m1Power::try_new(71).unwrap();
        let veto = Ra4m1Power::veto(7).unwrap();
        assert_eq!(
            power.idle(IdleMode::CpuSleep),
            Err(PowerError::Vetoed(1 << 7))
        );
        drop(veto);
        assert_eq!(power.idle(IdleMode::CpuSleep), Ok(()));
    }

    #[test]
    fn reset_causes_have_deterministic_precedence() {
        let _lock = crate::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cause = ResetCause {
            status0: 1,
            status1: 1 << 2,
            status2: 1,
        };
        assert_eq!(cause.primary(), PrimaryResetCause::PowerOn);
        let cause = ResetCause {
            status0: 0,
            status1: 1 << 2,
            status2: 1,
        };
        assert_eq!(cause.primary(), PrimaryResetCause::Software);
        let cause = ResetCause {
            status0: 0,
            status1: 0,
            status2: 1,
        };
        assert_eq!(cause.primary(), PrimaryResetCause::Clock);
    }
}
