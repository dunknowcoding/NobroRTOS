//! Fixed-layout reports emitted by firmware and decoded by host tools.

use crate::{
    manifest::module_code, Action, EventKind, EventLog, EventPayload, EventRecord, EventSeverity,
    KernelError, ModuleId, ModuleRunState, ModuleRuntimeEntry, ModuleRuntimeGuard, Supervisor,
    SupervisorSnapshot, SystemBudget, SystemState,
};
use crate::{DegradeApplication, DegradeReason};

pub const HEALTH_REPORT_MAGIC: u32 = 0x4E42_484C; // "NBHL"
pub const HEALTH_REPORT_VERSION: u32 = 1;
pub const RUNTIME_REPORT_MAGIC: u32 = 0x4E42_5254; // "NBRT"
pub const RUNTIME_REPORT_VERSION: u32 = 1;
pub const EVENT_LOG_REPORT_MAGIC: u32 = 0x4E42_454C; // "NBEL"
pub const EVENT_LOG_REPORT_VERSION: u32 = 1;
pub const MODULE_RUNTIME_REPORT_MAGIC: u32 = 0x4E42_4D52; // "NBMR"
pub const MODULE_RUNTIME_REPORT_VERSION: u32 = 1;
pub const DEGRADE_APPLICATION_REPORT_MAGIC: u32 = 0x4E42_4447; // "NBDG"
pub const DEGRADE_APPLICATION_REPORT_VERSION: u32 = 1;

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

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModuleRuntimeReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub module_count: u32,
    pub capacity: u32,
    pub active_count: u32,
    pub suspended_count: u32,
    pub faulted_count: u32,
    pub recovering_count: u32,
    pub disabled_count: u32,
    pub latest_module_tag: u32,
    pub latest_state: u32,
    pub latest_fault_count: u32,
    pub latest_recovery_count: u32,
    pub latest_change_us_lo: u32,
    pub latest_change_us_hi: u32,
    pub checksum: u32,
}

impl ModuleRuntimeReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            module_count: 0,
            capacity: 0,
            active_count: 0,
            suspended_count: 0,
            faulted_count: 0,
            recovering_count: 0,
            disabled_count: 0,
            latest_module_tag: 0,
            latest_state: 0,
            latest_fault_count: 0,
            latest_recovery_count: 0,
            latest_change_us_lo: 0,
            latest_change_us_hi: 0,
            checksum: 0,
        }
    }

    pub fn from_guard<const N: usize>(guard: &ModuleRuntimeGuard<N>) -> Self {
        let mut report = Self {
            module_count: guard.len() as u32,
            capacity: guard.capacity() as u32,
            active_count: guard.count_state(ModuleRunState::Active) as u32,
            suspended_count: guard.count_state(ModuleRunState::Suspended) as u32,
            faulted_count: guard.count_state(ModuleRunState::Faulted) as u32,
            recovering_count: guard.count_state(ModuleRunState::Recovering) as u32,
            disabled_count: guard.count_state(ModuleRunState::Disabled) as u32,
            ..Self::zeroed()
        };
        if let Some(entry) = guard.latest_changed() {
            report.write_latest(entry);
        }
        report.seal();
        report
    }

    fn write_latest(&mut self, entry: ModuleRuntimeEntry) {
        self.latest_module_tag = module_tag(entry.module);
        self.latest_state = module_run_state_code(entry.state);
        self.latest_fault_count = entry.fault_count;
        self.latest_recovery_count = entry.recovery_count;
        self.latest_change_us_lo = entry.last_change_us as u32;
        self.latest_change_us_hi = (entry.last_change_us >> 32) as u32;
    }

    pub fn latest_change_us(&self) -> u64 {
        (u64::from(self.latest_change_us_hi) << 32) | u64::from(self.latest_change_us_lo)
    }

    pub fn seal(&mut self) {
        self.magic = MODULE_RUNTIME_REPORT_MAGIC;
        self.version = MODULE_RUNTIME_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == MODULE_RUNTIME_REPORT_MAGIC
            && self.version == MODULE_RUNTIME_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.module_count
            ^ self.capacity
            ^ self.active_count
            ^ self.suspended_count
            ^ self.faulted_count
            ^ self.recovering_count
            ^ self.disabled_count
            ^ self.latest_module_tag
            ^ self.latest_state
            ^ self.latest_fault_count
            ^ self.latest_recovery_count
            ^ self.latest_change_us_lo
            ^ self.latest_change_us_hi
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DegradeApplicationReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub requested_count: u32,
    pub disabled_count: u32,
    pub already_disabled_count: u32,
    pub reason: u32,
    pub applied_at_us_lo: u32,
    pub applied_at_us_hi: u32,
    pub checksum: u32,
}

