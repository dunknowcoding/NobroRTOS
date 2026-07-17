//! Allocation-free admission shared by firmware build scripts and the kernel.
//!
//! The input is deliberately data-only. Build scripts can run the same bounded
//! fixed-priority response-time analysis as target-side dynamic admission, then
//! emit only [`AdmittedWorkload`] into firmware read-only data.

#![no_std]

/// Report status used when a subsystem was intentionally not linked.
pub const SUBSYSTEM_ABSENT: u16 = 0xFFFF;
pub const ADMITTED_SCHEMA_VERSION: u16 = 2;
/// Largest interval that remains unambiguous under 32-bit wrapping time
/// comparisons used by the allocation-free target dispatcher.
pub const MAX_WRAP_SAFE_INTERVAL_US: u32 = 0x7FFF_FFFF;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmissionProfile {
    pub flash_limit_bytes: u32,
    pub ram_limit_bytes: u32,
    pub pool_slot_limit: u16,
    pub max_tasks: u16,
    /// Measured compare-wake-to-dispatch bound, charged once per response.
    pub wake_latency_us: u32,
}

impl AdmissionProfile {
    pub const fn new(
        flash_limit_bytes: u32,
        ram_limit_bytes: u32,
        pool_slot_limit: u16,
        max_tasks: u16,
    ) -> Self {
        Self {
            flash_limit_bytes,
            ram_limit_bytes,
            pool_slot_limit,
            max_tasks,
            wake_latency_us: 0,
        }
    }

    pub const fn wake_latency_us(mut self, wake_latency_us: u32) -> Self {
        self.wake_latency_us = wake_latency_us;
        self
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskContract {
    active: bool,
    pub id: u16,
    /// Lower values have higher fixed priority.
    pub priority_key: u16,
    /// First release offset from the executor epoch. Must be below `period_us`.
    pub phase_us: u32,
    /// Zero marks a capacity-only entry that does not participate in RTA.
    pub period_us: u32,
    /// Zero means the entry has no deadline contract.
    pub deadline_us: u32,
    pub jitter_us: u32,
    pub execution_us: u32,
    pub blocking_us: u32,
    pub flash_bytes: u32,
    pub ram_bytes: u32,
    pub pool_slots: u16,
    pub capability_bits: u32,
    /// Packed runtime object limits: mailbox, alarm, and KV counts in the
    /// lowest three bytes. Prefer [`TaskContract::object_quotas`] to packing.
    pub quota_bits: u32,
}

impl TaskContract {
    /// Unused capacity in a fixed array. It cannot be constructed accidentally
    /// through the public task builder.
    pub const EMPTY: Self = Self {
        active: false,
        id: 0,
        priority_key: u16::MAX,
        phase_us: 0,
        period_us: 0,
        deadline_us: 0,
        jitter_us: 0,
        execution_us: 0,
        blocking_us: 0,
        flash_bytes: 0,
        ram_bytes: 0,
        pool_slots: 0,
        capability_bits: 0,
        quota_bits: 0,
    };

    pub const fn new(id: u16) -> Self {
        Self {
            active: true,
            id,
            priority_key: u16::MAX,
            phase_us: 0,
            period_us: 0,
            deadline_us: 0,
            jitter_us: 0,
            execution_us: 0,
            blocking_us: 0,
            flash_bytes: 0,
            ram_bytes: 0,
            pool_slots: 0,
            capability_bits: 0,
            quota_bits: 0,
        }
    }

    pub const fn deadline(
        mut self,
        period_us: u32,
        deadline_us: u32,
        jitter_us: u32,
        execution_us: u32,
        blocking_us: u32,
    ) -> Self {
        self.period_us = period_us;
        self.deadline_us = deadline_us;
        self.jitter_us = jitter_us;
        self.execution_us = execution_us;
        self.blocking_us = blocking_us;
        self
    }

    pub const fn priority(mut self, priority_key: u16) -> Self {
        self.priority_key = priority_key;
        self
    }

    /// Offset the first periodic release without changing its relative deadline.
    /// Response-time analysis remains conservatively valid because it does not
    /// subtract interference merely because phases differ.
    pub const fn phase(mut self, phase_us: u32) -> Self {
        self.phase_us = phase_us;
        self
    }

    pub const fn memory(mut self, flash_bytes: u32, ram_bytes: u32, pool_slots: u16) -> Self {
        self.flash_bytes = flash_bytes;
        self.ram_bytes = ram_bytes;
        self.pool_slots = pool_slots;
        self
    }

    pub const fn bindings(mut self, capability_bits: u32, quota_bits: u32) -> Self {
        self.capability_bits = capability_bits;
        self.quota_bits = quota_bits;
        self
    }

    /// Select admitted runtime capabilities without changing object quotas.
    pub const fn capabilities(mut self, capability_bits: u32) -> Self {
        self.capability_bits = capability_bits;
        self
    }

    /// Select mailbox, alarm, and key-value entry limits without bit packing.
    pub const fn object_quotas(mut self, mailbox_slots: u8, alarms: u8, kv_entries: u8) -> Self {
        self.quota_bits =
            (mailbox_slots as u32) | ((alarms as u32) << 8) | ((kv_entries as u32) << 16);
        self
    }

    const fn participates(self) -> bool {
        self.deadline_us != 0
    }

    const fn active(self) -> bool {
        self.active
    }
}

/// Operations permitted in an interrupt-domain step. Arbitrary callbacks,
/// allocation, locks, waits, and peripheral polling are intentionally absent.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IsrOperations(u16);

impl IsrOperations {
    pub const ACK_PERIPHERAL: Self = Self(1 << 0);
    pub const READ_CLOCK: Self = Self(1 << 1);
    pub const MARK_READY: Self = Self(1 << 2);
    pub const PUSH_BOUNDED_EVENT: Self = Self(1 << 3);
    pub const ALL_BOUNDED: Self = Self(
        Self::ACK_PERIPHERAL.0
            | Self::READ_CLOCK.0
            | Self::MARK_READY.0
            | Self::PUSH_BOUNDED_EVENT.0,
    );

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }
}

/// One deadline-critical interrupt domain. Lower logical priority values are
/// higher urgency, matching NVIC convention before hardware bit shifting.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterruptContract {
    active: bool,
    pub id: u16,
    pub priority: u8,
    pub period_us: u32,
    pub deadline_us: u32,
    pub execution_us: u32,
    /// Basic/extended exception-frame and handler-owned stack bound.
    pub stack_bytes: u16,
    pub operations: IsrOperations,
}

impl InterruptContract {
    pub const EMPTY: Self = Self {
        active: false,
        id: 0,
        priority: u8::MAX,
        period_us: 0,
        deadline_us: 0,
        execution_us: 0,
        stack_bytes: 0,
        operations: IsrOperations::empty(),
    };

