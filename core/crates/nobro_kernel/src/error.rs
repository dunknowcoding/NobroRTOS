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
