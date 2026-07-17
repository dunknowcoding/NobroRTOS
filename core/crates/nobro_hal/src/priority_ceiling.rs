//! nRF52840 BASEPRI ceiling sections.
//!
//! Deadline/watchdog and P-ISR domains above the ceiling remain serviceable.
//! They may touch only lock-free handoff state; shared kernel structures remain
//! below the ceiling. On S140, the ceiling masks only application priorities 6
//! and 7, leaving every SoftDevice-reserved priority untouched.

// The target package, rather than each application, supplies the process-wide
// critical-section implementation. This makes every `critical_section::with`
// user in the final nRF image (kernel, HAL, USB, portable-atomic, and adapters)
// share the same BASEPRI contract. Interrupts above the selected ceiling must
// use only lock-free handoff state; admitting such an ISR while it touches a
// critical-section mutex would violate the implementation's safety contract.
#[cfg(all(target_arch = "arm", target_has_atomic = "32"))]
mod implementation {
    use critical_section::{set_impl, Impl, RawRestoreState};

    struct Nrf52840PriorityCeiling;
    set_impl!(Nrf52840PriorityCeiling);

    #[cfg(feature = "board-promicro-nosd")]
    const RAW_CEILING: u8 = super::PriorityCeiling::NRF52840_BARE.raw();
    #[cfg(feature = "board-nicenano-s140")]
    const RAW_CEILING: u8 = super::PriorityCeiling::NRF52840_S140.raw();

    unsafe impl Impl for Nrf52840PriorityCeiling {
        unsafe fn acquire() -> RawRestoreState {
            let previous = cortex_m::register::basepri::read();
            // Every Nobro-owned BASEPRI section uses the board ceiling. A
            // nonzero entry is therefore already nested under an equal or
            // stricter mask and must be left untouched so bool restore state
            // remains compatible with the ecosystem's standard CS ABI.
            if previous == 0 {
                cortex_m::register::basepri::write(RAW_CEILING);
            }
            cortex_m::asm::dmb();
            previous == 0
        }