    pub const fn new(
        id: u16,
        priority: u8,
        period_us: u32,
        deadline_us: u32,
        execution_us: u32,
        stack_bytes: u16,
    ) -> Self {
        Self {
            active: true,
            id,
            priority,
            period_us,
            deadline_us,
            execution_us,
            stack_bytes,
            operations: IsrOperations::empty(),
        }
    }

    pub const fn operations(mut self, operations: IsrOperations) -> Self {
        self.operations = operations;
        self
    }

    const fn active(self) -> bool {
        self.active
    }
}

/// Target-specific interrupt constraints supplied to shared admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterruptProfile {
    pub priority_levels: u8,
    /// Logical priority bits reserved by firmware/radio/boot stacks.
    pub reserved_priorities: u8,
    pub max_nesting: u8,
    pub interrupt_stack_limit_bytes: u16,
}

impl InterruptProfile {
    pub const fn new(
        priority_levels: u8,
        reserved_priorities: u8,
        max_nesting: u8,
        interrupt_stack_limit_bytes: u16,
    ) -> Self {
        Self {
            priority_levels,
            reserved_priorities,
            max_nesting,
            interrupt_stack_limit_bytes,
        }
    }

    /// nRF52840 without a SoftDevice: all eight logical levels are available.
    pub const NRF52840_BARE: Self = Self::new(8, 0, 3, 1_024);
    /// S140 reserves logical priorities 0, 1, 4, and 5; application IRQs use
    /// 2, 3, 6, or 7. The stack bound remains an explicit application budget.
    pub const NRF52840_S140: Self =
        Self::new(8, (1 << 0) | (1 << 1) | (1 << 4) | (1 << 5), 3, 1_024);
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionErrorCode {
    EmptyWorkload = 1,
    TooManyTasks = 2,
    DuplicateId = 3,
    InvalidDeadline = 4,
    InvalidJitter = 5,
    InvalidExecution = 6,
    InvalidBlocking = 7,
    UtilizationExceeded = 8,
    ResponseTimeExceeded = 9,
    FlashExceeded = 10,
    RamExceeded = 11,
    PoolExceeded = 12,
    ArithmeticOverflow = 13,
    WakeLatencyExceeded = 14,
    InvalidPhase = 15,
    InvalidInterruptPriority = 16,
    ReservedInterruptPriority = 17,
    InvalidInterruptContract = 18,
    UnsafeInterruptOperation = 19,
    InterruptStackExceeded = 20,
    InterruptResponseExceeded = 21,
}

impl AdmissionErrorCode {
    pub const fn diagnostic(self) -> &'static str {
        match self {
            Self::EmptyWorkload => "NOBRO-E001 empty workload",
            Self::TooManyTasks => "NOBRO-E002 task capacity exceeded",
            Self::DuplicateId => "NOBRO-E003 duplicate task identity",
            Self::InvalidDeadline => "NOBRO-E004 invalid deadline/period",
            Self::InvalidJitter => "NOBRO-E005 jitter must be below deadline",
            Self::InvalidExecution => "NOBRO-E006 execution bound is missing or too large",
            Self::InvalidBlocking => "NOBRO-E007 execution plus blocking exceeds deadline",
            Self::UtilizationExceeded => "NOBRO-E008 utilization exceeds one core",
            Self::ResponseTimeExceeded => "NOBRO-E009 response-time analysis missed deadline",
            Self::FlashExceeded => "NOBRO-E010 flash profile exceeded",
            Self::RamExceeded => "NOBRO-E011 RAM profile exceeded",
            Self::PoolExceeded => "NOBRO-E012 sample-pool profile exceeded",
            Self::ArithmeticOverflow => "NOBRO-E013 admission arithmetic overflow",
            Self::WakeLatencyExceeded => "NOBRO-E014 wake-latency bound exceeds deadline",
            Self::InvalidPhase => "NOBRO-E015 phase must be below period",
            Self::InvalidInterruptPriority => {
                "NOBRO-E016 interrupt priority is outside target range"
            }
            Self::ReservedInterruptPriority => {
                "NOBRO-E017 interrupt priority is reserved by the platform stack"
            }
            Self::InvalidInterruptContract => "NOBRO-E018 invalid interrupt timing/stack contract",
            Self::UnsafeInterruptOperation => {
                "NOBRO-E019 interrupt step requests an unbounded operation"
            }
            Self::InterruptStackExceeded => "NOBRO-E020 nested interrupt-stack budget exceeded",
            Self::InterruptResponseExceeded => "NOBRO-E021 interrupt interference exceeds deadline",
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmissionError {
    pub code: AdmissionErrorCode,
    /// Input index; `u16::MAX` identifies a workload-wide error.
    pub task_index: u16,
    pub observed: u64,
    pub limit: u64,
}

impl AdmissionError {
    const fn task(code: AdmissionErrorCode, task_index: usize, observed: u64, limit: u64) -> Self {
        Self {
            code,
            task_index: task_index as u16,
            observed,
            limit,
        }
    }

    const fn global(code: AdmissionErrorCode, observed: u64, limit: u64) -> Self {
        Self {
            code,
            task_index: u16::MAX,
            observed,
            limit,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmittedTask {
    pub id: u16,
    /// Zero is the highest fixed priority. Capacity-only entries use `u16::MAX`.
    pub priority: u16,
    pub phase_us: u32,
    pub period_us: u32,
    pub deadline_us: u32,
    pub response_bound_us: u32,
    pub capability_bits: u32,
    pub quota_bits: u32,
}

impl AdmittedTask {
    pub const EMPTY: Self = Self {
        id: 0,
        priority: u16::MAX,
        phase_us: 0,
        period_us: 0,
        deadline_us: 0,
        response_bound_us: 0,
        capability_bits: 0,
        quota_bits: 0,
    };
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmittedWorkload<const N: usize> {
    pub schema_version: u16,
    pub task_count: u16,
    pub tasks: [AdmittedTask; N],
    pub flash_bytes: u32,
    pub ram_bytes: u32,
    pub pool_slots: u16,
    pub utilization_permyriad: u16,
}

const fn checked_add_u32(left: u32, right: u32) -> Option<u32> {
    if left > u32::MAX - right {
        None
    } else {
        Some(left + right)
    }
}

const fn higher_priority(left: usize, right: usize, tasks: &[TaskContract]) -> bool {
    let a = tasks[left];
    let b = tasks[right];
    a.participates()
        && (!b.participates()
            || a.priority_key < b.priority_key
            || (a.priority_key == b.priority_key
                && (a.period_us < b.period_us || (a.period_us == b.period_us && left < right))))
}

const fn priority_of(index: usize, tasks: &[TaskContract]) -> u16 {
    let mut priority = 0u16;
    let mut other = 0usize;
    if tasks[index].participates() {
        while other < tasks.len() {
            if higher_priority(other, index, tasks) {
                priority += 1;
            }
            other += 1;
        }
    } else {
        while other < tasks.len() {
            if tasks[other].active()
                && (tasks[other].participates() || (!tasks[other].participates() && other < index))
            {
                priority += 1;
            }
            other += 1;
        }
    }
    priority
}

const fn response_time(
    index: usize,
    tasks: &[TaskContract],
    wake_latency_us: u32,
) -> Result<u32, AdmissionError> {
    let task = tasks[index];
    let execution_and_blocking = match checked_add_u32(task.execution_us, task.blocking_us) {
        Some(value) => value,
        None => {
            return Err(AdmissionError::task(
                AdmissionErrorCode::ArithmeticOverflow,
                index,
                u64::MAX,
                u32::MAX as u64,
            ))
        }
    };
    let mut response = match checked_add_u32(execution_and_blocking, wake_latency_us) {
        Some(value) => value,
        None => {
            return Err(AdmissionError::task(
                AdmissionErrorCode::ArithmeticOverflow,
                index,
                u64::MAX,
                u32::MAX as u64,
            ))
        }
    };
    let mut iteration = 0usize;
    while iteration < 64 {
        let mut next = match checked_add_u32(execution_and_blocking, wake_latency_us) {
            Some(value) => value,
            None => {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::ArithmeticOverflow,
                    index,
                    u64::MAX,
                    u32::MAX as u64,
                ))
            }
        };
        let mut other = 0usize;
        while other < tasks.len() {
            if higher_priority(other, index, tasks) {
                let hp = tasks[other];
                let releases =
                    (response as u64 + hp.jitter_us as u64).div_ceil(hp.period_us as u64);
                let interference = releases * hp.execution_us as u64;
                if interference > u32::MAX as u64 || next as u64 + interference > u32::MAX as u64 {
                    return Err(AdmissionError::task(
                        AdmissionErrorCode::ArithmeticOverflow,
                        index,
                        next as u64 + interference,
                        u32::MAX as u64,
                    ));
                }
                next += interference as u32;
            }
            other += 1;
        }
        if next == response {
            return Ok(next);
        }
        if next as u64 + task.jitter_us as u64 > task.deadline_us as u64 {
            return Err(AdmissionError::task(
                AdmissionErrorCode::ResponseTimeExceeded,
                index,
                next as u64 + task.jitter_us as u64,
                task.deadline_us as u64,
            ));
        }
        response = next;
        iteration += 1;
    }
    Err(AdmissionError::task(
        AdmissionErrorCode::ResponseTimeExceeded,
        index,
        response as u64 + task.jitter_us as u64,
        task.deadline_us as u64,
    ))
}

/// Admit a complete workload. This is `const`, so a generated project may make
/// rejection a compile-time error; build scripts use the same function to add
/// the task label to the diagnostic.
pub const fn admit<const N: usize>(
    tasks: [TaskContract; N],
    profile: AdmissionProfile,
) -> Result<AdmittedWorkload<N>, AdmissionError> {
    let mut flash = 0u32;
    let mut ram = 0u32;
    let mut pool = 0u16;
    let mut utilization = 0u64;
    let mut active_count = 0usize;
    let mut index = 0usize;
    while index < N {
        let task = tasks[index];
        if !task.active() {
            index += 1;
            continue;
        }
        active_count += 1;
        let mut previous = 0usize;
        while previous < index {
            if tasks[previous].active() && tasks[previous].id == task.id {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::DuplicateId,
                    index,
                    task.id as u64,
                    0,
                ));
            }
            previous += 1;
        }
        flash = match checked_add_u32(flash, task.flash_bytes) {
            Some(value) => value,
            None => {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::ArithmeticOverflow,
                    index,
                    u64::MAX,
                    u32::MAX as u64,
                ))
            }
        };
        ram = match checked_add_u32(ram, task.ram_bytes) {
            Some(value) => value,
            None => {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::ArithmeticOverflow,
                    index,
                    u64::MAX,
                    u32::MAX as u64,
                ))
            }
        };
        pool = match pool.checked_add(task.pool_slots) {
            Some(value) => value,
            None => {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::ArithmeticOverflow,
                    index,
                    u64::MAX,
                    u16::MAX as u64,
                ))
            }
        };

        if task.participates() {
            if task.period_us == 0
                || task.period_us > MAX_WRAP_SAFE_INTERVAL_US
                || task.deadline_us > task.period_us
            {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::InvalidDeadline,
                    index,
                    task.deadline_us as u64,
                    task.period_us as u64,
                ));
            }
            if task.phase_us >= task.period_us {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::InvalidPhase,
                    index,
                    task.phase_us as u64,
                    task.period_us.saturating_sub(1) as u64,
                ));
            }
            if task.jitter_us >= task.deadline_us {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::InvalidJitter,
                    index,
                    task.jitter_us as u64,
                    task.deadline_us.saturating_sub(1) as u64,
                ));
            }
            if task.execution_us == 0 || task.execution_us > task.deadline_us {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::InvalidExecution,
                    index,
                    task.execution_us as u64,
                    task.deadline_us as u64,
                ));
            }
            let execution_and_blocking = task.execution_us as u64 + task.blocking_us as u64;
            if execution_and_blocking > task.deadline_us as u64 {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::InvalidBlocking,
                    index,
                    execution_and_blocking,
                    task.deadline_us as u64,
                ));
            }
            let response_floor = execution_and_blocking + profile.wake_latency_us as u64;
            if response_floor > task.deadline_us as u64 {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::WakeLatencyExceeded,
                    index,
                    response_floor,
                    task.deadline_us as u64,
                ));
            }
            utilization += (task.execution_us as u64 * 10_000).div_ceil(task.period_us as u64);
            if utilization > 10_000 {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::UtilizationExceeded,
                    index,
                    utilization,
                    10_000,
                ));
            }
        } else if task.phase_us != 0
            || task.period_us != 0
            || task.jitter_us != 0
            || task.execution_us != 0
            || task.blocking_us != 0
        {
            return Err(AdmissionError::task(
                AdmissionErrorCode::InvalidDeadline,
                index,
                task.deadline_us as u64,
                0,
            ));
        }
        index += 1;
    }

    if active_count == 0 {
        return Err(AdmissionError::global(
            AdmissionErrorCode::EmptyWorkload,
            0,
            1,
        ));
    }
    if active_count > profile.max_tasks as usize {
        return Err(AdmissionError::global(
            AdmissionErrorCode::TooManyTasks,
            active_count as u64,
            profile.max_tasks as u64,
        ));
    }

    if flash > profile.flash_limit_bytes {
        return Err(AdmissionError::global(
            AdmissionErrorCode::FlashExceeded,
            flash as u64,
            profile.flash_limit_bytes as u64,
        ));
    }
    if ram > profile.ram_limit_bytes {
        return Err(AdmissionError::global(
            AdmissionErrorCode::RamExceeded,
            ram as u64,
            profile.ram_limit_bytes as u64,
        ));
    }
    if pool > profile.pool_slot_limit {
        return Err(AdmissionError::global(
            AdmissionErrorCode::PoolExceeded,
            pool as u64,
            profile.pool_slot_limit as u64,
        ));
    }

    let mut admitted = [AdmittedTask::EMPTY; N];
    index = 0;
    while index < N {
        let task = tasks[index];
        if !task.active() {
            index += 1;
            continue;
        }
        let response = if task.participates() {
            match response_time(index, &tasks, profile.wake_latency_us) {
                Ok(value) => value,
                Err(error) => return Err(error),
            }
        } else {
            0
        };
        if task.participates() && response as u64 + task.jitter_us as u64 > task.deadline_us as u64
        {
            return Err(AdmissionError::task(
                AdmissionErrorCode::ResponseTimeExceeded,
                index,
                response as u64 + task.jitter_us as u64,
                task.deadline_us as u64,
            ));
        }
        admitted[index] = AdmittedTask {
            id: task.id,
            priority: priority_of(index, &tasks),
            phase_us: task.phase_us,
            period_us: task.period_us,
            deadline_us: task.deadline_us,
            response_bound_us: response,
            capability_bits: task.capability_bits,
            quota_bits: task.quota_bits,
        };
        index += 1;
    }

    Ok(AdmittedWorkload {
        schema_version: ADMITTED_SCHEMA_VERSION,
        task_count: active_count as u16,
        tasks: admitted,
        flash_bytes: flash,
        ram_bytes: ram,
        pool_slots: pool,
        utilization_permyriad: utilization as u16,
    })
}