impl DegradeApplicationReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            requested_count: 0,
            disabled_count: 0,
            already_disabled_count: 0,
            reason: 0,
            applied_at_us_lo: 0,
            applied_at_us_hi: 0,
            checksum: 0,
        }
    }

    pub fn from_application(application: DegradeApplication) -> Self {
        let mut report = Self {
            requested_count: application.requested as u32,
            disabled_count: application.disabled as u32,
            already_disabled_count: application.already_disabled as u32,
            reason: degrade_reason_code(application.reason),
            applied_at_us_lo: application.applied_at_us as u32,
            applied_at_us_hi: (application.applied_at_us >> 32) as u32,
            ..Self::zeroed()
        };
        report.seal();
        report
    }

    pub fn applied_at_us(&self) -> u64 {
        (u64::from(self.applied_at_us_hi) << 32) | u64::from(self.applied_at_us_lo)
    }

    pub fn seal(&mut self) {
        self.magic = DEGRADE_APPLICATION_REPORT_MAGIC;
        self.version = DEGRADE_APPLICATION_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == DEGRADE_APPLICATION_REPORT_MAGIC
            && self.version == DEGRADE_APPLICATION_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.requested_count
            ^ self.disabled_count
            ^ self.already_disabled_count
            ^ self.reason
            ^ self.applied_at_us_lo
            ^ self.applied_at_us_hi
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventLogReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub event_count: u32,
    pub capacity: u32,
    pub dropped_events: u32,
    pub latest_seq: u32,
    pub latest_at_us_lo: u32,
    pub latest_at_us_hi: u32,
    pub latest_module_tag: u32,
    pub latest_severity: u32,
    pub latest_kind: u32,
    pub latest_payload_kind: u32,
    pub latest_payload0: u32,
    pub latest_payload1: u32,
    pub checksum: u32,
}

