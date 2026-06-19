//! Fixed-layout reports emitted by firmware and decoded by host tools.

use crate::{Action, EventSeverity, KernelError, ModuleId, Supervisor, SupervisorSnapshot};

pub const HEALTH_REPORT_MAGIC: u32 = 0x4152_484C; // "ARHL"
pub const HEALTH_REPORT_VERSION: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HealthReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub module_tag: u32,
    pub total_errors: u32,
    pub consecutive_errors: u32,
    pub last_error: u32,
    pub last_action: u32,
    pub event_count: u32,
    pub dropped_events: u32,
    pub error_events: u32,
    pub fatal_events: u32,
    pub last_seen_us_lo: u32,
    pub last_seen_us_hi: u32,
    pub checksum: u32,
}

impl HealthReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            module_tag: 0,
            total_errors: 0,
            consecutive_errors: 0,
            last_error: 0,
            last_action: 0,
            event_count: 0,
            dropped_events: 0,
            error_events: 0,
            fatal_events: 0,
            last_seen_us_lo: 0,
            last_seen_us_hi: 0,
            checksum: 0,
        }
    }

    pub fn from_snapshot(
        snapshot: SupervisorSnapshot,
        error_events: u32,
        fatal_events: u32,
    ) -> Self {
        let mut report = Self {
            module_tag: module_tag(snapshot.module),
            total_errors: snapshot.counters.total_errors,
            consecutive_errors: u32::from(snapshot.counters.consecutive_errors),
            last_error: snapshot.counters.last_error.map(error_code).unwrap_or(0),
            last_action: action_code(snapshot.counters.last_action),
            event_count: snapshot.log_len as u32,
            dropped_events: snapshot.dropped_events,
            error_events,
            fatal_events,
            last_seen_us_lo: snapshot.counters.last_seen_us as u32,
            last_seen_us_hi: (snapshot.counters.last_seen_us >> 32) as u32,
            ..Self::zeroed()
        };
        report.seal();
        report
    }

    pub fn from_supervisor<const HEALTH_SLOTS: usize, const LOG_SLOTS: usize>(
        supervisor: &Supervisor<HEALTH_SLOTS, LOG_SLOTS>,
        module: ModuleId,
    ) -> Option<Self> {
        let snapshot = supervisor.snapshot(module)?;
        Some(Self::from_snapshot(
            snapshot,
            supervisor.events().count_at_or_above(EventSeverity::Error) as u32,
            supervisor.events().count_at_or_above(EventSeverity::Fatal) as u32,
        ))
    }

    pub fn last_seen_us(&self) -> u64 {
        (u64::from(self.last_seen_us_hi) << 32) | u64::from(self.last_seen_us_lo)
    }

    pub fn seal(&mut self) {
        self.magic = HEALTH_REPORT_MAGIC;
        self.version = HEALTH_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == HEALTH_REPORT_MAGIC
            && self.version == HEALTH_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.module_tag
            ^ self.total_errors
            ^ self.consecutive_errors
            ^ self.last_error
            ^ self.last_action
            ^ self.event_count
            ^ self.dropped_events
            ^ self.error_events
            ^ self.fatal_events
            ^ self.last_seen_us_lo
            ^ self.last_seen_us_hi
    }
}

pub const fn module_tag(module: ModuleId) -> u32 {
    match module {
        ModuleId::Kernel => 1,
        ModuleId::Hal => 2,
        ModuleId::Bus => 3,
        ModuleId::Radio => 4,
        ModuleId::Sensor => 5,
        ModuleId::Actuator => 6,
        ModuleId::Stream => 7,
        ModuleId::Crypto => 8,
        ModuleId::App(id) => 0x100 + id as u32,
    }
}

pub const fn error_code(error: KernelError) -> u32 {
    match error {
        KernelError::LeaseConflict => 1,
        KernelError::BusTimeout => 2,
        KernelError::RadioTxFail => 3,
        KernelError::SensorReadFail => 4,
        KernelError::DeadlineMissed => 5,
    }
}

pub const fn action_code(action: Action) -> u32 {
    match action {
        Action::RetryNow => 1,
        Action::RetryDelay(delay_us) => 0x1000_0000 | delay_us,
        Action::NotifyUserTask => 2,
        Action::RebootModule => 3,
        Action::Ignore => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FaultThresholds, Supervisor};

    #[test]
    fn health_report_is_built_from_supervisor_snapshot() {
        let mut supervisor = Supervisor::<2, 8>::with_default_policy(FaultThresholds {
            notify_after: 1,
            reboot_after: 3,
        });
        supervisor.record_error(ModuleId::Sensor, KernelError::SensorReadFail, 0x1_0000_0040);

        let report = HealthReport::from_supervisor(&supervisor, ModuleId::Sensor).expect("report");

        assert!(report.verify_checksum());
        assert_eq!(report.module_tag, module_tag(ModuleId::Sensor));
        assert_eq!(report.total_errors, 1);
        assert_eq!(report.consecutive_errors, 1);
        assert_eq!(report.last_error, error_code(KernelError::SensorReadFail));
        assert_eq!(report.last_action, action_code(Action::NotifyUserTask));
        assert_eq!(report.event_count, 2);
        assert_eq!(report.error_events, 2);
        assert_eq!(report.fatal_events, 0);
        assert_eq!(report.last_seen_us(), 0x1_0000_0040);
    }

    #[test]
    fn health_report_detects_corruption() {
        let mut supervisor = Supervisor::<1, 4>::default();
        supervisor.record_error(ModuleId::Radio, KernelError::RadioTxFail, 10);
        let mut report =
            HealthReport::from_supervisor(&supervisor, ModuleId::Radio).expect("report");

        assert!(report.verify_checksum());
        report.total_errors += 1;
        assert!(!report.verify_checksum());
    }

    #[test]
    fn health_report_is_missing_when_module_has_no_snapshot() {
        let supervisor = Supervisor::<1, 4>::default();

        assert_eq!(
            HealthReport::from_supervisor(&supervisor, ModuleId::Sensor),
            None
        );
    }
}