#[inline(always)]
fn runtime_higher_priority(
    left: usize,
    right: usize,
    task_at: &impl Fn(usize) -> TaskContract,
) -> bool {
    let a = task_at(left);
    let b = task_at(right);
    a.participates()
        && (!b.participates()
            || a.priority_key < b.priority_key
            || (a.priority_key == b.priority_key
                && (a.period_us < b.period_us || (a.period_us == b.period_us && left < right))))
}

fn runtime_response_time(
    index: usize,
    task_count: usize,
    wake_latency_us: u32,
    task_at: &impl Fn(usize) -> TaskContract,
) -> Result<u32, AdmissionError> {
    let task = task_at(index);
    let execution_and_blocking =
        checked_add_u32(task.execution_us, task.blocking_us).ok_or(AdmissionError::task(
            AdmissionErrorCode::ArithmeticOverflow,
            index,
            u64::MAX,
            u32::MAX as u64,
        ))?;
    let mut response =
        checked_add_u32(execution_and_blocking, wake_latency_us).ok_or(AdmissionError::task(
            AdmissionErrorCode::ArithmeticOverflow,
            index,
            u64::MAX,
            u32::MAX as u64,
        ))?;
    let mut iteration = 0usize;
    while iteration < 64 {
        let mut next = checked_add_u32(execution_and_blocking, wake_latency_us).ok_or(
            AdmissionError::task(
                AdmissionErrorCode::ArithmeticOverflow,
                index,
                u64::MAX,
                u32::MAX as u64,
            ),
        )?;
        let mut other = 0usize;
        while other < task_count {
            if runtime_higher_priority(other, index, task_at) {
                let hp = task_at(other);
                let releases =
                    (response as u64 + hp.jitter_us as u64).div_ceil(hp.period_us as u64);
                let interference = releases * hp.execution_us as u64;
                if interference > u32::MAX as u64 || next as u64 + interference > u32::MAX as u64 {
                    return Err(AdmissionError::task(
                        AdmissionErrorCode::ArithmeticOverflow,
                        index,
                        next as u64 + interference,
                        u32::MAX as u64,
                    ));
                }
                next += interference as u32;
            }
            other += 1;
        }
        if next == response {
            return Ok(next);
        }
        if next as u64 + task.jitter_us as u64 > task.deadline_us as u64 {
            return Err(AdmissionError::task(
                AdmissionErrorCode::ResponseTimeExceeded,
                index,
                next as u64 + task.jitter_us as u64,
                task.deadline_us as u64,
            ));
        }
        response = next;
        iteration += 1;
    }
    Err(AdmissionError::task(
        AdmissionErrorCode::ResponseTimeExceeded,
        index,
        response as u64 + task.jitter_us as u64,
        task.deadline_us as u64,
    ))
}

