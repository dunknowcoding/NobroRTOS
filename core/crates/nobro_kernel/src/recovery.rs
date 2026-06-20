//! Recovery coordinator for health, lifecycle, and watchdog-triggered faults.

use crate::{
    Action, EventLog, FaultThresholds, KernelError, Lifecycle, LifecycleError, ModuleId,
    Supervisor, SupervisorSnapshot, SystemState,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryOutcome {
    pub module: ModuleId,
    pub error: KernelError,
    pub action: Action,
    pub state: SystemState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryError {
    Lifecycle(LifecycleError),
}

pub struct RecoveryCoordinator<const HEALTH_SLOTS: usize, const LOG_SLOTS: usize> {
    supervisor: Supervisor<HEALTH_SLOTS, LOG_SLOTS>,
    lifecycle: Lifecycle,
}

impl<const HEALTH_SLOTS: usize, const LOG_SLOTS: usize>
    RecoveryCoordinator<HEALTH_SLOTS, LOG_SLOTS>
{
    pub const fn new(thresholds: FaultThresholds) -> Self {
        Self {
            supervisor: Supervisor::with_default_policy(thresholds),
            lifecycle: Lifecycle::new(),
        }
    }

    pub fn transition(&mut self, to: SystemState, now_us: u64) -> Result<(), RecoveryError> {
        let event = self
            .lifecycle
            .transition(to, now_us)
            .map_err(RecoveryError::Lifecycle)?;
        self.supervisor.events_mut().push(event);
        Ok(())
    }

    pub fn record_ok(&mut self, module: ModuleId, now_us: u64) {
        self.supervisor.record_ok(module, now_us);
    }

    pub fn record_error(
        &mut self,
        module: ModuleId,
        error: KernelError,
        now_us: u64,
    ) -> Result<RecoveryOutcome, RecoveryError> {
        let action = self.supervisor.record_error(module, error, now_us);
        let event = self
            .lifecycle
            .apply_action(module, error, action, now_us)
            .map_err(RecoveryError::Lifecycle)?;
        self.supervisor.events_mut().push(event);

        Ok(RecoveryOutcome {
            module,
            error,
            action,
            state: self.lifecycle.state(),
        })
    }

    pub fn record_watchdog_expired(
        &mut self,
        module: ModuleId,
        now_us: u64,
    ) -> Result<RecoveryOutcome, RecoveryError> {
        self.record_error(module, KernelError::DeadlineMissed, now_us)
    }

    pub const fn state(&self) -> SystemState {
        self.lifecycle.state()
    }

    pub const fn lifecycle(&self) -> &Lifecycle {
        &self.lifecycle
    }

    pub fn snapshot(&self, module: ModuleId) -> Option<SupervisorSnapshot> {
        self.supervisor.snapshot(module)
    }

    pub fn events(&self) -> &EventLog<LOG_SLOTS> {
        self.supervisor.events()
    }
}

impl<const HEALTH_SLOTS: usize, const LOG_SLOTS: usize> Default
    for RecoveryCoordinator<HEALTH_SLOTS, LOG_SLOTS>
{
    fn default() -> Self {
        Self::new(FaultThresholds::DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventKind, EventPayload, EventRecord, EventSeverity};

    fn running_coordinator() -> RecoveryCoordinator<2, 12> {
        let mut recovery = RecoveryCoordinator::<2, 12>::new(FaultThresholds {
            notify_after: 1,
            reboot_after: 3,
        });
        recovery
            .transition(SystemState::ValidateManifest, 10)
            .unwrap();
        recovery.transition(SystemState::InitDrivers, 20).unwrap();
        recovery.transition(SystemState::Running, 30).unwrap();
        recovery
    }

    #[test]
    fn notify_action_moves_running_to_degraded() {
        let mut recovery = running_coordinator();

        let outcome = recovery
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 40)
            .unwrap();

        assert_eq!(outcome.action, Action::NotifyUserTask);
        assert_eq!(outcome.state, SystemState::Degraded);
        assert_eq!(recovery.state(), SystemState::Degraded);
        assert!(
            recovery
                .snapshot(ModuleId::Sensor)
                .expect("sensor snapshot")
                .log_len
                >= 6
        );
    }

    #[test]
    fn repeated_errors_move_to_recovering() {
        let mut recovery = running_coordinator();

        recovery
            .record_error(ModuleId::Radio, KernelError::RadioTxFail, 40)
            .unwrap();
        recovery
            .record_error(ModuleId::Radio, KernelError::RadioTxFail, 50)
            .unwrap();
        let outcome = recovery
            .record_error(ModuleId::Radio, KernelError::RadioTxFail, 60)
            .unwrap();

        assert_eq!(outcome.action, Action::RebootModule);
        assert_eq!(outcome.state, SystemState::Recovering);
    }

    #[test]
    fn watchdog_expiry_is_routed_as_deadline_fault() {
        let mut recovery = running_coordinator();

        let outcome = recovery
            .record_watchdog_expired(ModuleId::Actuator, 40)
            .unwrap();

        assert_eq!(outcome.error, KernelError::DeadlineMissed);
        assert_eq!(outcome.action, Action::NotifyUserTask);
        assert_eq!(outcome.state, SystemState::Degraded);
    }

    #[test]
    fn transition_events_are_written_to_log() {
        let recovery = running_coordinator();
        let empty = EventRecord::new(
            0,
            ModuleId::Kernel,
            EventSeverity::Trace,
            EventKind::Boot,
            EventPayload::None,
        );
        let mut recent = [empty; 3];

        assert_eq!(recovery.events().copy_recent(&mut recent), 3);
        assert_eq!(recent[0].payload, EventPayload::Pair(0, 1));
        assert_eq!(recent[1].payload, EventPayload::Pair(1, 2));
        assert_eq!(recent[2].payload, EventPayload::Pair(2, 3));
    }
}
