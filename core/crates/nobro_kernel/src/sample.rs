//! Unified inter-module data envelope (replaces v1.0 TensorBuffer).

/// Opaque handle into the static sample pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PoolHandle(pub u16);

impl PoolHandle {
    pub const INVALID: Self = Self(u16::MAX);

    pub fn is_valid(self) -> bool {
        self.0 != u16::MAX
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum SampleKind {
    #[default]
    Imu = 0,
    Range = 1,
    Power = 2,
    RadioRx = 3,
    Tensor = 4,
    Raw = 5,
}

/// All modules pass `Sample` tickets, not raw pointers across crate boundaries.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub handle: PoolHandle,
    pub len: u16,
    pub kind: SampleKind,
    pub captured_us: u64,
    pub deadline_us: u64,
}

/// Default static pool slot count (Phase 0); DMA pool wired in Phase 1.
pub const SAMPLE_POOL_SIZE: usize = 8;
