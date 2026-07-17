//! L0 kernel for workloads admitted before target compilation.
//!
//! It owns no runtime validator, allocator, formatter, report encoder, recovery
//! engine, or async runtime. The admitted table lives in `.rodata`; the target
//! only releases periodic work into a fixed-priority bitmap and dispatches it.

use crate::{StackFault, StackGuardTable};
use nobro_admission::{
    AdmittedWorkload, ADMITTED_SCHEMA_VERSION, MAX_WRAP_SAFE_INTERVAL_US, SUBSYSTEM_ABSENT,
};

pub const SUBSYSTEM_PRESENT: u16 = 0;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelLayer {
    Nano = 0,
    Guarded = 1,
    Managed = 2,
    Assured = 3,
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NanoError {
    UnsupportedSchema,
    EmptyWorkload,
    TooManyTasks,
    InvalidPriority,
    InvalidPeriod,
    MissingStackGuard,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NanoSubsystemReport {
    pub admission_runtime: u16,
    pub recovery: u16,
    pub report: u16,
    pub trace: u16,
    pub quota: u16,
    pub health: u16,
    pub stack_guard: u16,
    pub mpu: u16,
    pub async_rt: u16,
    pub classic_compat: u16,
}

impl NanoSubsystemReport {
    pub const ABSENT: Self = Self {
        admission_runtime: SUBSYSTEM_ABSENT,
        recovery: SUBSYSTEM_ABSENT,
        report: SUBSYSTEM_ABSENT,
        trace: SUBSYSTEM_ABSENT,
        quota: SUBSYSTEM_ABSENT,
        health: SUBSYSTEM_ABSENT,
        stack_guard: SUBSYSTEM_ABSENT,
        mpu: SUBSYSTEM_ABSENT,
        async_rt: SUBSYSTEM_ABSENT,
        classic_compat: SUBSYSTEM_ABSENT,
    };

    pub const GUARDED: Self = Self {
        stack_guard: SUBSYSTEM_PRESENT,
        ..Self::ABSENT
    };
}

/// L1 preset: L0 dispatch plus default-on stack watermark/canary sweeps.
pub struct GuardedNanoKernel<const N: usize, const G: usize> {
    dispatch: NanoKernel<N>,
    guards: StackGuardTable<G>,
}

impl<const N: usize, const G: usize> GuardedNanoKernel<N, G> {
    pub fn new(
        workload: &'static AdmittedWorkload<N>,
        epoch_us: u32,
        guards: StackGuardTable<G>,
    ) -> Result<Self, NanoError> {
        if guards.is_empty() {
            return Err(NanoError::MissingStackGuard);
        }
        Ok(Self {
            dispatch: NanoKernel::new(workload, epoch_us)?,
            guards,
        })
    }

    pub const fn dispatch(&self) -> &NanoKernel<N> {
        &self.dispatch
    }

    pub fn dispatch_mut(&mut self) -> &mut NanoKernel<N> {
        &mut self.dispatch
    }

    pub const fn guards(&self) -> &StackGuardTable<G> {
        &self.guards
    }

    pub fn sweep_stacks(&self) -> Option<StackFault> {
        self.guards.sweep()
    }

    pub const fn subsystem_report(&self) -> NanoSubsystemReport {
        NanoSubsystemReport::GUARDED
    }
}

/// Pre-admitted periodic dispatcher. `N` is limited to 32 so ready state is a
/// single word and selecting the next fixed priority is one trailing-zero op.
pub struct NanoKernel<const N: usize> {
    workload: &'static AdmittedWorkload<N>,
    next_release_us: [u32; N],
    priority_to_task: [u8; 32],
    ready_priorities: u32,
}

impl<const N: usize> NanoKernel<N> {
    pub fn new(workload: &'static AdmittedWorkload<N>, epoch_us: u32) -> Result<Self, NanoError> {
        if workload.schema_version != ADMITTED_SCHEMA_VERSION {
            return Err(NanoError::UnsupportedSchema);
        }
        if workload.task_count == 0 {
            return Err(NanoError::EmptyWorkload);
        }
        if N > 32 || usize::from(workload.task_count) > N {
            return Err(NanoError::TooManyTasks);
        }
        let mut priority_to_task = [u8::MAX; 32];
        let mut next_release_us = [0; N];
        for (index, task) in workload.tasks.iter().enumerate() {
            if task.priority == u16::MAX {
                continue;
            }
            // A zero period denotes an event-only task released through
            // `mark_ready`; only periodic entries need the wrap-safe horizon.
            if task.period_us > MAX_WRAP_SAFE_INTERVAL_US {
                return Err(NanoError::InvalidPeriod);
            }
            let priority = usize::from(task.priority);
            if priority >= usize::from(workload.task_count) || priority_to_task[priority] != u8::MAX
            {
                return Err(NanoError::InvalidPriority);
            }
            priority_to_task[priority] = index as u8;
            next_release_us[index] = epoch_us.wrapping_add(task.phase_us);
        }
        Ok(Self {
            workload,
            next_release_us,
            priority_to_task,
            ready_priorities: 0,
        })
    }

    /// Release every periodic task due at `now_us`, preserving its original
    /// phase after lateness. Returns the number of distinct tasks made ready.
    pub fn release_due(&mut self, now_us: u32) -> u8 {
        let before = self.ready_priorities;
        for (index, task) in self.workload.tasks.iter().enumerate() {
            if task.period_us == 0 || task.priority == u16::MAX {
                continue;
            }
            let due = self.next_release_us[index];
            if now_us.wrapping_sub(due) < 0x8000_0000 {
                self.ready_priorities |= 1u32 << task.priority;
                let elapsed = now_us.wrapping_sub(due);
                let periods = elapsed / task.period_us + 1;
                self.next_release_us[index] =
                    due.wrapping_add(periods.wrapping_mul(task.period_us));
            }
        }
        (self.ready_priorities & !before).count_ones() as u8
    }

    /// Wake a task by its admitted input index (for IRQ/device completion).
    pub fn mark_ready(&mut self, task_index: usize) -> Result<(), NanoError> {
        let Some(task) = self.workload.tasks.get(task_index) else {
            return Err(NanoError::InvalidPriority);
        };
        if task.priority == u16::MAX || task.priority >= 32 {
            return Err(NanoError::InvalidPriority);
        }
        self.ready_priorities |= 1u32 << task.priority;
        Ok(())
    }

    /// Return the earliest periodic release in the wrap-safe `u32` time
    /// domain. A due or overdue release is reported as `now_us`.
    ///
    /// This lets a Nano application compose its own tickless power provider
    /// without enabling the managed executor. Call [`Self::release_due`]
    /// before sleeping so every currently due task has been made ready.
    pub fn next_release_us(&self, now_us: u32) -> Option<u32> {
        let mut earliest_distance: Option<u32> = None;
        for (index, task) in self.workload.tasks.iter().enumerate() {
            if task.period_us == 0 || task.priority == u16::MAX {
                continue;
            }
            let distance = self.next_release_us[index].wrapping_sub(now_us);
            let distance = if distance < 0x8000_0000 { distance } else { 0 };
            earliest_distance = Some(match earliest_distance {
                Some(current) => current.min(distance),
                None => distance,
            });
        }
        earliest_distance.map(|distance| now_us.wrapping_add(distance))
    }

    /// Return the admitted input index of the highest-priority ready task.
    pub fn take_next(&mut self) -> Option<usize> {
        if self.ready_priorities == 0 {
            return None;
        }
        let priority = self.ready_priorities.trailing_zeros() as usize;
        self.ready_priorities &= !(1u32 << priority);
        let index = self.priority_to_task[priority];
        (index != u8::MAX).then_some(usize::from(index))
    }

    pub const fn is_idle(&self) -> bool {
        self.ready_priorities == 0
    }

    pub const fn subsystem_report(&self) -> NanoSubsystemReport {
        NanoSubsystemReport::ABSENT
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nobro_admission::{admit, AdmissionProfile, AdmittedTask, TaskContract};

    const CONTRACTS: [TaskContract; 3] = [
        TaskContract::new(1).deadline(10, 10, 1, 1, 0),
        TaskContract::new(2).deadline(20, 20, 1, 1, 0).phase(5),
        TaskContract::new(3),
    ];
    const WORKLOAD: AdmittedWorkload<3> =
        match admit(CONTRACTS, AdmissionProfile::new(1024, 1024, 0, 3)) {
            Ok(value) => value,
            Err(_) => panic!("fixture must admit"),
        };

    #[test]
    fn releases_preserve_phase_and_dispatch_in_constant_priority_order() {
        let mut kernel = NanoKernel::new(&WORKLOAD, 100).unwrap();
        assert_eq!(kernel.release_due(100), 1);
        assert_eq!(kernel.take_next(), Some(0));
        assert_eq!(kernel.release_due(104), 0);
        assert_eq!(kernel.release_due(105), 1);
        assert_eq!(kernel.take_next(), Some(1));
        assert!(kernel.is_idle());

        assert_eq!(kernel.release_due(139), 2);
        assert_eq!(kernel.take_next(), Some(0));
        assert_eq!(kernel.take_next(), Some(1));
        assert_eq!(kernel.release_due(140), 1);
        assert_eq!(kernel.take_next(), Some(0));
        assert_eq!(kernel.release_due(144), 0);
        assert_eq!(kernel.release_due(145), 1);
        assert_eq!(kernel.take_next(), Some(1));
    }

    #[test]
    fn next_release_supports_tickless_provider_composition() {
        let mut kernel = NanoKernel::new(&WORKLOAD, 100).unwrap();
        assert_eq!(kernel.next_release_us(99), Some(100));
        assert_eq!(kernel.next_release_us(100), Some(100));
        kernel.release_due(100);
        assert_eq!(kernel.next_release_us(100), Some(105));
        kernel.release_due(105);
        assert_eq!(kernel.next_release_us(105), Some(110));
        kernel.release_due(139);
        assert_eq!(kernel.next_release_us(139), Some(140));
    }

    #[test]
    fn next_release_preserves_wrap_safe_phase() {
        let epoch = u32::MAX - 3;
        let mut kernel = NanoKernel::new(&WORKLOAD, epoch).unwrap();
        kernel.release_due(epoch);
        assert_eq!(kernel.next_release_us(epoch), Some(1));
    }

    #[test]
    fn malformed_workload_cannot_bypass_wrap_safe_period_gate() {
        static WORKLOAD: AdmittedWorkload<1> = AdmittedWorkload {
            schema_version: ADMITTED_SCHEMA_VERSION,
            task_count: 1,
            tasks: [AdmittedTask {
                id: 1,
                priority: 0,
                phase_us: 0,
                period_us: MAX_WRAP_SAFE_INTERVAL_US + 1,
                deadline_us: 1,
                response_bound_us: 1,
                capability_bits: 0,
                quota_bits: 0,
            }],
            flash_bytes: 0,
            ram_bytes: 0,
            pool_slots: 0,
            utilization_permyriad: 0,
        };
        assert!(matches!(
            NanoKernel::new(&WORKLOAD, 0),
            Err(NanoError::InvalidPeriod)
        ));
    }

    #[test]
    fn device_wake_and_absence_report_are_unambiguous() {
        let mut kernel = NanoKernel::new(&WORKLOAD, 100).unwrap();
        kernel.mark_ready(2).unwrap();
        assert_eq!(kernel.take_next(), Some(2));
        assert_eq!(kernel.subsystem_report().recovery, SUBSYSTEM_ABSENT);
    }

    #[test]
    fn guarded_layer_rejects_an_empty_guard_contract() {
        assert!(matches!(
            GuardedNanoKernel::new(&WORKLOAD, 0, StackGuardTable::<0>::new()),
            Err(NanoError::MissingStackGuard)
        ));
        assert_eq!(NanoSubsystemReport::GUARDED.stack_guard, SUBSYSTEM_PRESENT);
        assert_eq!(NanoSubsystemReport::GUARDED.recovery, SUBSYSTEM_ABSENT);
    }
}