impl EventLogReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            event_count: 0,
            capacity: 0,
            dropped_events: 0,
            latest_seq: 0,
            latest_at_us_lo: 0,
            latest_at_us_hi: 0,
            latest_module_tag: 0,
            latest_severity: 0,
            latest_kind: 0,
            latest_payload_kind: 0,
            latest_payload0: 0,
            latest_payload1: 0,
            checksum: 0,
        }
    }

    pub fn from_event_log<const N: usize>(events: &EventLog<N>) -> Self {
        let mut report = Self {
            event_count: events.len() as u32,
            capacity: events.capacity() as u32,
            dropped_events: events.dropped(),
            ..Self::zeroed()
        };
        if let Some(record) = events.latest() {
            report.write_latest(record);
        }
        report.seal();
        report
    }

    fn write_latest(&mut self, record: EventRecord) {
        let (payload_kind, payload0, payload1) = payload_fields(record.payload);
        self.latest_seq = record.seq;
        self.latest_at_us_lo = record.at_us as u32;
        self.latest_at_us_hi = (record.at_us >> 32) as u32;
        self.latest_module_tag = module_tag(record.module);
        self.latest_severity = severity_code(record.severity);
        self.latest_kind = event_kind_code(record.kind);
        self.latest_payload_kind = payload_kind;
        self.latest_payload0 = payload0;
        self.latest_payload1 = payload1;
    }

    pub fn latest_at_us(&self) -> u64 {
        (u64::from(self.latest_at_us_hi) << 32) | u64::from(self.latest_at_us_lo)
    }

    pub fn seal(&mut self) {
        self.magic = EVENT_LOG_REPORT_MAGIC;
        self.version = EVENT_LOG_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == EVENT_LOG_REPORT_MAGIC
            && self.version == EVENT_LOG_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.event_count
            ^ self.capacity
            ^ self.dropped_events
            ^ self.latest_seq
            ^ self.latest_at_us_lo
            ^ self.latest_at_us_hi
            ^ self.latest_module_tag
            ^ self.latest_severity
            ^ self.latest_kind
            ^ self.latest_payload_kind
            ^ self.latest_payload0
            ^ self.latest_payload1
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

pub const fn module_run_state_code(state: ModuleRunState) -> u32 {
    match state {
        ModuleRunState::Registered => 1,
        ModuleRunState::Active => 2,
        ModuleRunState::Suspended => 3,
        ModuleRunState::Faulted => 4,
        ModuleRunState::Recovering => 5,
        ModuleRunState::Disabled => 6,
    }
}

pub const fn degrade_reason_code(reason: Option<DegradeReason>) -> u32 {
    match reason {
        None => 0,
        Some(DegradeReason::FlashBudget) => 1,
        Some(DegradeReason::RamBudget) => 2,
        Some(DegradeReason::PoolBudget) => 3,
        Some(DegradeReason::ModuleLimit) => 4,
    }
}

pub const fn error_code(error: KernelError) -> u32 {
    match error {
        KernelError::LeaseConflict => 1,
        KernelError::BusTimeout => 2,
        KernelError::RadioTxFail => 3,
        KernelError::SensorReadFail => 4,
        KernelError::DeadlineMissed => 5,
        KernelError::ForeignModuleInitFail => 6,
        KernelError::ForeignModulePollFail => 7,
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

pub const fn severity_code(severity: EventSeverity) -> u32 {
    match severity {
        EventSeverity::Trace => 0,
        EventSeverity::Info => 1,
        EventSeverity::Warn => 2,
        EventSeverity::Error => 3,
        EventSeverity::Fatal => 4,
    }
}

pub const fn event_kind_code(kind: EventKind) -> u32 {
    match kind {
        EventKind::Boot => 1,
        EventKind::Health => 2,
        EventKind::Recovery => 3,
        EventKind::TaskOverrun => 4,
        EventKind::Lease => 5,
        EventKind::SamplePool => 6,
        EventKind::Manifest => 7,
        EventKind::Host => 8,
    }
}

pub const fn payload_fields(payload: EventPayload) -> (u32, u32, u32) {
    match payload {
        EventPayload::None => (0, 0, 0),
        EventPayload::Error(error) => (1, error_code(error), 0),
        EventPayload::Action(action) => (2, action_code(action), 0),
        EventPayload::Counter(value) => (3, value, 0),
        EventPayload::Pair(left, right) => (4, left, right),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventLog, EventPayload, FaultThresholds, ModuleRuntimeGuard, Supervisor};

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

    #[test]
    fn module_runtime_report_summarizes_guard() {
        let mut guard = ModuleRuntimeGuard::<3>::new();
        guard.register(ModuleId::Kernel, 0x1_0000_0000).unwrap();
        guard.register(ModuleId::Sensor, 0x1_0000_0010).unwrap();
        guard.register(ModuleId::Radio, 0x1_0000_0020).unwrap();
        guard.activate_all(0x1_0000_0040).unwrap();
        guard.suspend(ModuleId::Radio, 0x1_0000_0080).unwrap();
        guard
            .note_recovery_outcome(
                crate::RecoveryOutcome {
                    module: ModuleId::Sensor,
                    error: KernelError::SensorReadFail,
                    action: Action::NotifyUserTask,
                    state: SystemState::Degraded,
                    coalesced: false,
                },
                0x1_0000_00C0,
            )
            .unwrap();

        let report = ModuleRuntimeReport::from_guard(&guard);

        assert!(report.verify_checksum());
        assert_eq!(report.magic, MODULE_RUNTIME_REPORT_MAGIC);
        assert_eq!(report.module_count, 3);
        assert_eq!(report.capacity, 3);
        assert_eq!(report.active_count, 1);
        assert_eq!(report.suspended_count, 1);
        assert_eq!(report.faulted_count, 1);
        assert_eq!(report.recovering_count, 0);
        assert_eq!(report.disabled_count, 0);
        assert_eq!(report.latest_module_tag, module_tag(ModuleId::Sensor));
        assert_eq!(
            report.latest_state,
            module_run_state_code(ModuleRunState::Faulted)
        );
        assert_eq!(report.latest_fault_count, 1);
        assert_eq!(report.latest_change_us(), 0x1_0000_00C0);
    }

    #[test]
    fn degrade_application_report_summarizes_application() {
        let report = DegradeApplicationReport::from_application(DegradeApplication {
            requested: 3,
            disabled: 2,
            already_disabled: 1,
            reason: Some(DegradeReason::RamBudget),
            applied_at_us: 0x1_0000_0040,
        });

        assert!(report.verify_checksum());
        assert_eq!(report.magic, DEGRADE_APPLICATION_REPORT_MAGIC);
        assert_eq!(report.requested_count, 3);
        assert_eq!(report.disabled_count, 2);
        assert_eq!(report.already_disabled_count, 1);
        assert_eq!(
            report.reason,
            degrade_reason_code(Some(DegradeReason::RamBudget))
        );
        assert_eq!(report.applied_at_us(), 0x1_0000_0040);
    }

    #[test]
    fn event_log_report_summarizes_latest_event() {
        let mut events = EventLog::<2>::new();
        events.push(EventRecord::new(
            0x1_0000_0040,
            ModuleId::Sensor,
            EventSeverity::Error,
            EventKind::Health,
            EventPayload::Error(KernelError::SensorReadFail),
        ));
        events.push(EventRecord::new(
            0x1_0000_0080,
            ModuleId::Sensor,
            EventSeverity::Warn,
            EventKind::Recovery,
            EventPayload::Action(Action::RetryNow),
        ));

        let report = EventLogReport::from_event_log(&events);

        assert!(report.verify_checksum());
        assert_eq!(report.magic, EVENT_LOG_REPORT_MAGIC);
        assert_eq!(report.event_count, 2);
        assert_eq!(report.capacity, 2);
        assert_eq!(report.dropped_events, 0);
        assert_eq!(report.latest_seq, 2);
        assert_eq!(report.latest_at_us(), 0x1_0000_0080);
        assert_eq!(report.latest_module_tag, module_tag(ModuleId::Sensor));
        assert_eq!(report.latest_severity, severity_code(EventSeverity::Warn));
        assert_eq!(report.latest_kind, event_kind_code(EventKind::Recovery));
        assert_eq!(report.latest_payload_kind, 2);
        assert_eq!(report.latest_payload0, action_code(Action::RetryNow));
    }

    #[test]
    fn event_log_report_detects_corruption() {
        let events = EventLog::<1>::new();
        let mut report = EventLogReport::from_event_log(&events);

        assert!(report.verify_checksum());
        assert_eq!(report.event_count, 0);
        assert_eq!(report.capacity, 1);

        report.event_count = 1;
        assert!(!report.verify_checksum());
    }
}
