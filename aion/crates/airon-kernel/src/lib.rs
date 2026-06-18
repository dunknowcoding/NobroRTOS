//! AIRON kernel: Sample envelope, error policy, and scheduling hooks.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod error;
pub mod eval;
pub mod event_log;
pub mod executor;
pub mod health;
pub mod manifest;
pub mod pool;
pub mod sample;
pub mod scheduler;
pub mod supervisor;

pub use error::{Action, KernelError};
pub use eval::{
    EvalGate, EvalReport, ImuHwEvalReport, SalEvalReport, EVAL_MAGIC, IMU_HW_EVAL_MAGIC,
    MAX_JITTER_US, MIN_DEADLINE_TICKS, MIN_IMU_HW_READS, MIN_IMU_SAMPLES, MIN_SERVO_STEPS,
    SAL_EVAL_MAGIC, SERVO_READBACK_TOL_US,
};
pub use event_log::{EventKind, EventLog, EventPayload, EventRecord, EventSeverity};
pub use executor::{I2cPollTask, Poll, StatsTask, Task, TaskMeta, TaskSlot, TaskStats, TaskTable};
pub use health::{FaultThresholds, HealthCounters, HealthMonitor, HealthSlot, ModuleId};
pub use manifest::{
    kernel_owned_capabilities, Capability, CapabilitySet, Criticality, DeadlineContract,
    ManifestError, MemoryBudget, ModuleSpec, SystemBudget, SystemManifest, SystemProfile,
};
pub use pool::{ImuPayload, SamplePool};
pub use sample::{PoolHandle, Sample, SampleKind, SAMPLE_POOL_SIZE};
pub use scheduler::{Scheduler, Timer, DEADLINE_PERIOD_US};
pub use supervisor::{Supervisor, SupervisorSnapshot};
