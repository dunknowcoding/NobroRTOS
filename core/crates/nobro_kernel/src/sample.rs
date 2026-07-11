//! Unified inter-module data envelope (replaces v1.0 TensorBuffer).

/// Opaque handle into the static sample pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PoolHandle(pub u32);

impl PoolHandle {
    pub const INVALID: Self = Self(u32::MAX);

    pub fn is_valid(self) -> bool {
        self.0 != u32::MAX && self.generation() != 0
    }

    pub(crate) const fn from_parts(index: usize, generation: u32) -> Self {
        Self((generation << 8) | index as u32)
    }

    pub(crate) const fn index(self) -> usize {
        (self.0 & 0xFF) as usize
    }

    pub(crate) const fn generation(self) -> u32 {
        self.0 >> 8
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
#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub handle: PoolHandle,
    pub len: u16,
    pub kind: SampleKind,
    pub captured_us: u64,
    pub deadline_us: u64,
}

/// Default static pool slot count (Phase 0); DMA pool wired in Phase 1.
pub const SAMPLE_POOL_SIZE: usize = 8;