/// Validate a runtime-provided workload without first copying it into a
/// capacity-sized [`TaskContract`] array.
///
/// This is the dynamic counterpart to [`admit`]: it applies the same ordering
/// of identity, resource, contract, utilization, and response-time checks but
/// returns no admitted table. The callback may derive each contract directly
/// from its owning runtime representation. It must return the same contract
/// for an index throughout this call.
pub fn validate_runtime(
    task_count: usize,
    profile: AdmissionProfile,
    task_at: impl Fn(usize) -> TaskContract,
) -> Result<(), AdmissionError> {
    let mut flash = 0u32;
    let mut ram = 0u32;
    let mut pool = 0u16;
    let mut utilization = 0u64;
    let mut active_count = 0usize;
    let mut index = 0usize;
    while index < task_count {
        let task = task_at(index);
        if !task.active() {
            index += 1;
            continue;
        }
        active_count += 1;
        let mut previous = 0usize;
        while previous < index {
            let prior = task_at(previous);
            if prior.active() && prior.id == task.id {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::DuplicateId,
                    index,
                    task.id as u64,
                    0,
                ));
            }
            previous += 1;
        }
        flash = checked_add_u32(flash, task.flash_bytes).ok_or(AdmissionError::task(
            AdmissionErrorCode::ArithmeticOverflow,
            index,
            u64::MAX,
            u32::MAX as u64,
        ))?;
        ram = checked_add_u32(ram, task.ram_bytes).ok_or(AdmissionError::task(
            AdmissionErrorCode::ArithmeticOverflow,
            index,
            u64::MAX,
            u32::MAX as u64,
        ))?;
        pool = pool
            .checked_add(task.pool_slots)
            .ok_or(AdmissionError::task(
                AdmissionErrorCode::ArithmeticOverflow,
                index,
                u64::MAX,
                u16::MAX as u64,
            ))?;

        if task.participates() {
            if task.period_us == 0
                || task.period_us > MAX_WRAP_SAFE_INTERVAL_US
                || task.deadline_us > task.period_us
            {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::InvalidDeadline,
                    index,
                    task.deadline_us as u64,
                    task.period_us as u64,
                ));
            }
            if task.phase_us >= task.period_us {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::InvalidPhase,
                    index,
                    task.phase_us as u64,
                    task.period_us.saturating_sub(1) as u64,
                ));
            }
            if task.jitter_us >= task.deadline_us {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::InvalidJitter,
                    index,
                    task.jitter_us as u64,
                    task.deadline_us.saturating_sub(1) as u64,
                ));
            }
            if task.execution_us == 0 || task.execution_us > task.deadline_us {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::InvalidExecution,
                    index,
                    task.execution_us as u64,
                    task.deadline_us as u64,
                ));
            }
            let execution_and_blocking = task.execution_us as u64 + task.blocking_us as u64;
            if execution_and_blocking > task.deadline_us as u64 {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::InvalidBlocking,
                    index,
                    execution_and_blocking,
                    task.deadline_us as u64,
                ));
            }
            let response_floor = execution_and_blocking + profile.wake_latency_us as u64;
            if response_floor > task.deadline_us as u64 {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::WakeLatencyExceeded,
                    index,
                    response_floor,
                    task.deadline_us as u64,
                ));
            }
            utilization += (task.execution_us as u64 * 10_000).div_ceil(task.period_us as u64);
            if utilization > 10_000 {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::UtilizationExceeded,
                    index,
                    utilization,
                    10_000,
                ));
            }
        } else if task.phase_us != 0
            || task.period_us != 0
            || task.jitter_us != 0
            || task.execution_us != 0
            || task.blocking_us != 0
        {
            return Err(AdmissionError::task(
                AdmissionErrorCode::InvalidDeadline,
                index,
                task.deadline_us as u64,
                0,
            ));
        }
        index += 1;
    }

    if active_count == 0 {
        return Err(AdmissionError::global(
            AdmissionErrorCode::EmptyWorkload,
            0,
            1,
        ));
    }
    if active_count > profile.max_tasks as usize {
        return Err(AdmissionError::global(
            AdmissionErrorCode::TooManyTasks,
            active_count as u64,
            profile.max_tasks as u64,
        ));
    }
    if flash > profile.flash_limit_bytes {
        return Err(AdmissionError::global(
            AdmissionErrorCode::FlashExceeded,
            flash as u64,
            profile.flash_limit_bytes as u64,
        ));
    }
    if ram > profile.ram_limit_bytes {
        return Err(AdmissionError::global(
            AdmissionErrorCode::RamExceeded,
            ram as u64,
            profile.ram_limit_bytes as u64,
        ));
    }
    if pool > profile.pool_slot_limit {
        return Err(AdmissionError::global(
            AdmissionErrorCode::PoolExceeded,
            pool as u64,
            profile.pool_slot_limit as u64,
        ));
    }

    index = 0;
    while index < task_count {
        let task = task_at(index);
        if task.active() && task.participates() {
            let response =
                runtime_response_time(index, task_count, profile.wake_latency_us, &task_at)?;
            if response as u64 + task.jitter_us as u64 > task.deadline_us as u64 {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::ResponseTimeExceeded,
                    index,
                    response as u64 + task.jitter_us as u64,
                    task.deadline_us as u64,
                ));
            }
        }
        index += 1;
    }
    Ok(())
}

