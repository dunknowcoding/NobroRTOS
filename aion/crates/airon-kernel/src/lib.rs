//! AIRON kernel: Sample envelope, error policy, and scheduling hooks.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod admission;
pub mod alarm;
pub mod capability;
pub mod degrade;
pub mod error;
pub mod eval;
pub mod event_log;
pub mod executor;
pub mod fault_inject;
pub mod health;
pub mod kv;
pub mod lifecycle;
pub mod mailbox;
pub mod manifest;
pub mod module_runtime;
pub mod pool;
pub mod quota;
pub mod recovery;
pub mod report;
pub mod retry;
pub mod runtime;
pub mod sample;
pub mod scheduler;
pub mod startup;
pub mod supervisor;
pub mod watchdog;

pub use admission::{
    AdmissionController, AdmissionError, AdmissionPlan, AdmissionReport, ADMISSION_REPORT_MAGIC,
    ADMISSION_REPORT_VERSION,
};
pub use alarm::{Alarm, AlarmError, AlarmId, AlarmQueue};
pub use capability::{CapabilityGrant, CapabilityGrantError, CapabilityGrantTable};
pub use degrade::{DegradeDecision, DegradeError, DegradePlanner, DegradeReason};
pub use error::{Action, KernelError};
pub use eval::{
    EvalGate, EvalReport, ImuHwEvalReport, SalEvalReport, EVAL_MAGIC, IMU_HW_EVAL_MAGIC,
    MAX_JITTER_US, MIN_DEADLINE_TICKS, MIN_IMU_HW_READS, MIN_IMU_SAMPLES, MIN_SERVO_STEPS,
    SAL_EVAL_MAGIC, SERVO_READBACK_TOL_US,
};
pub use event_log::{EventKind, EventLog, EventPayload, EventRecord, EventSeverity};
pub use executor::{I2cPollTask, Poll, StatsTask, Task, TaskMeta, TaskSlot, TaskStats, TaskTable};
pub use fault_inject::{FaultInjectError, FaultInjector, FaultMode, FaultRule};
pub use health::{FaultThresholds, HealthCounters, HealthMonitor, HealthSlot, ModuleId};
pub use kv::{KvEntry, KvError, KvKey, KvStore, KvValue};
pub use lifecycle::{Lifecycle, LifecycleError, SystemState};
pub use mailbox::{Mailbox, MailboxError, Message, MessageKind};
pub use manifest::{
    kernel_module_spec, kernel_owned_capabilities, Capability, CapabilitySet, Criticality,
    DeadlineContract, ManifestError, ManifestReport, MemoryBudget, ModuleSpec, SystemBudget,
    SystemManifest, SystemProfile, MANIFEST_REPORT_MAGIC, MANIFEST_REPORT_VERSION,
};
pub use module_runtime::{
    ModuleRunState, ModuleRuntimeEntry, ModuleRuntimeError, ModuleRuntimeGuard,
};
pub use pool::{ImuPayload, SamplePool};
pub use quota::{QuotaEntry, QuotaError, QuotaLedger};
pub use recovery::{RecoveryCoordinator, RecoveryError, RecoveryOutcome};
pub use report::{
    action_code, degrade_reason_code, error_code, event_kind_code, module_run_state_code,
    module_tag, payload_fields, severity_code, state_code, DegradeApplicationReport,
    EventLogReport, HealthReport, ModuleRuntimeReport, RuntimeReport, RuntimeReportInput,
    DEGRADE_APPLICATION_REPORT_MAGIC, DEGRADE_APPLICATION_REPORT_VERSION, EVENT_LOG_REPORT_MAGIC,
    EVENT_LOG_REPORT_VERSION, HEALTH_REPORT_MAGIC, HEALTH_REPORT_VERSION,
    MODULE_RUNTIME_REPORT_MAGIC, MODULE_RUNTIME_REPORT_VERSION, RUNTIME_REPORT_MAGIC,
    RUNTIME_REPORT_VERSION,
};
pub use retry::{BackoffKind, RetryPolicy, RetryState};
pub use runtime::{AlarmDispatch, DegradeApplication, Runtime, RuntimeError, WatchdogSweep};
pub use sample::{PoolHandle, Sample, SampleKind, SAMPLE_POOL_SIZE};
pub use scheduler::{Scheduler, Timer, DEADLINE_PERIOD_US};
pub use startup::{
    DependencySet, StartupError, StartupGraph, StartupGraphError, StartupNode, StartupPlan,
    StartupPlanner,
};
pub use supervisor::{Supervisor, SupervisorSnapshot};
pub use watchdog::{Watchdog, WatchdogEntry, WatchdogError};
