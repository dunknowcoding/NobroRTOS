//! Deadline slot scheduler (Phase 1): TIMER1 drives 50 Hz hard-real-time ticks.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::KernelError;

/// Expected interval between deadline ticks (50 Hz servo loop).
pub const DEADLINE_PERIOD_US: u64 = 20_000;

static EXPECTED_NEXT_US: AtomicU32 = AtomicU32::new(0);
static MAX_JITTER_US: AtomicU32 = AtomicU32::new(0);
static TICK_COUNT: AtomicU32 = AtomicU32::new(0);
static DEADLINE_MISSES: AtomicU32 = AtomicU32::new(0);

pub type DeadlineHandler = fn();

static mut DEADLINE_HANDLER: Option<DeadlineHandler> = None;

pub struct Scheduler;

impl Scheduler {
    pub unsafe fn set_deadline_handler(handler: DeadlineHandler) {
        DEADLINE_HANDLER = Some(handler);
    }

    pub fn reset_stats() {
        MAX_JITTER_US.store(0, Ordering::Release);
        TICK_COUNT.store(0, Ordering::Release);
        DEADLINE_MISSES.store(0, Ordering::Release);
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

    /// Called from TIMER1 ISR or polled compare handler.
    pub fn on_deadline_tick(now_us: u64) {
        let now_lo = now_us as u32;
        let expected = EXPECTED_NEXT_US.load(Ordering::Acquire);
        if expected != 0 {
            let jitter = if now_lo >= expected {
                now_lo - expected
            } else {
                expected - now_lo
            };
            if jitter > MAX_JITTER_US.load(Ordering::Relaxed) {
                MAX_JITTER_US.store(jitter, Ordering::Release);
            }
            if jitter > 10 {
                DEADLINE_MISSES.fetch_add(1, Ordering::AcqRel);
            }
        }
        EXPECTED_NEXT_US.store(
            now_lo.wrapping_add(DEADLINE_PERIOD_US as u32),
            Ordering::Release,
        );
        TICK_COUNT.fetch_add(1, Ordering::AcqRel);

        unsafe {
            if let Some(handler) = DEADLINE_HANDLER {
                handler();
            }
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
        Self::after_us(ms * 1000, now_us)
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
    }
}
