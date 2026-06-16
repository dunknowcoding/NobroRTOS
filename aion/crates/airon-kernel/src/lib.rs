//! AIRON kernel — Sample envelope, error policy, scheduling hooks (Phase 0 subset).

#![no_std]

pub mod eval;
pub mod error;
pub mod executor;
pub mod sample;
pub mod scheduler;

pub use error::{Action, KernelError};
pub use executor::{I2cPollTask, Poll, StatsTask, Task};
pub use sample::{PoolHandle, Sample, SampleKind, SAMPLE_POOL_SIZE};
pub use eval::{EvalGate, EvalReport, EVAL_MAGIC, MAX_JITTER_US, MIN_DEADLINE_TICKS};
pub use scheduler::{Scheduler, Timer, DEADLINE_PERIOD_US};
