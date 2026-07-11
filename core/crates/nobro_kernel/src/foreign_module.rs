//! Fail-closed execution state for callbacks behind a foreign ABI.

use crate::{CapabilitySet, ModuleLaunchGate};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForeignModuleState {
    Closed,
    Admitted,
    Running,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForeignModuleError {
    AdmissionDenied,
    InvalidState(ForeignModuleState),
    InitFailed(i32),
    PollFailed(i32),
}

/// Owns the only legal path from admission to foreign init/poll callbacks.
pub struct ForeignModuleRunner<'a> {
    gate: &'a ModuleLaunchGate,
    state: ForeignModuleState,
}

impl<'a> ForeignModuleRunner<'a> {
    pub const fn new(gate: &'a ModuleLaunchGate) -> Self {
        Self {
            gate,
            state: ForeignModuleState::Closed,
        }
    }

    pub fn admit(&mut self, granted: Option<CapabilitySet>) -> Result<(), ForeignModuleError> {
        let Some(granted) = granted.filter(|set| set.bits() != 0) else {
            self.fail();
            return Err(ForeignModuleError::AdmissionDenied);
        };
        self.gate.install(granted);
        self.state = ForeignModuleState::Admitted;
        Ok(())
    }

    pub fn initialize<F>(&mut self, callback: F) -> Result<(), ForeignModuleError>
    where
        F: FnOnce() -> i32,
    {
        if self.state != ForeignModuleState::Admitted {
            return Err(ForeignModuleError::InvalidState(self.state));
        }
        let result = callback();
        if result < 0 {
            self.fail();
            return Err(ForeignModuleError::InitFailed(result));
        }
        self.state = ForeignModuleState::Running;
        Ok(())
    }

    pub fn poll<F>(&mut self, callback: F) -> Result<(), ForeignModuleError>
    where
        F: FnOnce() -> i32,
    {
        if self.state != ForeignModuleState::Running {
            return Err(ForeignModuleError::InvalidState(self.state));
        }
        let result = callback();
        if result < 0 {
            self.fail();
            return Err(ForeignModuleError::PollFailed(result));
        }
        Ok(())
    }

    pub fn close(&mut self) {
        self.gate.revoke();
        self.state = ForeignModuleState::Closed;
    }

    pub const fn state(&self) -> ForeignModuleState {
        self.state
    }

    fn fail(&mut self) {
        self.gate.revoke();
        self.state = ForeignModuleState::Failed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Capability;
    use core::cell::Cell;

    #[test]
    fn denied_admission_never_calls_init_or_poll() {
        let gate = ModuleLaunchGate::new();
        let mut runner = ForeignModuleRunner::new(&gate);
        let calls = Cell::new(0);

        assert_eq!(runner.admit(None), Err(ForeignModuleError::AdmissionDenied));
        assert_eq!(
            runner.initialize(|| {
                calls.set(calls.get() + 1);
                0
            }),
            Err(ForeignModuleError::InvalidState(ForeignModuleState::Failed))
        );
        assert_eq!(
            runner.poll(|| {
                calls.set(calls.get() + 1);
                0
            }),
            Err(ForeignModuleError::InvalidState(ForeignModuleState::Failed))
        );
        assert_eq!(calls.get(), 0);
        assert!(!gate.is_admitted());
    }

    #[test]
    fn failed_poll_revokes_authority_and_cannot_continue() {
        let gate = ModuleLaunchGate::new();
        let mut runner = ForeignModuleRunner::new(&gate);
        let polls = Cell::new(0);
        runner
            .admit(Some(CapabilitySet::empty().with(Capability::Bus0)))
            .unwrap();
        runner.initialize(|| 0).unwrap();

        assert_eq!(
            runner.poll(|| {
                polls.set(polls.get() + 1);
                -7
            }),
            Err(ForeignModuleError::PollFailed(-7))
        );
        assert_eq!(
            runner.poll(|| {
                polls.set(polls.get() + 1);
                0
            }),
            Err(ForeignModuleError::InvalidState(ForeignModuleState::Failed))
        );
        assert_eq!(polls.get(), 1);
        assert!(!gate.allows(Capability::Bus0));
    }
}
