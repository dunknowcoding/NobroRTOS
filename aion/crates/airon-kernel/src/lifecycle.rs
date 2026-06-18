//! System lifecycle state machine for boot, degradation, and recovery.

use crate::{Action, EventKind, EventPayload, EventRecord, EventSeverity, KernelError, ModuleId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemState {
    ColdBoot,
    ValidateManifest,
    InitDrivers,
    Running,
    Degraded,
    Recovering,
    Halted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleError {
    InvalidTransition { from: SystemState, to: SystemState },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lifecycle {
    state: SystemState,
    previous: SystemState,
    transitions: u32,
    last_change_us: u64,
}

impl Lifecycle {
    pub const fn new() -> Self {
        Self {
            state: SystemState::ColdBoot,
            previous: SystemState::ColdBoot,
            transitions: 0,
            last_change_us: 0,
        }
    }

    pub const fn state(&self) -> SystemState {
        self.state
    }

    pub const fn previous(&self) -> SystemState {
        self.previous
    }

    pub const fn transitions(&self) -> u32 {
        self.transitions
    }

    pub const fn last_change_us(&self) -> u64 {
        self.last_change_us
    }

    pub fn transition(
        &mut self,
        to: SystemState,
        now_us: u64,
    ) -> Result<EventRecord, LifecycleError> {
        if !Self::is_valid_transition(self.state, to) {
            return Err(LifecycleError::InvalidTransition {
                from: self.state,
                to,
            });
        }

        let from = self.state;
        self.previous = from;
        self.state = to;
        self.transitions = self.transitions.saturating_add(1);
        self.last_change_us = now_us;

        Ok(EventRecord::new(
            now_us,
            ModuleId::Kernel,
            Self::severity_for(to),
            EventKind::Host,
            EventPayload::Pair(from as u32, to as u32),
        ))
    }

    pub fn apply_action(
        &mut self,
        module: ModuleId,
        error: KernelError,
        action: Action,
        now_us: u64,
    ) -> Result<EventRecord, LifecycleError> {
        let target = match action {
            Action::RetryNow | Action::RetryDelay(_) | Action::Ignore => self.state,
            Action::NotifyUserTask => SystemState::Degraded,
            Action::RebootModule => SystemState::Recovering,
        };

        if target == self.state {
            return Ok(EventRecord::new(
                now_us,
                module,
                EventSeverity::Info,
                EventKind::Recovery,
                EventPayload::Error(error),
            ));
        }

        self.transition(target, now_us)
    }

    pub const fn is_valid_transition(from: SystemState, to: SystemState) -> bool {
        use SystemState::*;
        matches!(
            (from, to),
            (ColdBoot, ValidateManifest)
                | (ValidateManifest, InitDrivers)
                | (ValidateManifest, Halted)
                | (InitDrivers, Running)
                | (InitDrivers, Halted)
                | (Running, Degraded)
                | (Running, Recovering)
                | (Running, Halted)
                | (Degraded, Running)
                | (Degraded, Recovering)
                | (Degraded, Halted)
                | (Recovering, InitDrivers)
                | (Recovering, Degraded)
                | (Recovering, Halted)
        )
    }

    const fn severity_for(state: SystemState) -> EventSeverity {
        match state {
            SystemState::ColdBoot
            | SystemState::ValidateManifest
            | SystemState::InitDrivers
            | SystemState::Running => EventSeverity::Info,
            SystemState::Degraded | SystemState::Recovering => EventSeverity::Warn,
            SystemState::Halted => EventSeverity::Fatal,
        }
    }
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_path_reaches_running() {
        let mut lifecycle = Lifecycle::new();

        lifecycle
            .transition(SystemState::ValidateManifest, 10)
            .unwrap();
        lifecycle.transition(SystemState::InitDrivers, 20).unwrap();
        let event = lifecycle.transition(SystemState::Running, 30).unwrap();

        assert_eq!(lifecycle.state(), SystemState::Running);
        assert_eq!(lifecycle.previous(), SystemState::InitDrivers);
        assert_eq!(lifecycle.transitions(), 3);
        assert_eq!(event.payload, EventPayload::Pair(2, 3));
    }

    #[test]
    fn invalid_transition_is_rejected() {
        let mut lifecycle = Lifecycle::new();
        assert_eq!(
            lifecycle.transition(SystemState::Running, 10),
            Err(LifecycleError::InvalidTransition {
                from: SystemState::ColdBoot,
                to: SystemState::Running
            })
        );
    }

    #[test]
    fn recovery_action_moves_running_to_recovering() {
        let mut lifecycle = Lifecycle::new();
        lifecycle
            .transition(SystemState::ValidateManifest, 10)
            .unwrap();
        lifecycle.transition(SystemState::InitDrivers, 20).unwrap();
        lifecycle.transition(SystemState::Running, 30).unwrap();

        let event = lifecycle
            .apply_action(
                ModuleId::Sensor,
                KernelError::SensorReadFail,
                Action::RebootModule,
                40,
            )
            .unwrap();

        assert_eq!(lifecycle.state(), SystemState::Recovering);
        assert_eq!(event.severity, EventSeverity::Warn);
    }

    #[test]
    fn ignored_action_does_not_move_state() {
        let mut lifecycle = Lifecycle::new();
        lifecycle
            .transition(SystemState::ValidateManifest, 10)
            .unwrap();

        let event = lifecycle
            .apply_action(ModuleId::Bus, KernelError::BusTimeout, Action::Ignore, 20)
            .unwrap();

        assert_eq!(lifecycle.state(), SystemState::ValidateManifest);
        assert_eq!(event.module, ModuleId::Bus);
        assert_eq!(event.kind, EventKind::Recovery);
    }
}
