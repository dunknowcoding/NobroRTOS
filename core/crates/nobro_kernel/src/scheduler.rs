//! Deadline slot scheduler (Phase 1): TIMER1 drives 50 Hz hard-real-time ticks.

use core::cell::Cell;

use critical_section::Mutex;
use portable_atomic::{AtomicU32, Ordering};

use crate::KernelError;

/// Default interval between deadline ticks (50 Hz servo loop).
pub const DEADLINE_PERIOD_US: u64 = 20_000;
pub const DEFAULT_JITTER_TOLERANCE_US: u32 = 10;

static EXPECTED_NEXT_US: AtomicU32 = AtomicU32::new(0);
static MAX_JITTER_US: AtomicU32 = AtomicU32::new(0);
static TICK_COUNT: AtomicU32 = AtomicU32::new(0);
static DEADLINE_MISSES: AtomicU32 = AtomicU32::new(0);
static JITTER_TOLERANCE_US: AtomicU32 = AtomicU32::new(DEFAULT_JITTER_TOLERANCE_US);
static TICK_PERIOD_US: AtomicU32 = AtomicU32::new(DEADLINE_PERIOD_US as u32);
static STATS_SEQUENCE: AtomicU32 = AtomicU32::new(0);

pub type DeadlineHandler = fn();

static DEADLINE_HANDLER: Mutex<Cell<Option<DeadlineHandler>>> = Mutex::new(Cell::new(None));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeadlineHandlerError {
    AlreadyRegistered,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SchedulerStats {
    pub tick_count: u32,
    pub max_jitter_us: u32,
    pub deadline_misses: u32,
    pub jitter_tolerance_us: u32,
}

pub struct Scheduler;

impl Scheduler {
    /// # Safety
    /// Must be set before the scheduler tick starts; the handler runs in interrupt
    /// context and must be interrupt-safe.
    pub unsafe fn set_deadline_handler(
        handler: DeadlineHandler,
    ) -> Result<(), DeadlineHandlerError> {
        critical_section::with(|cs| {
            let slot = DEADLINE_HANDLER.borrow(cs);
            if slot.get().is_some() {
                Err(DeadlineHandlerError::AlreadyRegistered)
            } else {
                slot.set(Some(handler));
                Ok(())
            }
        })
    }

    /// # Safety
    /// The caller must first quiesce the timer interrupt and ensure the previous
    /// handler has returned.
    pub unsafe fn clear_deadline_handler() {
        critical_section::with(|cs| DEADLINE_HANDLER.borrow(cs).set(None));
    }

    pub fn reset_stats() {
        STATS_SEQUENCE.fetch_add(1, Ordering::AcqRel);
        EXPECTED_NEXT_US.store(0, Ordering::Release);
        MAX_JITTER_US.store(0, Ordering::Release);
        TICK_COUNT.store(0, Ordering::Release);
        DEADLINE_MISSES.store(0, Ordering::Release);
        JITTER_TOLERANCE_US.store(DEFAULT_JITTER_TOLERANCE_US, Ordering::Release);
        TICK_PERIOD_US.store(DEADLINE_PERIOD_US as u32, Ordering::Release);
        STATS_SEQUENCE.fetch_add(1, Ordering::Release);
    }

    pub fn max_jitter_us() -> u32 {
        MAX_JITTER_US.load(Ordering::Acquire)
    }

    pub fn tick_count() -> u32 {
        TICK_COUNT.load(Ordering::Acquire)
    }

    pub fn deadline_misses() -> u32 {
        DEADLINE_MISSES.load(Ordering::Acquire)
    }

    pub fn jitter_tolerance_us() -> u32 {
        JITTER_TOLERANCE_US.load(Ordering::Acquire)
    }

    pub fn set_jitter_tolerance_us(tolerance_us: u32) {
        STATS_SEQUENCE.fetch_add(1, Ordering::AcqRel);
        JITTER_TOLERANCE_US.store(tolerance_us, Ordering::Release);
        STATS_SEQUENCE.fetch_add(1, Ordering::Release);
    }

    pub fn tick_period_us() -> u32 {
        TICK_PERIOD_US.load(Ordering::Acquire)
    }

    /// Configure the deadline cadence (SCH-05). Must match the reprogrammed
    /// hardware timer compare interval; call with the tick source quiesced,
    /// then `reset_stats()` so jitter is measured against the new phase.
    /// Zero periods are rejected (the previous cadence is kept).
    pub fn set_tick_period_us(period_us: u32) -> bool {
        if period_us == 0 {
            return false;
        }
        STATS_SEQUENCE.fetch_add(1, Ordering::AcqRel);
        TICK_PERIOD_US.store(period_us, Ordering::Release);
        EXPECTED_NEXT_US.store(0, Ordering::Release);
        STATS_SEQUENCE.fetch_add(1, Ordering::Release);
        true
    }

    pub fn stats() -> SchedulerStats {
        loop {
            let before = STATS_SEQUENCE.load(Ordering::Acquire);
            if before & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }
            let stats = SchedulerStats {
                tick_count: Self::tick_count(),
                max_jitter_us: Self::max_jitter_us(),
                deadline_misses: Self::deadline_misses(),
                jitter_tolerance_us: Self::jitter_tolerance_us(),
            };
            let after = STATS_SEQUENCE.load(Ordering::Acquire);
            if before == after {
                return stats;
            }
        }
    }

    /// Called from TIMER1 ISR or polled compare handler.
    pub fn on_deadline_tick(now_us: u64) {
        STATS_SEQUENCE.fetch_add(1, Ordering::AcqRel);
        let now_lo = now_us as u32;
        let expected = EXPECTED_NEXT_US.load(Ordering::Acquire);
        if expected != 0 {
            let late = now_lo.wrapping_sub(expected);
            let early = expected.wrapping_sub(now_lo);
            let jitter = late.min(early);
            MAX_JITTER_US.fetch_max(jitter, Ordering::AcqRel);
            if jitter > JITTER_TOLERANCE_US.load(Ordering::Acquire) {
                DEADLINE_MISSES.fetch_add(1, Ordering::AcqRel);
            }
        }
        let period = TICK_PERIOD_US.load(Ordering::Acquire);
        let next = if expected == 0 {
            now_lo.wrapping_add(period)
        } else {
            expected.wrapping_add(period)
        };
        EXPECTED_NEXT_US.store(next, Ordering::Release);
        TICK_COUNT.fetch_add(1, Ordering::AcqRel);
        STATS_SEQUENCE.fetch_add(1, Ordering::Release);

        let handler = critical_section::with(|cs| DEADLINE_HANDLER.borrow(cs).get());
        if let Some(handler) = handler {
            handler();
        }
    }

    pub fn note_error(err: KernelError) -> crate::Action {
        default_action(&err)
    }
}

