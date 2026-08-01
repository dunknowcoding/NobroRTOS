//! Pollable lifecycle model for the Arduino-ESP8266 station adapter.
//!
//! The C++ facade owns no credentials or heap. The vendor radio/lwIP stack is
//! process-wide and heap-managed, so this crate models only Nobro's bounded
//! state, deadline, recovery, and diagnostic contract.
#![no_std]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleState {
    Down,
    Ready,
    Joining,
    Up,
    Quiesced,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollObservation {
    Pending,
    Connected,
    Rejected,
    TransportFault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleError {
    Busy,
    NotReady,
    InvalidDeadline,
    DeadlineElapsed,
    AssociationRejected,
    BackendFault,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Diagnostics {
    pub join_attempts: u32,
    pub join_failures: u32,
    pub deadline_misses: u32,
    pub leaves: u32,
    pub recoveries: u32,
    pub transport_faults: u32,
}

pub struct AsyncWifiLifecycle {
    state: LifecycleState,
    deadline_us: u64,
    diagnostics: Diagnostics,
}

impl Default for AsyncWifiLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncWifiLifecycle {
    pub const fn new() -> Self {
        Self {
            state: LifecycleState::Down,
            deadline_us: 0,
            diagnostics: Diagnostics {
                join_attempts: 0,
                join_failures: 0,
                deadline_misses: 0,
                leaves: 0,
                recoveries: 0,
                transport_faults: 0,
            },
        }
    }

    pub const fn state(&self) -> LifecycleState {
        self.state
    }

    pub const fn diagnostics(&self) -> Diagnostics {
        self.diagnostics
    }

    pub fn mount(&mut self) -> Result<(), LifecycleError> {
        if !matches!(self.state, LifecycleState::Down | LifecycleState::Quiesced) {
            return Err(LifecycleError::Busy);
        }
        self.deadline_us = 0;
        self.state = LifecycleState::Ready;
        Ok(())
    }

    pub fn begin_join(&mut self, now_us: u64, deadline_us: u64) -> Result<(), LifecycleError> {
        if self.state != LifecycleState::Ready {
            return Err(LifecycleError::NotReady);
        }
        if deadline_us <= now_us {
            return Err(LifecycleError::InvalidDeadline);
        }
        self.deadline_us = deadline_us;
        self.state = LifecycleState::Joining;
        self.diagnostics.join_attempts = self.diagnostics.join_attempts.saturating_add(1);
        Ok(())
    }

    pub fn poll(
        &mut self,
        now_us: u64,
        observation: PollObservation,
    ) -> Result<bool, LifecycleError> {
        if self.state != LifecycleState::Joining {
            return Err(LifecycleError::NotReady);
        }
        match observation {
            PollObservation::Connected => {
                self.state = LifecycleState::Up;
                Ok(true)
            }
            PollObservation::Rejected => {
                self.state = LifecycleState::Ready;
                self.diagnostics.join_failures = self.diagnostics.join_failures.saturating_add(1);
                Err(LifecycleError::AssociationRejected)
            }
            PollObservation::TransportFault => {
                self.state = LifecycleState::Faulted;
                self.diagnostics.join_failures = self.diagnostics.join_failures.saturating_add(1);
                self.diagnostics.transport_faults =
                    self.diagnostics.transport_faults.saturating_add(1);
                Err(LifecycleError::BackendFault)
            }
            PollObservation::Pending if now_us >= self.deadline_us => {
                self.state = LifecycleState::Ready;
                self.diagnostics.join_failures = self.diagnostics.join_failures.saturating_add(1);
                self.diagnostics.deadline_misses =
                    self.diagnostics.deadline_misses.saturating_add(1);
                Err(LifecycleError::DeadlineElapsed)
            }
            PollObservation::Pending => Ok(false),
        }
    }

    pub fn leave(&mut self) -> Result<(), LifecycleError> {
        if !matches!(
            self.state,
            LifecycleState::Ready | LifecycleState::Joining | LifecycleState::Up
        ) {
            return Err(LifecycleError::NotReady);
        }
        self.deadline_us = 0;
        self.state = LifecycleState::Ready;
        self.diagnostics.leaves = self.diagnostics.leaves.saturating_add(1);
        Ok(())
    }

    pub fn quiesce(&mut self) {
        self.deadline_us = 0;
        self.state = LifecycleState::Quiesced;
    }

    pub fn recover(&mut self) {
        self.deadline_us = 0;
        self.state = LifecycleState::Ready;
        self.diagnostics.recoveries = self.diagnostics.recoveries.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_join_is_pollable_and_recoverable() {
        let mut lifecycle = AsyncWifiLifecycle::new();
        assert_eq!(lifecycle.mount(), Ok(()));
        assert_eq!(lifecycle.begin_join(100, 1_000), Ok(()));
        assert_eq!(lifecycle.poll(200, PollObservation::Pending), Ok(false));
        assert_eq!(lifecycle.poll(300, PollObservation::Connected), Ok(true));
        assert_eq!(lifecycle.state(), LifecycleState::Up);
        assert_eq!(lifecycle.leave(), Ok(()));
        lifecycle.quiesce();
        lifecycle.recover();
        assert_eq!(lifecycle.state(), LifecycleState::Ready);
        assert_eq!(lifecycle.diagnostics().recoveries, 1);
    }

    #[test]
    fn deadline_and_rejection_return_to_ready_without_faulting() {
        let mut lifecycle = AsyncWifiLifecycle::new();
        lifecycle.mount().unwrap();
        lifecycle.begin_join(10, 20).unwrap();
        assert_eq!(
            lifecycle.poll(20, PollObservation::Pending),
            Err(LifecycleError::DeadlineElapsed)
        );
        assert_eq!(lifecycle.state(), LifecycleState::Ready);
        lifecycle.begin_join(30, 40).unwrap();
        assert_eq!(
            lifecycle.poll(31, PollObservation::Rejected),
            Err(LifecycleError::AssociationRejected)
        );
        assert_eq!(lifecycle.diagnostics().join_failures, 2);
    }

    #[test]
    fn faults_are_structured_and_owned_recovery_is_deterministic() {
        let mut lifecycle = AsyncWifiLifecycle::new();
        lifecycle.mount().unwrap();
        lifecycle.begin_join(1, 2).unwrap();
        assert_eq!(
            lifecycle.poll(1, PollObservation::TransportFault),
            Err(LifecycleError::BackendFault)
        );
        assert_eq!(lifecycle.state(), LifecycleState::Faulted);
        lifecycle.recover();
        assert_eq!(lifecycle.state(), LifecycleState::Ready);
        assert_eq!(lifecycle.diagnostics().transport_faults, 1);
    }

    #[test]
    fn invalid_transitions_fail_closed() {
        let mut lifecycle = AsyncWifiLifecycle::new();
        assert_eq!(lifecycle.begin_join(0, 1), Err(LifecycleError::NotReady));
        lifecycle.mount().unwrap();
        assert_eq!(
            lifecycle.begin_join(2, 2),
            Err(LifecycleError::InvalidDeadline)
        );
        lifecycle.begin_join(2, 3).unwrap();
        assert_eq!(lifecycle.begin_join(2, 4), Err(LifecycleError::NotReady));
        assert_eq!(lifecycle.mount(), Err(LifecycleError::Busy));
    }
}
