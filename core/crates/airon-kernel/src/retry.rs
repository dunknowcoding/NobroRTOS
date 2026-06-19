//! Retry and backoff policy for bounded recovery loops.

use crate::Action;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackoffKind {
    Constant,
    Linear,
    Exponential,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: u8,
    pub base_delay_us: u32,
    pub max_delay_us: u32,
    pub kind: BackoffKind,
}

impl RetryPolicy {
    pub const DEFAULT: Self = Self {
        max_attempts: 3,
        base_delay_us: 1_000,
        max_delay_us: 20_000,
        kind: BackoffKind::Exponential,
    };

    pub const fn new(
        max_attempts: u8,
        base_delay_us: u32,
        max_delay_us: u32,
        kind: BackoffKind,
    ) -> Self {
        Self {
            max_attempts,
            base_delay_us,
            max_delay_us,
            kind,
        }
    }

    pub fn delay_for_attempt(self, attempt: u8) -> u32 {
        if attempt == 0 || self.base_delay_us == 0 {
            return 0;
        }

        let raw = match self.kind {
            BackoffKind::Constant => self.base_delay_us,
            BackoffKind::Linear => self.base_delay_us.saturating_mul(u32::from(attempt)),
            BackoffKind::Exponential => {
                let shift = u32::from(attempt.saturating_sub(1)).min(31);
                self.base_delay_us.saturating_mul(1u32 << shift)
            }
        };
        raw.min(self.max_delay_us)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryState {
    pub attempts: u8,
    pub next_due_us: u64,
    pub exhausted: bool,
}

impl RetryState {
    pub const fn new() -> Self {
        Self {
            attempts: 0,
            next_due_us: 0,
            exhausted: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn fail(&mut self, now_us: u64, policy: RetryPolicy) -> Action {
        if self.exhausted || self.attempts >= policy.max_attempts {
            self.exhausted = true;
            return Action::NotifyUserTask;
        }

        self.attempts = self.attempts.saturating_add(1);
        let delay = policy.delay_for_attempt(self.attempts);
        self.next_due_us = now_us.saturating_add(u64::from(delay));
        if delay == 0 {
            Action::RetryNow
        } else {
            Action::RetryDelay(delay)
        }
    }

    pub fn ready(self, now_us: u64) -> bool {
        !self.exhausted && now_us >= self.next_due_us
    }
}

impl Default for RetryState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponential_backoff_is_capped() {
        let policy = RetryPolicy::new(5, 1_000, 3_000, BackoffKind::Exponential);
        assert_eq!(policy.delay_for_attempt(1), 1_000);
        assert_eq!(policy.delay_for_attempt(2), 2_000);
        assert_eq!(policy.delay_for_attempt(3), 3_000);
        assert_eq!(policy.delay_for_attempt(4), 3_000);
    }

    #[test]
    fn retry_state_exhausts_after_max_attempts() {
        let policy = RetryPolicy::new(2, 10, 100, BackoffKind::Constant);
        let mut state = RetryState::new();

        assert_eq!(state.fail(0, policy), Action::RetryDelay(10));
        assert_eq!(state.fail(10, policy), Action::RetryDelay(10));
        assert_eq!(state.fail(20, policy), Action::NotifyUserTask);
        assert!(state.exhausted);
    }

    #[test]
    fn retry_ready_respects_next_due_time() {
        let policy = RetryPolicy::new(1, 50, 100, BackoffKind::Constant);
        let mut state = RetryState::new();
        state.fail(100, policy);
        assert!(!state.ready(149));
        assert!(state.ready(150));
    }
}
