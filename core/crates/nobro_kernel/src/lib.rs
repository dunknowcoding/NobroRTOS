//! NobroRTOS kernel: Sample envelope, error policy, and scheduling hooks.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod admission;
pub mod alarm;
pub mod async_exec;
pub mod boot;
pub mod capability;
pub mod degrade;
pub mod error;
pub mod eval;
pub mod event_log;
pub mod executor;
pub mod fault_inject;
pub mod foreign_host;
pub mod foreign_module;
pub mod health;
pub mod hot_reload;
pub mod kernel_executor;
pub mod kv;
pub mod launch_gate;
pub mod lifecycle;
pub mod lifecycle_hooks;
pub mod mailbox;
pub mod manifest;
pub mod module_ctx;
pub mod module_runtime;
pub mod objects;
pub mod pool;
pub mod quota;
pub mod recovery;
pub mod report;
pub mod retry;
pub mod runtime;
pub mod sample;
pub mod scheduler;
pub mod stack_guard;
pub mod startup;
pub mod supervisor;
pub mod task_supervisor;
pub mod watchdog;

pub use admission::{
    AdmissionController, AdmissionError, AdmissionPlan, AdmissionReport, ADMISSION_REPORT_MAGIC,
    ADMISSION_REPORT_VERSION,
};
pub use alarm::{Alarm, AlarmError, AlarmId, AlarmQueue};
pub use async_exec::{AsyncError, BoundedExecutor, RunStats, SpawnedTask};
pub use boot::{
    BootAssembly, BootAssemblyError, BootAssemblyFailure, BootAssemblyReports, StartupDependency,
};
pub use capability::{
    CapabilityGrant, CapabilityGrantError, CapabilityGrantTable, CapabilityReplayScope,
    CapabilityTrace, CapabilityTraceError, CapabilityTraceInput, CapabilityTraceOp,
    CapabilityTraceRecord,
};
pub use degrade::{DegradeDecision, DegradeError, DegradePlanner, DegradeReason};
pub use error::{Action, KernelError};
pub use eval::{
    EvalGate, EvalReport, ImuHwEvalReport, SalEvalReport, EVAL_MAGIC, IMU_HW_EVAL_MAGIC,
    MAX_JITTER_US, MIN_DEADLINE_TICKS, MIN_IMU_HW_READS, MIN_IMU_SAMPLES, MIN_SERVO_STEPS,
    SAL_EVAL_MAGIC, SERVO_READBACK_TOL_US,
};
pub use event_log::{EventKind, EventLog, EventPayload, EventRecord, EventSeverity};
pub use executor::{
    I2cPollTask, Poll, StatsTask, Task, TaskMeta, TaskSlot, TaskStats, TaskTable, TaskTableError,
};
pub use fault_inject::{FaultInjectError, FaultInjector, FaultMode, FaultRule};
pub use foreign_host::{
    ForeignHostCall, ForeignHostContext, ForeignHostError, ForeignHostQuota, ForeignHostUsage,
};
pub use foreign_module::{ForeignModuleError, ForeignModuleRunner, ForeignModuleState};
pub use health::{
    FaultThresholdError, FaultThresholds, HealthCounters, HealthMonitor, HealthSlot, ModuleId,
};
pub use hot_reload::{
    HotReloadError, HotReloadOutcome, HotReloadPlan, HotReloadPolicy, HotReloadStep,
    HotReloadStepKind, LeaseReleaser, ModuleReloadRequest, NoopLeaseReleaser,
};
pub use kernel_executor::{
    ContainmentPolicy, CycleOutcome, ExecError, ExecutionSentinel, KernelExecutor, StuckPoll,
};
pub use kv::{KvEntry, KvError, KvKey, KvStore, KvValue};
pub use launch_gate::ModuleLaunchGate;
pub use lifecycle::{Lifecycle, LifecycleError, SystemState};
pub use lifecycle_hooks::{ModuleHookError, ModuleLifecycleHooks, ModuleReloadHooks};
pub use mailbox::{Mailbox, MailboxError, Message, MessageKind};
pub use manifest::{
    kernel_module_spec, kernel_owned_capabilities, module_code, Capability, CapabilitySet,
    Criticality, DeadlineContract, ManifestError, ManifestReport, MemoryBudget, ModuleSpec,
    ObjectQuota, SystemBudget, SystemManifest, SystemProfile, MANIFEST_REPORT_MAGIC,
    MANIFEST_REPORT_VERSION,
};
pub use module_ctx::ModuleCtx;
pub use module_runtime::{
    ModuleRunState, ModuleRuntimeEntry, ModuleRuntimeError, ModuleRuntimeGuard,
};
pub use objects::{ObjectKind, ObjectLedger, ObjectQuotaError, ObjectUsage};
pub use pool::{ImuPayload, SamplePool};
pub use quota::{QuotaEntry, QuotaError, QuotaLedger};
pub use recovery::{
    RecoveryCoordinator, RecoveryError, RecoveryOutcome, RecoveryPlan, RecoveryPlanDispatch,
    RecoveryPlanError, RecoveryPlanExecution, RecoveryPlanPolicy, RecoveryStep, RecoveryStepKind,
    RecoveryStormPolicy,
};
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
pub use runtime::{
    AlarmDispatch, CapacityError, DegradeApplication, RecoveryPlanning, Runtime, RuntimeCapacities,
    RuntimeError, WatchdogSweep,
};

