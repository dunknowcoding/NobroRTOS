//! Deadline slot scheduler (Phase 1): TIMER1 drives 50 Hz hard-real-time ticks.

use portable_atomic::{AtomicBool, AtomicU32, Ordering};

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
/// Thread-mode configuration writer sequence. It is separate from the ISR
/// sequence so the deadline source never waits for a configuration lock.
static CONFIG_SEQUENCE: AtomicU32 = AtomicU32::new(0);
static STATS_SEQUENCE: AtomicU32 = AtomicU32::new(0);
static PENDING_DEADLINE_TICKS: AtomicU32 = AtomicU32::new(0);
static DEFERRED_TICK: AtomicBool = AtomicBool::new(false);
static DEFERRED_NOW_US: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickConfigError<E> {
    ZeroPeriod,
    Provider(E),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SchedulerStats {
    pub tick_count: u32,
    pub max_jitter_us: u32,
    pub deadline_misses: u32,
    pub jitter_tolerance_us: u32,
}

pub struct Scheduler;

struct StatsWriter(u32);

impl Drop for StatsWriter {
    fn drop(&mut self) {
        STATS_SEQUENCE.store(self.0.wrapping_add(2), Ordering::Release);
    }
}

fn try_stats_writer() -> Option<StatsWriter> {
    let observed = STATS_SEQUENCE.load(Ordering::Acquire);
    if observed & 1 != 0 {
        return None;
    }
    STATS_SEQUENCE
        .compare_exchange(
            observed,
            observed.wrapping_add(1),
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .ok()
        .map(StatsWriter)
}

fn defer_tick(now_us: u64) {
    // TIMER1 cannot re-enter itself on the single-core nRF target. Publishing
    // the timestamp before the flag lets the interrupted configuration drain
    // this exact boundary after it commits, without ever waiting in the ISR.
    DEFERRED_NOW_US.store(now_us as u32, Ordering::Relaxed);
    DEFERRED_TICK.store(true, Ordering::Release);
}

fn drain_deferred_tick() {
    if DEFERRED_TICK.swap(false, Ordering::AcqRel) {
        Scheduler::on_deadline_tick(DEFERRED_NOW_US.load(Ordering::Acquire) as u64);
    }
}

fn configure<R>(operation: impl FnOnce() -> R) -> R {
    let generation = loop {
        let observed = CONFIG_SEQUENCE.load(Ordering::Acquire);
        if observed & 1 == 0
            && CONFIG_SEQUENCE
                .compare_exchange(
                    observed,
                    observed.wrapping_add(1),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
        {
            break observed;
        }
        core::hint::spin_loop();
    };
    // A host model may execute the writer and ISR on different threads. On
    // target, an in-flight ISR has already returned before thread mode resumes.
    while STATS_SEQUENCE.load(Ordering::Acquire) & 1 != 0 {
        core::hint::spin_loop();
    }
    struct Finish(u32);
    impl Drop for Finish {
        fn drop(&mut self) {
            CONFIG_SEQUENCE.store(self.0.wrapping_add(2), Ordering::Release);
        }
    }
    let finish = Finish(generation);
    let result = operation();
    drop(finish);
    drain_deferred_tick();
    result
}

impl Scheduler {
    pub fn reset_stats() {
        configure(|| {
            EXPECTED_NEXT_US.store(0, Ordering::Release);
            MAX_JITTER_US.store(0, Ordering::Release);
            TICK_COUNT.store(0, Ordering::Release);
            DEADLINE_MISSES.store(0, Ordering::Release);
            PENDING_DEADLINE_TICKS.store(0, Ordering::Release);
            JITTER_TOLERANCE_US.store(DEFAULT_JITTER_TOLERANCE_US, Ordering::Release);
        });
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
        configure(|| JITTER_TOLERANCE_US.store(tolerance_us, Ordering::Release));
    }

    pub fn tick_period_us() -> u32 {
        TICK_PERIOD_US.load(Ordering::Acquire)
    }

    /// Atomically reprogram the hardware tick source and publish its software cadence.
    /// The caller owns the provider session. The deadline ISR is never masked:
    /// a tick racing the bounded register update observes the old period, after
    /// which the successful publish re-anchors the next phase.
    pub fn reconfigure_tick_period<E>(
        period_us: u32,
        program_provider: impl FnOnce(u32) -> Result<(), E>,
    ) -> Result<(), TickConfigError<E>> {
        if period_us == 0 {
            return Err(TickConfigError::ZeroPeriod);
        }
        program_provider(period_us).map_err(TickConfigError::Provider)?;
        configure(|| {
            TICK_PERIOD_US.store(period_us, Ordering::Release);
            EXPECTED_NEXT_US.store(0, Ordering::Release);
        });
        Ok(())
    }

    pub fn stats() -> SchedulerStats {
        loop {
            let config_before = CONFIG_SEQUENCE.load(Ordering::Acquire);
            if config_before & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }
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
            let config_after = CONFIG_SEQUENCE.load(Ordering::Acquire);
            if before == after && config_before == config_after {
                return stats;
            }
        }
    }

    /// Called from TIMER1 ISR or polled compare handler.
    pub fn on_deadline_tick(now_us: u64) {
        let Some(writer) = try_stats_writer() else {
            defer_tick(now_us);
            return;
        };
        if CONFIG_SEQUENCE.load(Ordering::Acquire) & 1 != 0 {
            drop(writer);
            defer_tick(now_us);
            return;
        }
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
        let _ = PENDING_DEADLINE_TICKS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
            Some(value.saturating_add(1))
        });
        drop(writer);
    }

    /// Claim at most `max_ticks` ISR releases for execution as ordinary admitted work.
    /// The ISR never calls user code; consumers drain this counter under their executor
    /// budget/priority domain. Excess releases remain pending (saturating at `u32::MAX`).
    pub fn take_pending_deadline_ticks(max_ticks: u32) -> u32 {
        if max_ticks == 0 {
            return 0;
        }
        let mut claimed = 0;
        let _ =
            PENDING_DEADLINE_TICKS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                claimed = pending.min(max_ticks);
                Some(pending - claimed)
            });
        claimed
    }

    pub fn note_error(err: KernelError) -> crate::Action {
        default_action(&err)
    }
}

