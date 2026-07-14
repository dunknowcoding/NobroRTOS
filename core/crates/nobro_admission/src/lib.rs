//! Allocation-free admission shared by firmware build scripts and the kernel.
//!
//! The input is deliberately data-only. Build scripts can run the same bounded
//! fixed-priority response-time analysis as target-side dynamic admission, then
//! emit only [`AdmittedWorkload`] into firmware read-only data.

#![no_std]

/// Report status used when a subsystem was intentionally not linked.
pub const SUBSYSTEM_ABSENT: u16 = 0xFFFF;
pub const ADMITTED_SCHEMA_VERSION: u16 = 1;

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
    pub quota_bits: u32,
}

impl TaskContract {
    /// Unused capacity in a fixed array. It cannot be constructed accidentally
    /// through the public task builder.
    pub const EMPTY: Self = Self {
        active: false,
        id: 0,
        priority_key: u16::MAX,
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

    const fn participates(self) -> bool {
        self.deadline_us != 0
    }

    const fn active(self) -> bool {
        self.active
    }
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
            if task.period_us == 0 || task.deadline_us > task.period_us {
                return Err(AdmissionError::task(
                    AdmissionErrorCode::InvalidDeadline,
                    index,
                    task.deadline_us as u64,
                    task.period_us as u64,
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
        } else if task.period_us != 0
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