/// Preset runtime capacity profiles — coherent by construction, so users pick a
/// size instead of juggling seven const generics. Custom instantiations are
/// still validated when the runtime is assembled.
pub type SmallRuntime = Runtime<4, 4, 8, 4, 8, 4, 16>;
pub type StandardRuntime = Runtime<8, 8, 16, 8, 16, 8, 32>;
pub type LargeRuntime = Runtime<16, 16, 32, 16, 32, 16, 64>;
pub use sample::{PoolHandle, Sample, SampleKind, SAMPLE_POOL_SIZE};
pub use scheduler::{Scheduler, Timer, DEADLINE_PERIOD_US};
pub use stack_guard::{
    StackFault, StackGuardError, StackGuardTable, StackRegion, StackStatus, DEFAULT_CANARY_BYTES,
    WATERMARK_PATTERN,
};
pub use startup::{
    DependencyImpact, DependencySet, StartupError, StartupGraph, StartupGraphError, StartupNode,
    StartupPlan, StartupPlanner,
};
pub use supervisor::{Supervisor, SupervisorSnapshot};
pub use task_supervisor::{SupervisionAction, TaskSupervisor};
pub use watchdog::{Watchdog, WatchdogEntry, WatchdogError};

#[cfg(test)]
mod property_tests {
    //! Property-based tests (M171): a deterministic xorshift generator drives thousands
    //! of random operation sequences against kernel data structures, asserting
    //! invariants hold for every sequence. No external proptest dependency.
    use crate::quota::{QuotaError, QuotaLedger};
    use crate::{ModuleId, SystemBudget};

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn below(&mut self, n: u32) -> u32 {
            (self.next() % u64::from(n)) as u32
        }
    }

    const MODS: [ModuleId; 3] = [ModuleId::Sensor, ModuleId::Radio, ModuleId::Crypto];

    #[test]
    fn quota_ledger_never_exceeds_limits_under_random_ops() {
        for seed in 1..=200u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15));
            let mut ledger = QuotaLedger::<3>::new();
            let limit = SystemBudget::new(4096, 1024, 8);
            for &m in &MODS {
                ledger.register(m, limit).unwrap();
            }
            // shadow model of what we believe is reserved per module
            let mut used = [0i64; 3];
            for _ in 0..300 {
                let mi = rng.below(3) as usize;
                let m = MODS[mi];
                let ram = rng.below(200);
                if rng.below(2) == 0 {
                    // reserve
                    let amt = SystemBudget::new(0, ram, 0); // RAM-only: sole constraint
                    match ledger.reserve(m, amt) {
                        Ok(()) => {
                            used[mi] += i64::from(ram);
                            // INVARIANT: accepted reservation stays within the RAM limit
                            assert!(used[mi] <= 1024, "seed {seed}: over limit {}", used[mi]);
                        }
                        Err(QuotaError::Exceeded { .. }) => {
                            // INVARIANT: rejection only when it WOULD exceed
                            assert!(used[mi] + i64::from(ram) > 1024);
                        }
                        Err(_) => {}
                    }
                } else {
                    // release up to what we've reserved
                    let rel = rng.below(200).min(used[mi] as u32);
                    let amt = SystemBudget::new(0, rel, 0);
                    if ledger.release(m, amt).is_ok() {
                        used[mi] -= i64::from(rel);
                        assert!(used[mi] >= 0, "seed {seed}: negative usage");
                    }
                }
                // INVARIANT: reported usage always matches our shadow model
                let reported = ledger.usage(m).map(|b| i64::from(b.ram_bytes)).unwrap_or(0);
                assert_eq!(reported, used[mi], "seed {seed}: usage mismatch for {m:?}");
            }
        }
    }
}
