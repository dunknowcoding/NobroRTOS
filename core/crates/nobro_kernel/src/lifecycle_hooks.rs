//! Platform-owned executable hooks for module recovery and replacement.

use crate::ModuleId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleHookError {
    Notify,
    Retry,
    Quiesce,
    Stop,
    Start,
    SelfTest,
    Heartbeat,
    Resume,
    Unmount,
    Mount,
}

/// Executes module lifecycle work that the platform, ABI host, or module slot owns.
///
/// Implementations must report success only after the requested operation has completed.
/// The runtime changes its bookkeeping state only after the corresponding hook succeeds.
pub trait ModuleLifecycleHooks {
    fn notify(&mut self, module: ModuleId) -> Result<(), ModuleHookError>;
    fn retry(&mut self, module: ModuleId) -> Result<(), ModuleHookError>;
    fn quiesce(&mut self, module: ModuleId) -> Result<(), ModuleHookError>;
    fn stop(&mut self, module: ModuleId) -> Result<(), ModuleHookError>;
    fn start(&mut self, module: ModuleId) -> Result<(), ModuleHookError>;
    fn self_test(&mut self, module: ModuleId) -> Result<(), ModuleHookError>;
    fn heartbeat(&mut self, module: ModuleId) -> Result<(), ModuleHookError>;
    fn resume(&mut self, module: ModuleId) -> Result<(), ModuleHookError>;
}

/// Extends lifecycle recovery with an executable module-slot replacement boundary.
pub trait ModuleReloadHooks: ModuleLifecycleHooks {
    fn unmount(&mut self, module: ModuleId) -> Result<(), ModuleHookError>;
    fn mount(&mut self, module: ModuleId, revision: u32) -> Result<(), ModuleHookError>;
}
