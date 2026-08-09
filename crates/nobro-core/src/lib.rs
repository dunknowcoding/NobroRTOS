// SPDX-License-Identifier: GPL-3.0-only
//! NobroRTOS Core: a dependency-free, allocation-free periodic/event dispatcher.
//!
//! Core assumes task contracts were admitted before a target image is deployed.
//! It owns no allocator, task stacks, drivers, async runtime, recovery engine,
//! formatter, or dynamic module loader. A board binding owns its timer, idle
//! instruction, interrupts, watchdog, and task execution.

#![no_std]
#![forbid(unsafe_code)]

/// At most one 32-bit ready bitmap is retained.
pub const MAX_TASKS: usize = 32;

/// Periodic arithmetic remains unambiguous across one `u32` clock wrap.
pub const MAX_WRAP_SAFE_INTERVAL_US: u32 = 0x7fff_ffff;

/// One task known before target compilation.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskContract {
    id: u16,
    priority: u8,
    _reserved: u8,
    period_us: u32,
    phase_us: u32,
    finish_deadline_us: u32,
    wcet_us: u32,
}

impl TaskContract {
    /// Create an event-only task. Lower numeric priorities run first.
    pub const fn event(id: u16, priority: u8, wcet_us: u32) -> Self {
        Self {
            id,
            priority,
            _reserved: 0,
            period_us: 0,
            phase_us: 0,
            finish_deadline_us: 0,
            wcet_us,
        }
    }

    /// Create a periodic task with a constrained finish deadline.
    pub const fn periodic(
        id: u16,
        priority: u8,
        period_us: u32,
        finish_deadline_us: u32,
        wcet_us: u32,
    ) -> Self {
        Self {
            id,
            priority,
            _reserved: 0,
            period_us,
            phase_us: 0,
            finish_deadline_us,
            wcet_us,
        }
    }

    pub const fn phase(mut self, phase_us: u32) -> Self {
        self.phase_us = phase_us;
        self
    }

    pub const fn id(&self) -> u16 {
        self.id
    }

    pub const fn priority(&self) -> u8 {
        self.priority
    }

    pub const fn period_us(&self) -> u32 {
        self.period_us
    }

    pub const fn phase_us(&self) -> u32 {
        self.phase_us
    }

    pub const fn finish_deadline_us(&self) -> u32 {
        self.finish_deadline_us
    }

    pub const fn wcet_us(&self) -> u32 {
        self.wcet_us
    }

    pub const fn is_event_only(&self) -> bool {
        self.period_us == 0
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    Empty,
    TooManyTasks,
    DuplicateId,
    DuplicatePriority,
    InvalidPriority,
    InvalidTiming,
    UtilizationExceeded,
    DeadlineMiss,
}

/// A task table that passed the Core non-preemptive admission checks.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmittedWorkload<const N: usize> {
    tasks: [TaskContract; N],
}

impl<const N: usize> AdmittedWorkload<N> {
    pub const fn tasks(&self) -> &[TaskContract; N] {
        &self.tasks
    }

    pub const fn task(&self, index: usize) -> Option<&TaskContract> {
        if index < N {
            Some(&self.tasks[index])
        } else {
            None
        }
    }
}