/// Cooperative async sleep helper.
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
        KernelError::WatchdogExpired => NotifyUserTask,
        KernelError::ModuleCrash
        | KernelError::PoolCorruption
        | KernelError::PowerTransitionFail => RebootModule,
        KernelError::ProtocolAuthFail | KernelError::QuotaBreach | KernelError::StorageFail => {
            NotifyUserTask
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::sync::{Mutex, MutexGuard};

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    fn lock() -> MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
    }

    #[test]
    fn reset_clears_expected_deadline() {
        let _lock = lock();
        Scheduler::reconfigure_tick_period(DEADLINE_PERIOD_US as u32, |_| Ok::<_, ()>(())).unwrap();
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
        let _lock = lock();
        Scheduler::reconfigure_tick_period(DEADLINE_PERIOD_US as u32, |_| Ok::<_, ()>(())).unwrap();
        Scheduler::reset_stats();
        let first = u32::MAX as u64 - 5;
        Scheduler::on_deadline_tick(first);
        Scheduler::on_deadline_tick(first + DEADLINE_PERIOD_US + 3);
        assert_eq!(Scheduler::max_jitter_us(), 3);
    }

    #[test]
    fn jitter_tolerance_is_configurable() {
        let _lock = lock();
        Scheduler::reconfigure_tick_period(DEADLINE_PERIOD_US as u32, |_| Ok::<_, ()>(())).unwrap();
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
        let _lock = lock();
        Scheduler::reconfigure_tick_period(DEADLINE_PERIOD_US as u32, |_| Ok::<_, ()>(())).unwrap();
        Scheduler::reset_stats();
        Scheduler::on_deadline_tick(1_000);
        Scheduler::on_deadline_tick(21_007);
        Scheduler::on_deadline_tick(41_000);
        assert_eq!(Scheduler::max_jitter_us(), 7);
    }

    #[test]
    fn tick_cadence_is_configurable_and_rejects_zero() {
        let _lock = lock();
        Scheduler::reconfigure_tick_period(DEADLINE_PERIOD_US as u32, |_| Ok::<_, ()>(())).unwrap();
        Scheduler::reset_stats();
        assert_eq!(
            Scheduler::reconfigure_tick_period(0, |_| Ok::<_, ()>(())),
            Err(TickConfigError::ZeroPeriod)
        );
        assert_eq!(Scheduler::tick_period_us(), DEADLINE_PERIOD_US as u32);
        assert_eq!(
            Scheduler::reconfigure_tick_period(1_000, |_| Ok::<_, ()>(())),
            Ok(())
        );
        Scheduler::on_deadline_tick(10_000);
        Scheduler::on_deadline_tick(11_004);
        assert_eq!(Scheduler::max_jitter_us(), 4);
        Scheduler::reset_stats(); // stats reset preserves the real provider cadence
        assert_eq!(Scheduler::tick_period_us(), 1_000);
        Scheduler::reconfigure_tick_period(DEADLINE_PERIOD_US as u32, |_| Ok::<_, ()>(())).unwrap();
        assert_eq!(Scheduler::tick_period_us(), DEADLINE_PERIOD_US as u32);
    }

    #[test]
    fn provider_failure_keeps_old_cadence_and_isr_never_calls_user_code() {
        let _lock = lock();
        Scheduler::reconfigure_tick_period(20_000, |_| Ok::<_, ()>(())).unwrap();
        let error = Scheduler::reconfigure_tick_period(500, |_| Err("provider rejected"));
        assert_eq!(error, Err(TickConfigError::Provider("provider rejected")));
        assert_eq!(Scheduler::tick_period_us(), 20_000);
        Scheduler::reset_stats();
        Scheduler::on_deadline_tick(10);
        Scheduler::on_deadline_tick(20_010);
        assert_eq!(Scheduler::take_pending_deadline_ticks(1), 1);
        assert_eq!(Scheduler::take_pending_deadline_ticks(8), 1);
        assert_eq!(Scheduler::take_pending_deadline_ticks(8), 0);
    }

    #[test]
    fn tick_inside_configuration_is_deferred_once_and_uses_committed_period() {
        let _lock = lock();
        Scheduler::reconfigure_tick_period(DEADLINE_PERIOD_US as u32, |_| Ok::<_, ()>(())).unwrap();
        Scheduler::reset_stats();

        configure(|| {
            TICK_PERIOD_US.store(1_000, Ordering::Release);
            EXPECTED_NEXT_US.store(0, Ordering::Release);
            // Model TIMER1 preempting thread mode after the new cadence has
            // been written but before the configuration generation commits.
            Scheduler::on_deadline_tick(50_000);
            assert_eq!(Scheduler::tick_count(), 0);
            assert_eq!(Scheduler::take_pending_deadline_ticks(1), 0);
        });

        assert_eq!(Scheduler::tick_count(), 1);
        assert_eq!(Scheduler::take_pending_deadline_ticks(1), 1);
        Scheduler::on_deadline_tick(51_004);
        assert_eq!(Scheduler::max_jitter_us(), 4);
        assert_eq!(Scheduler::tick_count(), 2);
        Scheduler::reconfigure_tick_period(DEADLINE_PERIOD_US as u32, |_| Ok::<_, ()>(())).unwrap();
    }

    #[test]
    fn millisecond_timer_saturates_before_addition() {
        let _lock = lock();
        let timer = Timer::after_ms(u64::MAX, 10);
        assert!(timer.is_ready(u64::MAX));
        assert!(!timer.is_ready(u64::MAX - 1));
    }
}
