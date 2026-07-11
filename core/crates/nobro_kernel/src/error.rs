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
    WatchdogExpired,
    ModuleCrash,
    ProtocolAuthFail,
    QuotaBreach,
    PoolCorruption,
    StorageFail,
    PowerTransitionFail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultSource {
    Kernel,
    Scheduler,
    Watchdog,
    Module,
    Bus,
    Protocol,
    Storage,
    Memory,
    Power,
    Foreign,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultContext {
    pub source: FaultSource,
    pub code: u16,
    pub detail0: u32,
    pub detail1: u32,
}

impl FaultContext {
    pub const fn new(source: FaultSource, code: u16, detail0: u32, detail1: u32) -> Self {
        Self {
            source,
            code,
            detail0,
            detail1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HealthFault {
    pub error: KernelError,
    pub context: FaultContext,
}

impl HealthFault {
    pub const fn new(error: KernelError, context: FaultContext) -> Self {
        Self { error, context }
    }

    pub const fn from_error(error: KernelError) -> Self {
        let source = match error {
            KernelError::BusTimeout => FaultSource::Bus,
            KernelError::RadioTxFail | KernelError::ProtocolAuthFail => FaultSource::Protocol,
            KernelError::DeadlineMissed => FaultSource::Scheduler,
            KernelError::WatchdogExpired => FaultSource::Watchdog,
            KernelError::StackViolation
            | KernelError::MemoryFault
            | KernelError::PoolCorruption => FaultSource::Memory,
            KernelError::StorageFail => FaultSource::Storage,
            KernelError::PowerTransitionFail => FaultSource::Power,
            KernelError::ForeignModuleInitFail | KernelError::ForeignModulePollFail => {
                FaultSource::Foreign
            }
            KernelError::SensorReadFail | KernelError::ModuleCrash => FaultSource::Module,
            KernelError::LeaseConflict | KernelError::QuotaBreach => FaultSource::Kernel,
        };
        Self::new(error, FaultContext::new(source, 0, 0, 0))
    }
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