/// Admit one static, fixed-priority, non-preemptive Core workload.
///
/// WCET values must include the board's dispatch and interrupt overhead. The
/// response-time test conservatively includes one lower-priority blocking job
/// and all higher-priority interference. Event-only work is admitted for
/// explicit wakeup but is not assigned an unmeasured arrival rate.
pub const fn admit<const N: usize>(
    tasks: [TaskContract; N],
) -> Result<AdmittedWorkload<N>, AdmissionError> {
    if N == 0 {
        return Err(AdmissionError::Empty);
    }
    if N > MAX_TASKS {
        return Err(AdmissionError::TooManyTasks);
    }

    let mut priority_bits = 0u32;
    let mut utilization_q32 = 0u64;
    let mut index = 0;
    while index < N {
        let task = tasks[index];
        if task.priority as usize >= MAX_TASKS {
            return Err(AdmissionError::InvalidPriority);
        }
        let priority_bit = 1u32 << task.priority;
        if priority_bits & priority_bit != 0 {
            return Err(AdmissionError::DuplicatePriority);
        }
        priority_bits |= priority_bit;

        let mut prior = 0;
        while prior < index {
            if tasks[prior].id == task.id {
                return Err(AdmissionError::DuplicateId);
            }
            prior += 1;
        }

        if task.wcet_us == 0 || task.wcet_us > MAX_WRAP_SAFE_INTERVAL_US {
            return Err(AdmissionError::InvalidTiming);
        }
        if task.period_us == 0 {
            if task.phase_us != 0 || task.finish_deadline_us != 0 {
                return Err(AdmissionError::InvalidTiming);
            }
        } else {
            if task.period_us > MAX_WRAP_SAFE_INTERVAL_US
                || task.phase_us >= task.period_us
                || task.finish_deadline_us == 0
                || task.finish_deadline_us > task.period_us
                || task.wcet_us > task.finish_deadline_us
            {
                return Err(AdmissionError::InvalidTiming);
            }
            let scaled = (task.wcet_us as u64) << 32;
            let share = scaled.div_ceil(task.period_us as u64);
            utilization_q32 += share;
            if utilization_q32 > (1u64 << 32) {
                return Err(AdmissionError::UtilizationExceeded);
            }
        }
        index += 1;
    }

    index = 0;
    while index < N {
        let task = tasks[index];
        if task.period_us != 0 {
            let mut blocking = 0u64;
            let mut other = 0;
            while other < N {
                let candidate = tasks[other];
                if candidate.priority > task.priority && candidate.wcet_us as u64 > blocking {
                    blocking = candidate.wcet_us as u64;
                }
                other += 1;
            }

            let mut response = task.wcet_us as u64 + blocking;
            if response > task.finish_deadline_us as u64 {
                return Err(AdmissionError::DeadlineMiss);
            }
            loop {
                let mut updated = task.wcet_us as u64 + blocking;
                other = 0;
                while other < N {
                    let higher = tasks[other];
                    if higher.period_us != 0 && higher.priority < task.priority {
                        let jobs = response.div_ceil(higher.period_us as u64);
                        updated += jobs * higher.wcet_us as u64;
                    }
                    other += 1;
                }
                if updated == response {
                    break;
                }
                if updated > task.finish_deadline_us as u64 || updated < response {
                    return Err(AdmissionError::DeadlineMiss);
                }
                response = updated;
            }
        }
        index += 1;
    }

    Ok(AdmittedWorkload { tasks })
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelError {
    InvalidTask,
}

/// Allocation-free dispatcher over a statically admitted task table.
pub struct CoreKernel<'a, const N: usize> {
    workload: &'a AdmittedWorkload<N>,
    next_release_us: [u32; N],
    priority_to_task: [u8; MAX_TASKS],
    ready_priorities: u32,
}

impl<'a, const N: usize> CoreKernel<'a, N> {
    pub fn start(workload: &'a AdmittedWorkload<N>, epoch_us: u32) -> Self {
        let mut next_release_us = [0u32; N];
        let mut priority_to_task = [u8::MAX; MAX_TASKS];
        let mut index = 0;
        while index < N {
            let task = workload.tasks[index];
            priority_to_task[task.priority as usize] = index as u8;
            next_release_us[index] = epoch_us.wrapping_add(task.phase_us);
            index += 1;
        }
        Self {
            workload,
            next_release_us,
            priority_to_task,
            ready_priorities: 0,
        }
    }

    /// Release every periodic task due at `now_us` while preserving phase.
    pub fn release_due(&mut self, now_us: u32) -> u8 {
        let before = self.ready_priorities;
        let mut index = 0;
        while index < N {
            let task = self.workload.tasks[index];
            if task.period_us != 0 {
                let release = self.next_release_us[index];
                if now_us.wrapping_sub(release) < 0x8000_0000 {
                    self.ready_priorities |= 1u32 << task.priority;
                    let elapsed = now_us.wrapping_sub(release);
                    let periods = elapsed / task.period_us + 1;
                    self.next_release_us[index] =
                        release.wrapping_add(periods.wrapping_mul(task.period_us));
                }
            }
            index += 1;
        }
        (self.ready_priorities & !before).count_ones() as u8
    }

    /// Wake an event or periodic task by its admitted input index.
    pub fn mark_ready(&mut self, task_index: usize) -> Result<(), KernelError> {
        if task_index >= N {
            return Err(KernelError::InvalidTask);
        }
        let priority = self.workload.tasks[task_index].priority;
        self.ready_priorities |= 1u32 << priority;
        Ok(())
    }

    /// Take the highest-priority ready task. Lower priority numbers run first.
    pub fn take_next(&mut self) -> Option<usize> {
        if self.ready_priorities == 0 {
            return None;
        }
        let priority = self.ready_priorities.trailing_zeros() as usize;
        self.ready_priorities &= !(1u32 << priority);
        let index = self.priority_to_task[priority];
        if index == u8::MAX {
            None
        } else {
            Some(index as usize)
        }
    }