/// Cooperative async sleep helper (Embassy-style subset).
pub struct Timer {
    deadline_us: u64,
}

impl Timer {
    pub fn after_us(us: u64, now_us: u64) -> Self {
        Self {
            deadline_us: now_us.saturating_add(us),
        }
    }

    pub fn after_ms(ms: u64, now_us: u64) -> Self {
        Self::after_us(ms.saturating_mul(1000), now_us)
    }

    pub fn is_ready(&self, now_us: u64) -> bool {
        now_us >= self.deadline_us
    }
}

pub fn default_action(err: &KernelError) -> crate::Action {
    use crate::Action::*;
    match err {
        KernelError::LeaseConflict => Ignore,
        KernelError::BusTimeout => RetryDelay(1000),
        KernelError::RadioTxFail => RetryDelay(1000),
        KernelError::SensorReadFail => Ignore,
        KernelError::DeadlineMissed => NotifyUserTask,
        KernelError::ForeignModuleInitFail | KernelError::ForeignModulePollFail => RebootModule,
        // Memory-safety violations mean the module's state cannot be trusted:
        // restart it through recovery, never resume in place.
        KernelError::StackViolation | KernelError::MemoryFault => RebootModule,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_clears_expected_deadline() {
        Scheduler::reset_stats();
        Scheduler::on_deadline_tick(1_000);
        Scheduler::on_deadline_tick(1_000 + DEADLINE_PERIOD_US + 7);
        assert_eq!(Scheduler::tick_count(), 2);
        assert_eq!(Scheduler::max_jitter_us(), 7);

        Scheduler::reset_stats();
        Scheduler::on_deadline_tick(500_000);
        assert_eq!(Scheduler::tick_count(), 1);
        assert_eq!(Scheduler::max_jitter_us(), 0);
        assert_eq!(Scheduler::deadline_misses(), 0);
        assert_eq!(
            Scheduler::jitter_tolerance_us(),
            DEFAULT_JITTER_TOLERANCE_US
        );
    }

    #[test]
    fn jitter_handles_u32_wraparound() {
        Scheduler::reset_stats();
        let first = u32::MAX as u64 - 5;
        Scheduler::on_deadline_tick(first);
        Scheduler::on_deadline_tick(first + DEADLINE_PERIOD_US + 3);
        assert_eq!(Scheduler::max_jitter_us(), 3);
    }

    #[test]
    fn jitter_tolerance_is_configurable() {
        Scheduler::reset_stats();
        Scheduler::set_jitter_tolerance_us(25);
        Scheduler::on_deadline_tick(1_000);
        Scheduler::on_deadline_tick(1_000 + DEADLINE_PERIOD_US + 20);
        Scheduler::on_deadline_tick(1_000 + DEADLINE_PERIOD_US * 2 + 50);

        let stats = Scheduler::stats();
        assert_eq!(stats.tick_count, 3);
        assert_eq!(stats.max_jitter_us, 50);
        assert_eq!(stats.deadline_misses, 1);
        assert_eq!(stats.jitter_tolerance_us, 25);
    }

    #[test]
    fn late_tick_does_not_shift_the_periodic_phase() {
        Scheduler::reset_stats();
        Scheduler::on_deadline_tick(1_000);
        Scheduler::on_deadline_tick(21_007);
        Scheduler::on_deadline_tick(41_000);
        assert_eq!(Scheduler::max_jitter_us(), 7);
    }

    #[test]
    fn tick_cadence_is_configurable_and_rejects_zero() {
        Scheduler::reset_stats();
        assert!(!Scheduler::set_tick_period_us(0));
        assert_eq!(Scheduler::tick_period_us(), DEADLINE_PERIOD_US as u32);
        assert!(Scheduler::set_tick_period_us(1_000)); // 1 kHz control loop
        Scheduler::on_deadline_tick(10_000);
        Scheduler::on_deadline_tick(11_004);
        assert_eq!(Scheduler::max_jitter_us(), 4);
        Scheduler::reset_stats(); // restores the default cadence
        assert_eq!(Scheduler::tick_period_us(), DEADLINE_PERIOD_US as u32);
    }

    #[test]
    fn millisecond_timer_saturates_before_addition() {
        let timer = Timer::after_ms(u64::MAX, 10);
        assert!(timer.is_ready(u64::MAX));
        assert!(!timer.is_ready(u64::MAX - 1));
    }
}