        unsafe fn release(was_unmasked: RawRestoreState) {
            cortex_m::asm::dmb();
            if was_unmasked {
                cortex_m::register::basepri::write(0);
                cortex_m::asm::isb();
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriorityCeilingError {
    InvalidLogicalPriority,
    DeadlineWouldBeMasked,
    WatchdogWouldBeMasked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionInterruptPriorityError {
    InvalidLogicalPriority,
    WouldPreemptCriticalSection,
}

/// Validated NVIC priority for an ISR that uses [`CompletionCell`](crate::CompletionCell).
///
/// Completion ISRs briefly enter the process-wide critical section to publish
/// and take a task waker. They must therefore run at or below the board's
/// BASEPRI ceiling (numerically equal to or greater than it). Deadline and
/// watchdog priorities above that ceiling remain reserved for lock-free
/// handoffs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionInterruptPriority {
    logical: u8,
}

impl CompletionInterruptPriority {
    const PRIORITY_LEVELS: u8 = 8;
    const SHIFT: u8 = 5;

    #[cfg(feature = "board-promicro-nosd")]
    const MIN_LOGICAL: u8 = 3;
    #[cfg(feature = "board-nicenano-s140")]
    const MIN_LOGICAL: u8 = 6;

    pub const fn new(logical: u8) -> Result<Self, CompletionInterruptPriorityError> {
        if logical >= Self::PRIORITY_LEVELS {
            return Err(CompletionInterruptPriorityError::InvalidLogicalPriority);
        }
        if logical < Self::MIN_LOGICAL {
            return Err(CompletionInterruptPriorityError::WouldPreemptCriticalSection);
        }
        Ok(Self { logical })
    }

    /// Board-safe default used by compatibility constructors.
    pub const fn board_default() -> Self {
        Self {
            logical: Self::MIN_LOGICAL,
        }
    }

    pub const fn logical(self) -> u8 {
        self.logical
    }

    pub(crate) const fn raw(self) -> u8 {
        self.logical << Self::SHIFT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PriorityCeiling {
    raw: u8,
}

impl PriorityCeiling {
    const PRIORITY_LEVELS: u8 = 8;
    const SHIFT: u8 = 5;

    pub const fn new(
        ceiling: u8,
        deadline_priority: u8,
        watchdog_priority: u8,
    ) -> Result<Self, PriorityCeilingError> {
        if ceiling == 0
            || ceiling >= Self::PRIORITY_LEVELS
            || deadline_priority >= Self::PRIORITY_LEVELS
            || watchdog_priority >= Self::PRIORITY_LEVELS
        {
            return Err(PriorityCeilingError::InvalidLogicalPriority);
        }
        if deadline_priority >= ceiling {
            return Err(PriorityCeilingError::DeadlineWouldBeMasked);
        }
        if watchdog_priority >= ceiling {
            return Err(PriorityCeilingError::WatchdogWouldBeMasked);
        }
        Ok(Self {
            raw: ceiling << Self::SHIFT,
        })
    }

    /// No-SoftDevice profile: priorities 0/1 remain live; 3-7 are masked.
    pub const NRF52840_BARE: Self = Self {
        raw: 3 << Self::SHIFT,
    };
    /// S140 profile: application deadline/watchdog priorities 2/3 and all
    /// SoftDevice priorities remain live; only application 6/7 are masked.
    pub const NRF52840_S140: Self = Self {
        raw: 6 << Self::SHIFT,
    };

    pub const fn raw(self) -> u8 {
        self.raw
    }

    /// Execute bounded shared-state work under BASEPRI. Nested ceilings retain
    /// the stricter existing mask and restore it on every exit path.
    pub fn with<R>(self, operation: impl FnOnce() -> R) -> R {
        let previous = cortex_m::register::basepri::read();
        let effective = if previous == 0 {
            self.raw
        } else {
            previous.min(self.raw)
        };
        unsafe {
            cortex_m::register::basepri::write(effective);
        }
        cortex_m::asm::dmb();
        struct Restore(u8);
        impl Drop for Restore {
            fn drop(&mut self) {
                cortex_m::asm::dmb();
                unsafe {
                    cortex_m::register::basepri::write(self.0);
                }
                cortex_m::asm::isb();
            }
        }
        let restore = Restore(previous);
        let result = operation();
        drop(restore);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contracts_never_mask_deadline_or_watchdog_priority() {
        assert_eq!(PriorityCeiling::new(3, 0, 1).unwrap().raw(), 3 << 5);
        assert_eq!(
            PriorityCeiling::new(3, 3, 1),
            Err(PriorityCeilingError::DeadlineWouldBeMasked)
        );
        assert_eq!(
            PriorityCeiling::new(6, 2, 6),
            Err(PriorityCeilingError::WatchdogWouldBeMasked)
        );
        assert_eq!(PriorityCeiling::NRF52840_S140.raw(), 6 << 5);
    }

    #[test]
    fn completion_priorities_cannot_preempt_shared_waker_state() {
        #[cfg(feature = "board-promicro-nosd")]
        {
            assert_eq!(
                CompletionInterruptPriority::new(2),
                Err(CompletionInterruptPriorityError::WouldPreemptCriticalSection)
            );
            assert_eq!(CompletionInterruptPriority::new(3).unwrap().logical(), 3);
            assert_eq!(CompletionInterruptPriority::board_default().logical(), 3);
        }
        #[cfg(feature = "board-nicenano-s140")]
        {
            assert_eq!(
                CompletionInterruptPriority::new(5),
                Err(CompletionInterruptPriorityError::WouldPreemptCriticalSection)
            );
            assert_eq!(CompletionInterruptPriority::new(6).unwrap().logical(), 6);
            assert_eq!(CompletionInterruptPriority::board_default().logical(), 6);
        }
        assert_eq!(
            CompletionInterruptPriority::new(8),
            Err(CompletionInterruptPriorityError::InvalidLogicalPriority)
        );
    }
}
