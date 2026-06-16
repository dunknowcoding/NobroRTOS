//! AIRON kernel — Sample envelope, error policy, scheduling hooks (Phase 0 subset).

#![no_std]

pub mod error;
pub mod sample;

pub use error::{Action, KernelError};
pub use sample::{PoolHandle, Sample, SampleKind, SAMPLE_POOL_SIZE};