const fn interrupt_response<const I: usize>(
    index: usize,
    interrupts: &[InterruptContract; I],
) -> Result<u32, AdmissionError> {
    let interrupt = interrupts[index];
    let mut response = interrupt.execution_us;
    let mut iteration = 0usize;
    while iteration < 64 {
        let mut next = interrupt.execution_us as u64;
        let mut other = 0usize;
        while other < I {
            let hp = interrupts[other];
            // Equal-priority sources cannot preempt each other, but one may
            // already be pending when this source becomes ready. Without
            // vector-order metadata, charging their periodic demand as mutual
            // interference is conservative and permits useful priority groups.
            if other != index && hp.active() && hp.priority <= interrupt.priority {
                let releases = (response as u64).div_ceil(hp.period_us as u64);
                let interference = match releases.checked_mul(hp.execution_us as u64) {
                    Some(value) => value,
                    None => {
                        return Err(AdmissionError::task(
                            AdmissionErrorCode::ArithmeticOverflow,
                            index,
                            u64::MAX,
                            u32::MAX as u64,
                        ))
                    }
                };
                next = match next.checked_add(interference) {
                    Some(value) => value,
                    None => {
                        return Err(AdmissionError::task(
                            AdmissionErrorCode::ArithmeticOverflow,
                            index,
                            u64::MAX,
                            u32::MAX as u64,
                        ))
                    }
                };
            }
            other += 1;
        }
        if next > u32::MAX as u64 || next > interrupt.deadline_us as u64 {
            return Err(AdmissionError::task(
                AdmissionErrorCode::InterruptResponseExceeded,
                index,
                next,
                interrupt.deadline_us as u64,
            ));
        }
        if next as u32 == response {
            return Ok(response);
        }
        response = next as u32;
        iteration += 1;
    }
    Err(AdmissionError::task(
        AdmissionErrorCode::InterruptResponseExceeded,
        index,
        response as u64,
        interrupt.deadline_us as u64,
    ))
}

const fn task_response_with_interrupts<const T: usize, const I: usize>(
    index: usize,
    tasks: &[TaskContract; T],
    interrupts: &[InterruptContract; I],
    wake_latency_us: u32,
) -> Result<u32, AdmissionError> {
    let task = tasks[index];
    let execution_and_blocking =
        match (task.execution_us as u64).checked_add(task.blocking_us as u64) {
            Some(value) => value,
            None => {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::ArithmeticOverflow,
                    index,
                    u64::MAX,
                    u32::MAX as u64,
                ))
            }
        };
    let base = match execution_and_blocking.checked_add(wake_latency_us as u64) {
        Some(value) => value,
        None => {
            return Err(AdmissionError::task(
                AdmissionErrorCode::ArithmeticOverflow,
                index,
                u64::MAX,
                u32::MAX as u64,
            ))
        }
    };
    if base > u32::MAX as u64 {
        return Err(AdmissionError::task(
            AdmissionErrorCode::ArithmeticOverflow,
            index,
            base,
            u32::MAX as u64,
        ));
    }
    let mut response = base as u32;
    let mut iteration = 0usize;
    while iteration < 64 {
        let mut next = base;
        let mut other = 0usize;
        while other < T {
            if higher_priority(other, index, tasks) {
                let hp = tasks[other];
                let releases =
                    (response as u64 + hp.jitter_us as u64).div_ceil(hp.period_us as u64);
                let interference = match releases.checked_mul(hp.execution_us as u64) {
                    Some(value) => value,
                    None => {
                        return Err(AdmissionError::task(
                            AdmissionErrorCode::ArithmeticOverflow,
                            index,
                            u64::MAX,
                            u32::MAX as u64,
                        ))
                    }
                };
                next = match next.checked_add(interference) {
                    Some(value) => value,
                    None => {
                        return Err(AdmissionError::task(
                            AdmissionErrorCode::ArithmeticOverflow,
                            index,
                            u64::MAX,
                            u32::MAX as u64,
                        ))
                    }
                };
            }
            other += 1;
        }
        let mut irq = 0usize;
        while irq < I {
            let interrupt = interrupts[irq];
            if interrupt.active() {
                let releases = (response as u64).div_ceil(interrupt.period_us as u64);
                let interference = match releases.checked_mul(interrupt.execution_us as u64) {
                    Some(value) => value,
                    None => {
                        return Err(AdmissionError::task(
                            AdmissionErrorCode::ArithmeticOverflow,
                            index,
                            u64::MAX,
                            u32::MAX as u64,
                        ))
                    }
                };
                next = match next.checked_add(interference) {
                    Some(value) => value,
                    None => {
                        return Err(AdmissionError::task(
                            AdmissionErrorCode::ArithmeticOverflow,
                            index,
                            u64::MAX,
                            u32::MAX as u64,
                        ))
                    }
                };
            }
            irq += 1;
        }
        if next > u32::MAX as u64 || next + task.jitter_us as u64 > task.deadline_us as u64 {
            return Err(AdmissionError::task(
                AdmissionErrorCode::ResponseTimeExceeded,
                index,
                next + task.jitter_us as u64,
                task.deadline_us as u64,
            ));
        }
        if next as u32 == response {
            return Ok(response);
        }
        response = next as u32;
        iteration += 1;
    }
    Err(AdmissionError::task(
        AdmissionErrorCode::ResponseTimeExceeded,
        index,
        response as u64 + task.jitter_us as u64,
        task.deadline_us as u64,
    ))
}

