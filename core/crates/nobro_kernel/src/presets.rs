//! Coherent capacity presets and their zero-allocation composition boundaries.
//!
//! Use these aliases when their retained services match the application. Custom
//! const-generic runtimes remain supported and are validated during assembly.

pub use crate::{
    L0NanoKernel, L1GuardedKernel, L2ManagedKernel, L3AssuredKernel, L3AssuredKernelCell,
    LargeRuntime, LeanKernelExecutor, LeanKernelExecutorCell, LeanRuntime, SmallRuntime,
    StandardRuntime,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_and_module_aliases_are_layout_identical() {
        assert_eq!(
            core::mem::size_of::<SmallRuntime>(),
            core::mem::size_of::<crate::SmallRuntime>()
        );
        assert_eq!(
            core::mem::size_of::<L3AssuredKernel>(),
            core::mem::size_of::<crate::L3AssuredKernel>()
        );
    }
}
