//! Fixed-layout reports emitted by firmware and decoded by host tools.

use crate::{
    manifest::module_code, Action, EventSeverity, KernelError, ModuleId, Supervisor,
    SupervisorSnapshot, SystemBudget, SystemState,
};

pub const HEALTH_REPORT_MAGIC: u32 = 0x4152_484C; // "ARHL"
pub const HEALTH_REPORT_VERSION: u32 = 1;
pub const RUNTIME_REPORT_MAGIC: u32 = 0x4152_5254; // "ARRT"
pub const RUNTIME_REPORT_VERSION: u32 = 1;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeReportInput {
    pub state: SystemState,
    pub module_count: u32,
    pub mailbox_len: u32,
    pub mailbox_dropped: u32,
    pub alarm_len: u32,
    pub next_alarm_due_us: u64,
    pub kv_len: u32,
    pub kv_writes: u32,
    pub kv_deletes: u32,
    pub quota_used: SystemBudget,
    pub event_count: u32,
    pub dropped_events: u32,
}

impl Default for RuntimeReportInput {
    fn default() -> Self {
        Self {
            state: SystemState::ColdBoot,
            module_count: 0,
            mailbox_len: 0,
            mailbox_dropped: 0,
            alarm_len: 0,
            next_alarm_due_us: 0,
            kv_len: 0,
            kv_writes: 0,
            kv_deletes: 0,
            quota_used: SystemBudget::ZERO,
            event_count: 0,
            dropped_events: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub state: u32,
    pub module_count: u32,
    pub mailbox_len: u32,
    pub mailbox_dropped: u32,
    pub alarm_len: u32,
    pub next_alarm_due_us_lo: u32,
    pub next_alarm_due_us_hi: u32,
    pub kv_len: u32,
    pub kv_writes: u32,
    pub kv_deletes: u32,
    pub quota_flash_used_bytes: u32,
    pub quota_ram_used_bytes: u32,
    pub quota_pool_used_slots: u32,
    pub event_count: u32,
    pub dropped_events: u32,
    pub checksum: u32,
}

impl RuntimeReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            state: 0,
            module_count: 0,
            mailbox_len: 0,
            mailbox_dropped: 0,
            alarm_len: 0,
            next_alarm_due_us_lo: 0,
            next_alarm_due_us_hi: 0,
            kv_len: 0,
            kv_writes: 0,
            kv_deletes: 0,
            quota_flash_used_bytes: 0,
            quota_ram_used_bytes: 0,
            quota_pool_used_slots: 0,
            event_count: 0,
            dropped_events: 0,
            checksum: 0,
        }
    }

    pub fn from_input(input: RuntimeReportInput) -> Self {
        let mut report = Self {
            state: state_code(input.state),
            module_count: input.module_count,
            mailbox_len: input.mailbox_len,
            mailbox_dropped: input.mailbox_dropped,
            alarm_len: input.alarm_len,
            next_alarm_due_us_lo: input.next_alarm_due_us as u32,
            next_alarm_due_us_hi: (input.next_alarm_due_us >> 32) as u32,
            kv_len: input.kv_len,
            kv_writes: input.kv_writes,
            kv_deletes: input.kv_deletes,
            quota_flash_used_bytes: input.quota_used.flash_bytes,
            quota_ram_used_bytes: input.quota_used.ram_bytes,
            quota_pool_used_slots: u32::from(input.quota_used.pool_slots),
            event_count: input.event_count,
            dropped_events: input.dropped_events,
            ..Self::zeroed()
        };
        report.seal();
        report
    }

    pub fn next_alarm_due_us(&self) -> u64 {
        (u64::from(self.next_alarm_due_us_hi) << 32) | u64::from(self.next_alarm_due_us_lo)
    }

    pub fn seal(&mut self) {
        self.magic = RUNTIME_REPORT_MAGIC;
        self.version = RUNTIME_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == RUNTIME_REPORT_MAGIC
            && self.version == RUNTIME_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.state
            ^ self.module_count
            ^ self.mailbox_len
            ^ self.mailbox_dropped
            ^ self.alarm_len
            ^ self.next_alarm_due_us_lo
            ^ self.next_alarm_due_us_hi
            ^ self.kv_len
            ^ self.kv_writes
            ^ self.kv_deletes
            ^ self.quota_flash_used_bytes
            ^ self.quota_ram_used_bytes
            ^ self.quota_pool_used_slots
            ^ self.event_count
            ^ self.dropped_events
    }
}

pub const fn module_tag(module: ModuleId) -> u32 {
    module_code(module)
}

pub const fn state_code(state: SystemState) -> u32 {
    match state {
        SystemState::ColdBoot => 0,
        SystemState::ValidateManifest => 1,
        SystemState::InitDrivers => 2,
        SystemState::Running => 3,
        SystemState::Degraded => 4,
        SystemState::Recovering => 5,
        SystemState::Halted => 6,
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

    #[test]
    fn runtime_report_seals_runtime_control_state() {
        let report = RuntimeReport::from_input(RuntimeReportInput {
            state: SystemState::Running,
            module_count: 3,
            mailbox_len: 2,
            mailbox_dropped: 1,
            alarm_len: 1,
            next_alarm_due_us: 0x1234_5678_9ABC_DEF0,
            kv_len: 4,
            kv_writes: 5,
            kv_deletes: 1,
            quota_used: SystemBudget::new(4096, 1024, 2),
            event_count: 9,
            dropped_events: 1,
        });

        assert!(report.verify_checksum());
        assert_eq!(report.state, state_code(SystemState::Running));
        assert_eq!(report.next_alarm_due_us(), 0x1234_5678_9ABC_DEF0);
        assert_eq!(report.quota_pool_used_slots, 2);
    }

    #[test]
    fn runtime_report_detects_corruption() {
        let mut report = RuntimeReport::from_input(RuntimeReportInput {
            state: SystemState::Degraded,
            module_count: 1,
            ..RuntimeReportInput::default()
        });

        assert!(report.verify_checksum());
        report.module_count += 1;
        assert!(!report.verify_checksum());
    }
}
