//! Kernel supervisor tying health decisions to the event log.

use crate::{
    scheduler::default_action, Action, EventLog, FaultPolicy, FaultThresholds, HealthCounters,
    HealthFault, HealthMonitor, KernelError, ModuleId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupervisorSnapshot {
    pub module: ModuleId,
    pub counters: HealthCounters,
    pub log_len: usize,
    pub dropped_events: u32,
}

pub struct Supervisor<const HEALTH_SLOTS: usize, const LOG_SLOTS: usize> {
    health: HealthMonitor<HEALTH_SLOTS>,
    log: EventLog<LOG_SLOTS>,
    thresholds: FaultThresholds,
    policy: fn(&KernelError) -> Action,
}

impl<const HEALTH_SLOTS: usize, const LOG_SLOTS: usize> Supervisor<HEALTH_SLOTS, LOG_SLOTS> {
    pub const fn new(thresholds: FaultThresholds, policy: fn(&KernelError) -> Action) -> Self {
        Self {
            health: HealthMonitor::new(),
            log: EventLog::new(),
            thresholds,
            policy,
        }
    }

    pub const fn with_default_policy(thresholds: FaultThresholds) -> Self {
        Self::new(thresholds, default_action)
    }

    pub fn record_ok(&mut self, module: ModuleId, now_us: u64) {
        self.health.record_ok(module, now_us);
    }

    pub fn record_error(&mut self, module: ModuleId, error: KernelError, now_us: u64) -> Action {
        let action = self.record_error_unlogged(module, error, now_us);
        self.log.push_health(now_us, module, error, action);
        action
    }

    pub fn record_fault(
        &mut self,
        module: ModuleId,
        fault: HealthFault,
        now_us: u64,
        policy: &mut impl FaultPolicy,
    ) -> Action {
        let action = self.record_fault_unlogged(module, fault, now_us, policy);
        self.log.push_health(now_us, module, fault.error, action);
        action
    }

    pub(crate) fn record_fault_unlogged(
        &mut self,
        module: ModuleId,
        fault: HealthFault,
        now_us: u64,
        policy: &mut impl FaultPolicy,
    ) -> Action {
        self.health
            .record_fault(module, fault, now_us, self.thresholds, policy)
    }

    pub(crate) fn record_error_unlogged(
        &mut self,
        module: ModuleId,
        error: KernelError,
        now_us: u64,
    ) -> Action {
        self.health
            .record_error(module, error, now_us, self.thresholds, self.policy)
    }

    pub fn snapshot(&self, module: ModuleId) -> Option<SupervisorSnapshot> {
        self.health.get(module).map(|counters| SupervisorSnapshot {
            module,
            counters,
            log_len: self.log.len(),
            dropped_events: self.log.dropped(),
        })
    }

    pub fn health(&self) -> &HealthMonitor<HEALTH_SLOTS> {
        &self.health
    }

    pub fn events(&self) -> &EventLog<LOG_SLOTS> {
        &self.log
    }

    pub fn events_mut(&mut self) -> &mut EventLog<LOG_SLOTS> {
        &mut self.log
    }
}

impl<const HEALTH_SLOTS: usize, const LOG_SLOTS: usize> Default
    for Supervisor<HEALTH_SLOTS, LOG_SLOTS>
{
    fn default() -> Self {
        Self::with_default_policy(FaultThresholds::DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventKind, EventPayload, EventRecord, EventSeverity};

    #[test]
    fn supervisor_records_error_and_recovery_events() {
        let mut supervisor = Supervisor::<2, 4>::with_default_policy(FaultThresholds {
            notify_after: 2,
            reboot_after: 3,
        });

        let first = supervisor.record_error(ModuleId::Sensor, KernelError::SensorReadFail, 10);
        let second = supervisor.record_error(ModuleId::Sensor, KernelError::SensorReadFail, 20);

        assert_eq!(first, Action::Ignore);
        assert_eq!(second, Action::NotifyUserTask);

        let snapshot = supervisor
            .snapshot(ModuleId::Sensor)
            .expect("sensor snapshot");
        assert_eq!(snapshot.counters.total_errors, 2);
        assert_eq!(snapshot.counters.consecutive_errors, 2);
        assert_eq!(snapshot.log_len, 4);

        let empty = EventRecord::new(
            0,
            ModuleId::Kernel,
            EventSeverity::Trace,
            EventKind::Boot,
            EventPayload::None,
        );
        let mut recent = [empty; 4];
        supervisor.events().copy_recent(&mut recent);
        assert_eq!(
            recent[0].payload,
            EventPayload::Error(KernelError::SensorReadFail)
        );
        assert_eq!(recent[1].payload, EventPayload::Action(Action::Ignore));
        assert_eq!(
            recent[3].payload,
            EventPayload::Action(Action::NotifyUserTask)
        );
    }

    #[test]
    fn supervisor_ok_resets_consecutive_faults() {
        let mut supervisor = Supervisor::<1, 4>::default();

        supervisor.record_error(ModuleId::Bus, KernelError::BusTimeout, 10);
        supervisor.record_ok(ModuleId::Bus, 20);

        let snapshot = supervisor.snapshot(ModuleId::Bus).expect("bus snapshot");
        assert_eq!(snapshot.counters.total_errors, 1);
        assert_eq!(snapshot.counters.consecutive_errors, 0);
        assert_eq!(snapshot.counters.last_seen_us, 20);
    }

    #[test]
    fn supervisor_reports_event_drops() {
        let mut supervisor = Supervisor::<1, 2>::default();

        supervisor.record_error(ModuleId::Radio, KernelError::RadioTxFail, 1);
        supervisor.record_error(ModuleId::Radio, KernelError::RadioTxFail, 2);

        let snapshot = supervisor
            .snapshot(ModuleId::Radio)
            .expect("radio snapshot");
        assert_eq!(snapshot.log_len, 2);
        assert_eq!(snapshot.dropped_events, 2);
    }
}
