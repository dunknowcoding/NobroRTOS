//! Module error routing (Phase 0 definitions).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelError {
    LeaseConflict,
    BusTimeout,
    RadioTxFail,
    SensorReadFail,
    DeadlineMissed,
    ForeignModuleInitFail,
    ForeignModulePollFail,
    /// A stack guard's canary was destroyed or its watermark crossed the
    /// configured limit - attributed to the owning execution context (MEM-01).
    StackViolation,
    /// A memory-protection (MPU) fault was captured and attributed (MEM-02).
    MemoryFault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    RetryNow,
    RetryDelay(u32),
    NotifyUserTask,
    RebootModule,
    Ignore,
}

pub type ErrorHandler = fn(&KernelError) -> Action;