    /// Earliest periodic wake in the wrap-safe clock domain.
    pub fn next_release_us(&self, now_us: u32) -> Option<u32> {
        let mut nearest: Option<u32> = None;
        let mut index = 0;
        while index < N {
            let task = self.workload.tasks[index];
            if task.period_us != 0 {
                let raw = self.next_release_us[index].wrapping_sub(now_us);
                let distance = if raw < 0x8000_0000 { raw } else { 0 };
                nearest = Some(match nearest {
                    Some(current) if current <= distance => current,
                    _ => distance,
                });
            }
            index += 1;
        }
        nearest.map(|distance| now_us.wrapping_add(distance))
    }

    pub const fn is_idle(&self) -> bool {
        self.ready_priorities == 0
    }

    pub const fn workload(&self) -> &AdmittedWorkload<N> {
        self.workload
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TASKS: [TaskContract; 3] = [
        TaskContract::periodic(1, 0, 10, 10, 1),
        TaskContract::periodic(2, 3, 20, 20, 2).phase(5),
        TaskContract::event(3, 7, 1),
    ];
    const WORKLOAD: AdmittedWorkload<3> = match admit(TASKS) {
        Ok(value) => value,
        Err(_) => panic!("fixture must admit"),
    };

    #[test]
    fn dispatches_in_priority_order_and_preserves_phase() {
        let mut kernel = CoreKernel::start(&WORKLOAD, 100);
        assert_eq!(kernel.release_due(100), 1);
        kernel.mark_ready(2).unwrap();
        assert_eq!(kernel.take_next(), Some(0));
        assert_eq!(kernel.take_next(), Some(2));
        assert_eq!(kernel.next_release_us(100), Some(105));
        assert_eq!(kernel.release_due(105), 1);
        assert_eq!(kernel.take_next(), Some(1));
        assert!(kernel.is_idle());
    }

    #[test]
    fn late_release_skips_to_the_next_original_phase() {
        let mut kernel = CoreKernel::start(&WORKLOAD, 0);
        assert_eq!(kernel.release_due(39), 2);
        assert_eq!(kernel.next_release_us(39), Some(40));
    }

    #[test]
    fn wrapping_clock_remains_unambiguous() {
        let mut kernel = CoreKernel::start(&WORKLOAD, u32::MAX - 4);
        assert_eq!(kernel.release_due(u32::MAX - 4), 1);
        assert_eq!(kernel.next_release_us(u32::MAX - 4), Some(0));
        assert_eq!(kernel.release_due(0), 1);
    }

    #[test]
    fn invalid_index_is_rejected_without_ready_state() {
        let mut kernel = CoreKernel::start(&WORKLOAD, 0);
        assert_eq!(kernel.mark_ready(3), Err(KernelError::InvalidTask));
        assert!(kernel.is_idle());
    }

    #[test]
    fn admission_rejects_duplicate_identity_and_priority() {
        assert_eq!(
            admit([TaskContract::event(1, 0, 1), TaskContract::event(1, 1, 1),]),
            Err(AdmissionError::DuplicateId)
        );
        assert_eq!(
            admit([TaskContract::event(1, 0, 1), TaskContract::event(2, 0, 1),]),
            Err(AdmissionError::DuplicatePriority)
        );
    }

    #[test]
    fn admission_rejects_invalid_and_wrap_unsafe_timing() {
        assert_eq!(
            admit([TaskContract::periodic(1, 0, 10, 11, 1)]),
            Err(AdmissionError::InvalidTiming)
        );
        assert_eq!(
            admit([TaskContract::periodic(
                1,
                0,
                MAX_WRAP_SAFE_INTERVAL_US + 1,
                10,
                1,
            )]),
            Err(AdmissionError::InvalidTiming)
        );
    }

    #[test]
    fn admission_rejects_overload() {
        assert_eq!(
            admit([
                TaskContract::periodic(1, 0, 10, 10, 6),
                TaskContract::periodic(2, 1, 10, 10, 6),
            ]),
            Err(AdmissionError::UtilizationExceeded)
        );
    }

    #[test]
    fn admission_rejects_nonpreemptive_blocking_deadline_miss() {
        assert_eq!(
            admit([
                TaskContract::periodic(1, 0, 10, 2, 1),
                TaskContract::periodic(2, 1, 100, 100, 4),
            ]),
            Err(AdmissionError::DeadlineMiss)
        );
    }

    #[test]
    fn event_only_task_has_no_invented_arrival_rate() {
        let admitted = admit([TaskContract::event(9, 31, 3)]).unwrap();
        let mut kernel = CoreKernel::start(&admitted, 0);
        assert_eq!(kernel.release_due(0), 0);
        assert_eq!(kernel.next_release_us(0), None);
        kernel.mark_ready(0).unwrap();
        assert_eq!(kernel.take_next(), Some(0));
    }
}