/// Admit periodic tasks together with optional deadline-critical ISR domains.
/// ISR work is charged as interference to every cooperative task; each ISR is
/// also checked against higher-urgency ISR interference and a conservative
/// nested exception-stack bound. Default `admit` users pay no code/data cost.
pub const fn admit_with_interrupts<const T: usize, const I: usize>(
    tasks: [TaskContract; T],
    interrupts: [InterruptContract; I],
    profile: AdmissionProfile,
    interrupt_profile: InterruptProfile,
) -> Result<AdmittedWorkload<T>, AdmissionError> {
    let mut admitted = match admit(tasks, profile) {
        Ok(value) => value,
        Err(error) => return Err(error),
    };
    if interrupt_profile.priority_levels == 0
        || interrupt_profile.priority_levels > 8
        || interrupt_profile.max_nesting == 0
    {
        return Err(AdmissionError::global(
            AdmissionErrorCode::InvalidInterruptContract,
            interrupt_profile.priority_levels as u64,
            8,
        ));
    }

    let mut utilization = admitted.utilization_permyriad as u64;
    let mut index = 0usize;
    while index < I {
        let interrupt = interrupts[index];
        if !interrupt.active() {
            index += 1;
            continue;
        }
        if interrupt.priority >= interrupt_profile.priority_levels {
            return Err(AdmissionError::task(
                AdmissionErrorCode::InvalidInterruptPriority,
                index,
                interrupt.priority as u64,
                interrupt_profile.priority_levels.saturating_sub(1) as u64,
            ));
        }
        if interrupt_profile.reserved_priorities & (1u8 << interrupt.priority) != 0 {
            return Err(AdmissionError::task(
                AdmissionErrorCode::ReservedInterruptPriority,
                index,
                interrupt.priority as u64,
                interrupt_profile.reserved_priorities as u64,
            ));
        }
        if interrupt.period_us == 0
            || interrupt.period_us > MAX_WRAP_SAFE_INTERVAL_US
            || interrupt.deadline_us == 0
            || interrupt.deadline_us > interrupt.period_us
            || interrupt.execution_us == 0
            || interrupt.execution_us > interrupt.deadline_us
            || interrupt.stack_bytes < 32
            || interrupt.stack_bytes & 7 != 0
        {
            return Err(AdmissionError::task(
                AdmissionErrorCode::InvalidInterruptContract,
                index,
                interrupt.execution_us as u64,
                interrupt.deadline_us as u64,
            ));
        }
        if interrupt.operations.bits() & !IsrOperations::ALL_BOUNDED.bits() != 0 {
            return Err(AdmissionError::task(
                AdmissionErrorCode::UnsafeInterruptOperation,
                index,
                interrupt.operations.bits() as u64,
                IsrOperations::ALL_BOUNDED.bits() as u64,
            ));
        }
        let mut previous = 0usize;
        while previous < index {
            if interrupts[previous].active() && interrupts[previous].id == interrupt.id {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::InvalidInterruptContract,
                    index,
                    interrupt.id as u64,
                    0,
                ));
            }
            previous += 1;
        }
        let irq_utilization =
            (interrupt.execution_us as u64 * 10_000).div_ceil(interrupt.period_us as u64);
        utilization = match utilization.checked_add(irq_utilization) {
            Some(value) => value,
            None => {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::ArithmeticOverflow,
                    index,
                    u64::MAX,
                    10_000,
                ))
            }
        };
        if utilization > 10_000 {
            return Err(AdmissionError::task(
                AdmissionErrorCode::UtilizationExceeded,
                index,
                utilization,
                10_000,
            ));
        }
        match interrupt_response(index, &interrupts) {
            Ok(_) => {}
            Err(error) => return Err(error),
        }
        index += 1;
    }

    // Equal-priority NVIC handlers cannot nest. Select the largest frame at
    // each distinct priority, then sum the largest admitted nesting depth.
    let mut priorities_present = [false; 8];
    let mut priority_count = 0usize;
    index = 0;
    while index < I {
        if interrupts[index].active() {
            let priority = interrupts[index].priority as usize;
            if !priorities_present[priority] {
                priorities_present[priority] = true;
                priority_count += 1;
            }
        }
        index += 1;
    }
    // Every distinct NVIC priority may be live in one preemption chain. A
    // profile's nesting limit is an admitted deployment bound, not permission
    // to ignore the remaining frames. Reject an unrealizable profile instead
    // of truncating the stack calculation and understating MSP demand.
    if priority_count > interrupt_profile.max_nesting as usize {
        return Err(AdmissionError::global(
            AdmissionErrorCode::InvalidInterruptContract,
            priority_count as u64,
            interrupt_profile.max_nesting as u64,
        ));
    }
    let mut chosen_priority = [false; 8];
    let mut stack = 0u32;
    let mut depth = 0usize;
    while depth < priority_count {
        let mut largest = 0u16;
        let mut largest_index = I;
        index = 0;
        while index < I {
            if interrupts[index].active()
                && !chosen_priority[interrupts[index].priority as usize]
                && interrupts[index].stack_bytes > largest
            {
                largest = interrupts[index].stack_bytes;
                largest_index = index;
            }
            index += 1;
        }
        if largest_index < I {
            chosen_priority[interrupts[largest_index].priority as usize] = true;
            stack = match stack.checked_add(largest as u32) {
                Some(value) => value,
                None => {
                    return Err(AdmissionError::global(
                        AdmissionErrorCode::ArithmeticOverflow,
                        u64::MAX,
                        u32::MAX as u64,
                    ))
                }
            };
        }
        depth += 1;
    }
    if stack > interrupt_profile.interrupt_stack_limit_bytes as u32 {
        return Err(AdmissionError::global(
            AdmissionErrorCode::InterruptStackExceeded,
            stack as u64,
            interrupt_profile.interrupt_stack_limit_bytes as u64,
        ));
    }

    index = 0;
    while index < T {
        if tasks[index].active() && tasks[index].participates() {
            let response = match task_response_with_interrupts(
                index,
                &tasks,
                &interrupts,
                profile.wake_latency_us,
            ) {
                Ok(value) => value,
                Err(error) => return Err(error),
            };
            admitted.tasks[index].response_bound_us = response;
        }
        index += 1;
    }
    admitted.utilization_permyriad = utilization as u16;
    Ok(admitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: AdmissionProfile = AdmissionProfile::new(64 * 1024, 16 * 1024, 8, 8);
    const GOOD: [TaskContract; 3] = [
        TaskContract::new(0).memory(2048, 512, 1),
        TaskContract::new(1)
            .deadline(5_000, 5_000, 20, 400, 40)
            .memory(1024, 256, 1)
            .bindings(1, 0x0101),
        TaskContract::new(2)
            .deadline(10_000, 10_000, 50, 800, 100)
            .memory(1024, 256, 1),
    ];
    const ADMITTED: AdmittedWorkload<3> = match admit(GOOD, PROFILE) {
        Ok(value) => value,
        Err(_) => panic!("const workload should admit"),
    };

    #[test]
    fn const_and_runtime_admission_share_the_result() {
        assert_eq!(admit(GOOD, PROFILE), Ok(ADMITTED));
        assert_eq!(ADMITTED.tasks[1].priority, 0);
        assert_eq!(ADMITTED.tasks[2].priority, 1);
        assert!(ADMITTED.tasks[2].response_bound_us >= 1_300);
        assert_eq!(ADMITTED.flash_bytes, 4096);
    }

    #[test]
    fn named_binding_builders_match_the_packed_compatibility_form() {
        let packed = TaskContract::new(1).bindings(0xA5, 3 | (5 << 8) | (7 << 16));
        let named = TaskContract::new(1)
            .capabilities(0xA5)
            .object_quotas(3, 5, 7);
        assert_eq!(named, packed);
    }

    fn runtime_validation<const N: usize>(
        tasks: [TaskContract; N],
        profile: AdmissionProfile,
    ) -> Result<(), AdmissionError> {
        validate_runtime(N, profile, |index| tasks[index])
    }

    #[test]
    fn streamed_runtime_validation_matches_const_admission() {
        let mut cases = [
            GOOD,
            [TaskContract::EMPTY; 3],
            [
                TaskContract::new(1),
                TaskContract::new(1),
                TaskContract::EMPTY,
            ],
            [
                TaskContract::new(0).memory(u32::MAX, 0, 0),
                TaskContract::new(1).memory(1, 0, 0),
                TaskContract::EMPTY,
            ],
            [
                TaskContract::new(0).deadline(5_000, 5_000, 100, 2_000, 0),
                TaskContract::new(1).deadline(7_000, 3_000, 100, 1_500, 0),
                TaskContract::EMPTY,
            ],
        ];
        for tasks in cases {
            assert_eq!(
                runtime_validation(tasks, PROFILE),
                admit(tasks, PROFILE).map(|_| ())
            );
        }

        let periods = [0, 10, 1_000];
        let deadlines = [0, 9, 1_000];
        let executions = [0, 1, 900];
        let blockings = [0, 2, 200];
        let jitters = [0, 5, 999];
        for period in periods {
            for deadline in deadlines {
                for execution in executions {
                    for blocking in blockings {
                        for jitter in jitters {
                            let task = TaskContract::new(7)
                                .deadline(period, deadline, jitter, execution, blocking);
                            cases[0] = [
                                TaskContract::new(0).memory(1, 1, 0),
                                task,
                                TaskContract::new(8)
                                    .deadline(2_000, 1_500, 3, 300, 4)
                                    .phase(100),
                            ];
                            for wake_latency in [0, 1, 100, u32::MAX] {
                                let profile = PROFILE.wake_latency_us(wake_latency);
                                assert_eq!(
                                    runtime_validation(cases[0], profile),
                                    admit(cases[0], profile).map(|_| ()),
                                    "period={period} deadline={deadline} execution={execution} blocking={blocking} jitter={jitter} wake={wake_latency}"
                                );
                            }
                        }
                    }
                }
            }
        }

        for profile in [
            PROFILE,
            AdmissionProfile::new(1, 16 * 1024, 8, 8),
            AdmissionProfile::new(64 * 1024, 1, 8, 8),
            AdmissionProfile::new(64 * 1024, 16 * 1024, 0, 8),
            AdmissionProfile::new(64 * 1024, 16 * 1024, 8, 1),
        ] {
            assert_eq!(
                runtime_validation(GOOD, profile),
                admit(GOOD, profile).map(|_| ())
            );
        }
    }

    #[test]
    fn zero_jitter_is_a_valid_strict_bound() {
        let task = TaskContract::new(1).deadline(1_000, 1_000, 0, 100, 0);
        let admitted = admit([task], PROFILE).expect("zero jitter is a valid bound");
        assert_eq!(admitted.tasks[0].response_bound_us, 100);
    }

    #[test]
    fn rejects_each_contract_boundary_with_attribution() {
        let cases = [
            (
                TaskContract::new(1).deadline(0, 1, 1, 1, 0),
                AdmissionErrorCode::InvalidDeadline,
            ),
            (
                TaskContract::new(1).deadline(10, 10, 10, 1, 0),
                AdmissionErrorCode::InvalidJitter,
            ),
            (
                TaskContract::new(1).deadline(10, 10, 1, 0, 0),
                AdmissionErrorCode::InvalidExecution,
            ),
            (
                TaskContract::new(1).deadline(10, 10, 1, 8, 3),
                AdmissionErrorCode::InvalidBlocking,
            ),
        ];
        for (task, code) in cases {
            let error = admit([task], PROFILE).unwrap_err();
            assert_eq!(error.code, code);
            assert_eq!(error.task_index, 0);
        }
    }

    #[test]
    fn response_time_rejects_interference_even_below_aggregate_utilization() {
        let tasks = [
            TaskContract::new(1).deadline(5_000, 5_000, 100, 2_000, 0),
            TaskContract::new(2).deadline(7_000, 3_000, 100, 1_500, 0),
        ];
        let error = admit(tasks, PROFILE).unwrap_err();
        assert_eq!(error.code, AdmissionErrorCode::ResponseTimeExceeded);
        assert_eq!(error.task_index, 1);
    }

    #[test]
    fn measured_wake_latency_is_charged_once_and_fails_with_attribution() {
        let task = TaskContract::new(7).deadline(1_000, 1_000, 0, 900, 0);
        let admitted = admit([task], PROFILE.wake_latency_us(100))
            .expect("execution plus the wake bound exactly fits");
        assert_eq!(admitted.tasks[0].response_bound_us, 1_000);

        let error = admit([task], PROFILE.wake_latency_us(101)).unwrap_err();
        assert_eq!(error.code, AdmissionErrorCode::WakeLatencyExceeded);
        assert_eq!(error.task_index, 0);
        assert_eq!(error.observed, 1_001);
        assert_eq!(error.limit, 1_000);
        assert_eq!(
            error.code.diagnostic(),
            "NOBRO-E014 wake-latency bound exceeds deadline"
        );
    }

    #[test]
    fn phase_is_retained_and_invalid_offsets_fail_with_stable_diagnostic() {
        let task = TaskContract::new(7)
            .deadline(1_000, 800, 0, 100, 0)
            .phase(250);
        let admitted = admit([task], PROFILE).expect("valid offset admits");
        assert_eq!(admitted.tasks[0].phase_us, 250);
        assert_eq!(admitted.tasks[0].deadline_us, 800);

        let error = admit([task.phase(1_000)], PROFILE).unwrap_err();
        assert_eq!(error.code, AdmissionErrorCode::InvalidPhase);
        assert_eq!(error.task_index, 0);
        assert_eq!(
            error.code.diagnostic(),
            "NOBRO-E015 phase must be below period"
        );

        let too_long = TaskContract::new(4).deadline(
            MAX_WRAP_SAFE_INTERVAL_US + 1,
            MAX_WRAP_SAFE_INTERVAL_US + 1,
            0,
            1,
            0,
        );
        assert_eq!(
            admit([too_long], PROFILE).unwrap_err().code,
            AdmissionErrorCode::InvalidDeadline
        );
    }

    #[test]
    fn interrupt_domains_are_optional_admitted_interference() {
        let task = TaskContract::new(1).deadline(1_000, 500, 0, 300, 0);
        let irq = InterruptContract::new(9, 2, 1_000, 100, 50, 64)
            .operations(IsrOperations::ACK_PERIPHERAL.union(IsrOperations::MARK_READY));
        let admitted =
            admit_with_interrupts([task], [irq], PROFILE, InterruptProfile::NRF52840_S140)
                .expect("S140 application priority and bounded handoff admit");
        assert_eq!(admitted.tasks[0].response_bound_us, 350);
        assert_eq!(admitted.utilization_permyriad, 3_500);

        let reserved = admit_with_interrupts(
            [task],
            [InterruptContract::new(9, 1, 1_000, 100, 50, 64)],
            PROFILE,
            InterruptProfile::NRF52840_S140,
        )
        .unwrap_err();
        assert_eq!(reserved.code, AdmissionErrorCode::ReservedInterruptPriority);
        assert_eq!(
            reserved.code.diagnostic(),
            "NOBRO-E017 interrupt priority is reserved by the platform stack"
        );
    }

    #[test]
    fn interrupt_deadline_and_nested_stack_fail_closed() {
        let task = TaskContract::new(1).deadline(1_000, 900, 0, 100, 0);
        let too_slow = InterruptContract::new(7, 2, 1_000, 40, 50, 64);
        assert_eq!(
            admit_with_interrupts([task], [too_slow], PROFILE, InterruptProfile::NRF52840_BARE,)
                .unwrap_err()
                .code,
            AdmissionErrorCode::InvalidInterruptContract
        );

        let interrupts = [
            InterruptContract::new(7, 2, 1_000, 100, 10, 128),
            InterruptContract::new(8, 3, 1_000, 100, 10, 128),
        ];
        let profile = InterruptProfile::new(8, 0, 2, 192);
        let error = admit_with_interrupts([task], interrupts, PROFILE, profile).unwrap_err();
        assert_eq!(error.code, AdmissionErrorCode::InterruptStackExceeded);
        assert_eq!(error.observed, 256);

        let too_many_levels = [
            InterruptContract::new(7, 2, 1_000, 100, 10, 64),
            InterruptContract::new(8, 3, 1_000, 100, 10, 64),
            InterruptContract::new(9, 4, 1_000, 100, 10, 64),
        ];
        let error = admit_with_interrupts(
            [task],
            too_many_levels,
            PROFILE,
            InterruptProfile::new(8, 0, 2, 1_024),
        )
        .unwrap_err();
        assert_eq!(error.code, AdmissionErrorCode::InvalidInterruptContract);
        assert_eq!(error.observed, 3);
        assert_eq!(error.limit, 2);
    }

    #[test]
    fn interrupt_sources_may_share_a_priority_with_conservative_interference() {
        let task = TaskContract::new(1).deadline(2_000, 1_500, 0, 100, 0);
        let interrupts = [
            InterruptContract::new(7, 2, 1_000, 200, 50, 64),
            InterruptContract::new(8, 2, 1_000, 200, 40, 64),
        ];
        let admitted =
            admit_with_interrupts([task], interrupts, PROFILE, InterruptProfile::NRF52840_BARE)
                .expect("equal-priority interrupt sources compose");
        assert_eq!(admitted.tasks[0].response_bound_us, 190);

        let one_stack_depth = InterruptProfile::new(8, 0, 2, 64);
        assert!(admit_with_interrupts([task], interrupts, PROFILE, one_stack_depth).is_ok());

        let too_tight = [
            InterruptContract::new(7, 2, 1_000, 80, 50, 64),
            InterruptContract::new(8, 2, 1_000, 80, 40, 64),
        ];
        assert_eq!(
            admit_with_interrupts([task], too_tight, PROFILE, InterruptProfile::NRF52840_BARE,)
                .unwrap_err()
                .code,
            AdmissionErrorCode::InterruptResponseExceeded
        );
    }

    #[test]
    fn capacity_and_identity_fail_closed() {
        let duplicate = [TaskContract::new(7), TaskContract::new(7)];
        assert_eq!(
            admit(duplicate, PROFILE).unwrap_err().code,
            AdmissionErrorCode::DuplicateId
        );
        let too_large = [TaskContract::new(1).memory(65 * 1024, 1, 0)];
        assert_eq!(
            admit(too_large, PROFILE).unwrap_err().code,
            AdmissionErrorCode::FlashExceeded
        );
    }
}
