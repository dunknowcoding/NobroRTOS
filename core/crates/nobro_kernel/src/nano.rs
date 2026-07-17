//! L0 kernel for workloads admitted before target compilation.
//!
//! It owns no runtime validator, allocator, formatter, report encoder, recovery
//! engine, or async runtime. The admitted table lives in `.rodata`; the target
//! only releases periodic work into a fixed-priority bitmap and dispatches it.

use crate::{
    Action, Capability, EventLog, FaultThresholdError, FaultThresholds, HealthCounters,
    KernelError, ModuleId, ObjectKind, RecoveryCoordinator, RecoveryError, RecoveryOutcome,
    RecoveryPlan, RecoveryPlanError, RecoveryPlanPolicy, StackFault, StackGuardTable, SystemState,
};
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
    /// Runtime capability enforcement; appended to preserve prior field offsets.
    pub capability: u16,
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
        capability: SUBSYSTEM_ABSENT,
    };

    pub const GUARDED: Self = Self {
        stack_guard: SUBSYSTEM_PRESENT,
        ..Self::ABSENT
    };

    pub const GOVERNED: Self = Self {
        capability: SUBSYSTEM_PRESENT,
        quota: SUBSYSTEM_PRESENT,
        ..Self::ABSENT
    };

    pub const SUPERVISED: Self = Self {
        recovery: SUBSYSTEM_PRESENT,
        health: SUBSYSTEM_PRESENT,
        ..Self::ABSENT
    };

    pub const TRACED: Self = Self {
        trace: SUBSYSTEM_PRESENT,
        ..Self::ABSENT
    };

    /// Merge independently selected Nano services into one absence report.
    pub const fn union(self, other: Self) -> Self {
        const fn selected(left: u16, right: u16) -> u16 {
            if left == SUBSYSTEM_ABSENT {
                right
            } else {
                left
            }
        }

        Self {
            admission_runtime: selected(self.admission_runtime, other.admission_runtime),
            recovery: selected(self.recovery, other.recovery),
            report: selected(self.report, other.report),
            trace: selected(self.trace, other.trace),
            quota: selected(self.quota, other.quota),
            health: selected(self.health, other.health),
            stack_guard: selected(self.stack_guard, other.stack_guard),
            mpu: selected(self.mpu, other.mpu),
            async_rt: selected(self.async_rt, other.async_rt),
            classic_compat: selected(self.classic_compat, other.classic_compat),
            capability: selected(self.capability, other.capability),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NanoObjectUsage {
    pub mailbox_slots: u8,
    pub alarms: u8,
    pub kv_entries: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NanoGovernanceError {
    InvalidTask(usize),
    CapabilityDenied {
        task_index: usize,
        capability: Capability,
    },
    QuotaExceeded {
        task_index: usize,
        kind: ObjectKind,
        limit: u8,
    },
    QuotaUnderflow {
        task_index: usize,
        kind: ObjectKind,
    },
}

/// Optional L0 governance using only bindings already admitted into `.rodata`.
///
/// The service retains one packed usage word per task. It owns no manifest,
/// runtime admission engine, module registry, allocator, health service, or
/// trace. Dropping it from the application removes both its state and code.
pub struct NanoGovernance<const N: usize> {
    workload: &'static AdmittedWorkload<N>,
    usage_bits: [u32; N],
}

impl<const N: usize> NanoGovernance<N> {
    fn new(workload: &'static AdmittedWorkload<N>) -> Self {
        Self {
            workload,
            usage_bits: [0; N],
        }
    }

    pub fn authorize(
        &self,
        task_index: usize,
        capability: Capability,
    ) -> Result<(), NanoGovernanceError> {
        let task = self.task(task_index)?;
        if task.capability_bits & capability.bit() == 0 {
            return Err(NanoGovernanceError::CapabilityDenied {
                task_index,
                capability,
            });
        }
        Ok(())
    }

    pub fn charge(
        &mut self,
        task_index: usize,
        kind: ObjectKind,
    ) -> Result<(), NanoGovernanceError> {
        let task = self.task(task_index)?;
        let shift = Self::quota_shift(kind);
        let limit = ((task.quota_bits >> shift) & 0xFF) as u8;
        let used = ((self.usage_bits[task_index] >> shift) & 0xFF) as u8;
        if used >= limit {
            return Err(NanoGovernanceError::QuotaExceeded {
                task_index,
                kind,
                limit,
            });
        }
        self.usage_bits[task_index] += 1u32 << shift;
        Ok(())
    }

    pub fn release(
        &mut self,
        task_index: usize,
        kind: ObjectKind,
    ) -> Result<(), NanoGovernanceError> {
        self.task(task_index)?;
        let shift = Self::quota_shift(kind);
        let used = ((self.usage_bits[task_index] >> shift) & 0xFF) as u8;
        if used == 0 {
            return Err(NanoGovernanceError::QuotaUnderflow { task_index, kind });
        }
        self.usage_bits[task_index] -= 1u32 << shift;
        Ok(())
    }

    pub fn usage(&self, task_index: usize) -> Option<NanoObjectUsage> {
        self.task(task_index).ok()?;
        let bits = self.usage_bits[task_index];
        Some(NanoObjectUsage {
            mailbox_slots: bits as u8,
            alarms: (bits >> 8) as u8,
            kv_entries: (bits >> 16) as u8,
        })
    }

    /// Recovery cleanup for task-owned quota state. Returns the released usage.
    pub fn clear_task(
        &mut self,
        task_index: usize,
    ) -> Result<NanoObjectUsage, NanoGovernanceError> {
        let usage = self
            .usage(task_index)
            .ok_or(NanoGovernanceError::InvalidTask(task_index))?;
        self.usage_bits[task_index] = 0;
        Ok(usage)
    }

    pub const fn subsystem_report(&self) -> NanoSubsystemReport {
        NanoSubsystemReport::GOVERNED
    }

    fn task(
        &self,
        task_index: usize,
    ) -> Result<&nobro_admission::AdmittedTask, NanoGovernanceError> {
        if task_index >= usize::from(self.workload.task_count) {
            return Err(NanoGovernanceError::InvalidTask(task_index));
        }
        self.workload
            .tasks
            .get(task_index)
            .ok_or(NanoGovernanceError::InvalidTask(task_index))
    }

    const fn quota_shift(kind: ObjectKind) -> u32 {
        match kind {
            ObjectKind::MailboxSlot => 0,
            ObjectKind::Alarm => 8,
            ObjectKind::KvEntry => 16,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NanoRecoveryOutcome {
    pub task_index: usize,
    pub task_id: u16,
    pub error: KernelError,
    pub action: Action,
    pub state: SystemState,
    pub coalesced: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NanoRecoveryError {
    InvalidThresholds(FaultThresholdError),
    InvalidTask(usize),
    MismatchedWorkload,
    DuplicateDependency {
        task_index: usize,
        dependency_index: usize,
    },
    DependencyCycle,
    Coordinator(RecoveryError),
    Plan(RecoveryPlanError),
    CannotRestore(SystemState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NanoRecoveryPlanning<const STEPS: usize> {
    pub outcome: NanoRecoveryOutcome,
    pub plan: RecoveryPlan<STEPS>,
}

/// Task-index dependency graph for bounded Nano recovery planning.
///
/// Each dependency is declared once with [`Self::depends_on`]. Recovery plans
/// quiesce transitive dependents before restarting the failed task, then resume
/// them in dependency order.
pub struct NanoDependencies<const N: usize> {
    workload: &'static AdmittedWorkload<N>,
    depends_on: [u32; N],
}

impl<const N: usize> NanoDependencies<N> {
    fn new(workload: &'static AdmittedWorkload<N>) -> Self {
        Self {
            workload,
            depends_on: [0; N],
        }
    }

    pub fn depends_on(
        &mut self,
        task_index: usize,
        dependency_index: usize,
    ) -> Result<&mut Self, NanoRecoveryError> {
        self.module(task_index)?;
        self.module(dependency_index)?;
        let dependency_bit = 1u32 << dependency_index;
        if self.depends_on[task_index] & dependency_bit != 0 {
            return Err(NanoRecoveryError::DuplicateDependency {
                task_index,
                dependency_index,
            });
        }
        self.depends_on[task_index] |= dependency_bit;
        Ok(self)
    }

    fn dependency_impact(
        &self,
        root_index: usize,
    ) -> Result<crate::DependencyImpact<N>, NanoRecoveryError> {
        self.module(root_index)?;
        let count = usize::from(self.workload.task_count);
        let mut order = [0usize; N];
        let mut order_len = 0;
        let mut emitted = 0u32;
        while order_len < count {
            let before = order_len;
            for task_index in 0..count {
                let task_bit = 1u32 << task_index;
                if emitted & task_bit == 0 && self.depends_on[task_index] & !emitted == 0 {
                    order[order_len] = task_index;
                    order_len += 1;
                    emitted |= task_bit;
                }
            }
            if order_len == before {
                return Err(NanoRecoveryError::DependencyCycle);
            }
        }

        let root_bit = 1u32 << root_index;
        let mut affected = 0u32;
        loop {
            let before = affected;
            for task_index in 0..count {
                let task_bit = 1u32 << task_index;
                if task_index != root_index
                    && affected & task_bit == 0
                    && self.depends_on[task_index] & (root_bit | affected) != 0
                {
                    affected |= task_bit;
                }
            }
            if affected == before {
                break;
            }
        }

        let mut impact = crate::DependencyImpact::new(ModuleId::App(root_index as u8));
        for task_index in order.into_iter().take(order_len).rev() {
            if affected & (1u32 << task_index) != 0 {
                impact.affected[impact.affected_count] = Some(ModuleId::App(task_index as u8));
                impact.affected_count += 1;
            }
        }
        Ok(impact)
    }

    fn module(&self, task_index: usize) -> Result<ModuleId, NanoRecoveryError> {
        module_for(self.workload, task_index)
    }
}

/// Optional health escalation and lifecycle recovery for pre-admitted tasks.
///
/// Tasks are addressed by their Nano input index; callers do not need to
/// construct module identifiers or a runtime manifest. Retained event tracing
/// is selected by its const capacity; dependency plans and the managed runtime
/// remain separate choices.
pub struct NanoRecovery<const N: usize, const LOG: usize = 0> {
    workload: &'static AdmittedWorkload<N>,
    coordinator: RecoveryCoordinator<N, LOG>,
}

impl<const N: usize, const LOG: usize> NanoRecovery<N, LOG> {
    fn new(
        workload: &'static AdmittedWorkload<N>,
        thresholds: FaultThresholds,
        now_us: u64,
    ) -> Result<Self, NanoRecoveryError> {
        thresholds
            .validate()
            .map_err(NanoRecoveryError::InvalidThresholds)?;
        let mut recovery = Self {
            workload,
            coordinator: RecoveryCoordinator::new(thresholds),
        };
        recovery.enter_running(now_us)?;
        Ok(recovery)
    }

    unsafe fn init_in_place(
        destination: *mut Self,
        workload: &'static AdmittedWorkload<N>,
        thresholds: FaultThresholds,
    ) {
        core::ptr::addr_of_mut!((*destination).workload).write(workload);
        RecoveryCoordinator::init_in_place(
            core::ptr::addr_of_mut!((*destination).coordinator),
            thresholds,
        );
    }

    fn enter_running(&mut self, now_us: u64) -> Result<(), NanoRecoveryError> {
        for state in [
            SystemState::ValidateManifest,
            SystemState::InitDrivers,
            SystemState::Running,
        ] {
            self.coordinator
                .transition(state, now_us)
                .map_err(NanoRecoveryError::Coordinator)?;
        }
        Ok(())
    }

    pub fn record_ok(&mut self, task_index: usize, now_us: u64) -> Result<(), NanoRecoveryError> {
        let module = self.module(task_index)?;
        self.coordinator.record_ok(module, now_us);
        Ok(())
    }

    pub fn record_error(
        &mut self,
        task_index: usize,
        error: KernelError,
        now_us: u64,
    ) -> Result<NanoRecoveryOutcome, NanoRecoveryError> {
        let module = self.module(task_index)?;
        let outcome = self
            .coordinator
            .record_error(module, error, now_us)
            .map_err(NanoRecoveryError::Coordinator)?;
        Ok(NanoRecoveryOutcome {
            task_index,
            task_id: self.workload.tasks[task_index].id,
            error: outcome.error,
            action: outcome.action,
            state: outcome.state,
            coalesced: outcome.coalesced,
        })
    }

    /// Record one error and produce its bounded recovery plan.
    pub fn record_error_with_plan<const STEPS: usize>(
        &mut self,
        task_index: usize,
        error: KernelError,
        now_us: u64,
        policy: RecoveryPlanPolicy,
    ) -> Result<NanoRecoveryPlanning<STEPS>, NanoRecoveryError> {
        let outcome = self.record_error(task_index, error, now_us)?;
        let plan = RecoveryPlan::from_outcome(self.raw_outcome(outcome)?, now_us, policy)
            .map_err(NanoRecoveryError::Plan)?;
        Ok(NanoRecoveryPlanning { outcome, plan })
    }

    /// Record one error and include transitive task dependencies in its plan.
    pub fn record_error_with_dependencies<const STEPS: usize>(
        &mut self,
        task_index: usize,
        error: KernelError,
        now_us: u64,
        policy: RecoveryPlanPolicy,
        dependencies: &NanoDependencies<N>,
    ) -> Result<NanoRecoveryPlanning<STEPS>, NanoRecoveryError> {
        if !core::ptr::eq(self.workload, dependencies.workload) {
            return Err(NanoRecoveryError::MismatchedWorkload);
        }
        self.module(task_index)?;
        let impact = dependencies.dependency_impact(task_index)?;
        let outcome = self.record_error(task_index, error, now_us)?;
        let plan = RecoveryPlan::from_outcome_with_impact(
            self.raw_outcome(outcome)?,
            &impact,
            now_us,
            policy,
        )
        .map_err(NanoRecoveryError::Plan)?;
        Ok(NanoRecoveryPlanning { outcome, plan })
    }

    pub fn counters(&self, task_index: usize) -> Result<Option<HealthCounters>, NanoRecoveryError> {
        let module = self.module(task_index)?;
        Ok(self
            .coordinator
            .snapshot(module)
            .map(|snapshot| snapshot.counters))
    }

    /// Return a degraded or recovering system to `Running` after the caller
    /// has completed the selected recovery action.
    pub fn restore_running(&mut self, now_us: u64) -> Result<(), NanoRecoveryError> {
        match self.coordinator.state() {
            SystemState::Running => Ok(()),
            SystemState::Degraded => self
                .coordinator
                .transition(SystemState::Running, now_us)
                .map_err(NanoRecoveryError::Coordinator),
            SystemState::Recovering => {
                self.coordinator
                    .transition(SystemState::InitDrivers, now_us)
                    .map_err(NanoRecoveryError::Coordinator)?;
                self.coordinator
                    .transition(SystemState::Running, now_us)
                    .map_err(NanoRecoveryError::Coordinator)
            }
            state => Err(NanoRecoveryError::CannotRestore(state)),
        }
    }

    pub const fn state(&self) -> SystemState {
        self.coordinator.state()
    }

    pub fn events(&self) -> &EventLog<LOG> {
        self.coordinator.events()
    }

    pub const fn subsystem_report(&self) -> NanoSubsystemReport {
        if LOG == 0 {
            NanoSubsystemReport::SUPERVISED
        } else {
            NanoSubsystemReport::SUPERVISED.union(NanoSubsystemReport::TRACED)
        }
    }

    fn module(&self, task_index: usize) -> Result<ModuleId, NanoRecoveryError> {
        module_for(self.workload, task_index)
    }

    fn raw_outcome(
        &self,
        outcome: NanoRecoveryOutcome,
    ) -> Result<RecoveryOutcome, NanoRecoveryError> {
        let module = self.module(outcome.task_index)?;
        if self.workload.tasks[outcome.task_index].id != outcome.task_id {
            return Err(NanoRecoveryError::InvalidTask(outcome.task_index));
        }
        Ok(RecoveryOutcome {
            module,
            error: outcome.error,
            action: outcome.action,
            state: outcome.state,
            coalesced: outcome.coalesced,
        })
    }
}

fn module_for<const N: usize>(
    workload: &AdmittedWorkload<N>,
    task_index: usize,
) -> Result<ModuleId, NanoRecoveryError> {
    if task_index >= usize::from(workload.task_count) || workload.tasks.get(task_index).is_none() {
        return Err(NanoRecoveryError::InvalidTask(task_index));
    }
    Ok(ModuleId::App(task_index as u8))
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
        NanoKernel::new(workload, epoch_us)?.with_stack_guards(guards)
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

    /// Add stack guarding to an already configured Nano dispatcher.
    ///
    /// This is the zero-revalidation path from the L0 preset to L1: task
    /// admission, epoch, pending releases, and ready membership stay intact.
    /// The caller still owns the stack-region registration and its safety
    /// contract; an empty table fails closed instead of advertising a guarded
    /// profile that protects no execution context.
    pub fn with_stack_guards<const G: usize>(
        self,
        guards: StackGuardTable<G>,
    ) -> Result<GuardedNanoKernel<N, G>, NanoError> {
        if guards.is_empty() {
            return Err(NanoError::MissingStackGuard);
        }
        Ok(GuardedNanoKernel {
            dispatch: self,
            guards,
        })
    }

    /// Select lightweight capability/quota enforcement from admitted bindings.
    ///
    /// This service is independent of stack guards and the managed runtime:
    /// construct it only when operations need runtime authorization/accounting.
    pub fn governance(&self) -> NanoGovernance<N> {
        NanoGovernance::new(self.workload)
    }

    /// Select health escalation and lifecycle recovery without the managed
    /// runtime or a retained event trace.
    pub fn recovery(
        &self,
        thresholds: FaultThresholds,
        now_us: u64,
    ) -> Result<NanoRecovery<N>, NanoRecoveryError> {
        NanoRecovery::new(self.workload, thresholds, now_us)
    }

    /// Select recovery with a retained, fixed-capacity event trace.
    pub fn recovery_with_trace<const LOG: usize>(
        &self,
        thresholds: FaultThresholds,
        now_us: u64,
    ) -> Result<NanoRecovery<N, LOG>, NanoRecoveryError> {
        NanoRecovery::new(self.workload, thresholds, now_us)
    }

    /// Build a task-index dependency graph for bounded recovery plans.
    pub fn recovery_dependencies(&self) -> NanoDependencies<N> {
        NanoDependencies::new(self.workload)
    }

    /// Initialize recovery directly in caller-owned storage.
    ///
    /// This is the bounded-stack form for static or long-lived MCU services;
    /// it preserves the same short task-index API as [`Self::recovery`].
    pub fn recovery_into<'a>(
        &self,
        destination: &'a mut core::mem::MaybeUninit<NanoRecovery<N>>,
        thresholds: FaultThresholds,
        now_us: u64,
    ) -> Result<&'a mut NanoRecovery<N>, NanoRecoveryError> {
        self.recovery_with_trace_into(destination, thresholds, now_us)
    }

    /// Initialize traced recovery directly in caller-owned storage.
    pub fn recovery_with_trace_into<'a, const LOG: usize>(
        &self,
        destination: &'a mut core::mem::MaybeUninit<NanoRecovery<N, LOG>>,
        thresholds: FaultThresholds,
        now_us: u64,
    ) -> Result<&'a mut NanoRecovery<N, LOG>, NanoRecoveryError> {
        thresholds
            .validate()
            .map_err(NanoRecoveryError::InvalidThresholds)?;
        unsafe {
            NanoRecovery::init_in_place(destination.as_mut_ptr(), self.workload, thresholds);
        }
        let recovery = unsafe { destination.assume_init_mut() };
        recovery.enter_running(now_us)?;
        Ok(recovery)
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
    const GOVERNED_CONTRACTS: [TaskContract; 1] = [TaskContract::new(7)
        .deadline(10, 10, 1, 1, 0)
        .bindings(Capability::Mailbox.bit(), 2 | (1 << 8) | (1 << 16))];
    const GOVERNED_WORKLOAD: AdmittedWorkload<1> =
        match admit(GOVERNED_CONTRACTS, AdmissionProfile::new(1024, 1024, 0, 1)) {
            Ok(value) => value,
            Err(_) => panic!("governed fixture must admit"),
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
        assert!(matches!(
            NanoKernel::new(&WORKLOAD, 0)
                .unwrap()
                .with_stack_guards(StackGuardTable::<0>::new()),
            Err(NanoError::MissingStackGuard)
        ));
        assert_eq!(NanoSubsystemReport::GUARDED.stack_guard, SUBSYSTEM_PRESENT);
        assert_eq!(NanoSubsystemReport::GUARDED.recovery, SUBSYSTEM_ABSENT);
    }

    #[test]
    fn adding_guards_preserves_dispatch_state_without_revalidation() {
        let mut region = [0u8; 64];
        let mut guards = StackGuardTable::<1>::new();
        unsafe {
            guards
                .register_shared_msp(crate::StackRegion {
                    base: region.as_mut_ptr() as usize,
                    len: region.len(),
                    canary_bytes: 8,
                })
                .unwrap();
        }

        let mut nano = NanoKernel::new(&WORKLOAD, 100).unwrap();
        nano.release_due(100);
        nano.mark_ready(2).unwrap();
        let mut guarded = nano.with_stack_guards(guards).unwrap();

        assert_eq!(guarded.dispatch_mut().take_next(), Some(0));
        assert_eq!(guarded.dispatch_mut().take_next(), Some(2));
        assert_eq!(guarded.dispatch().next_release_us(100), Some(105));
        assert_eq!(guarded.sweep_stacks(), None);
        assert_eq!(guarded.subsystem_report().stack_guard, SUBSYSTEM_PRESENT);
    }

    #[test]
    fn governance_uses_admitted_bindings_and_fails_closed() {
        let nano = NanoKernel::new(&GOVERNED_WORKLOAD, 0).unwrap();
        let mut governance = nano.governance();

        assert_eq!(governance.authorize(0, Capability::Mailbox), Ok(()));
        assert!(matches!(
            governance.authorize(0, Capability::Radio),
            Err(NanoGovernanceError::CapabilityDenied {
                task_index: 0,
                capability: Capability::Radio
            })
        ));
        assert_eq!(
            governance.authorize(1, Capability::Mailbox),
            Err(NanoGovernanceError::InvalidTask(1))
        );

        assert_eq!(governance.charge(0, ObjectKind::MailboxSlot), Ok(()));
        assert_eq!(governance.charge(0, ObjectKind::MailboxSlot), Ok(()));
        assert!(matches!(
            governance.charge(0, ObjectKind::MailboxSlot),
            Err(NanoGovernanceError::QuotaExceeded {
                task_index: 0,
                kind: ObjectKind::MailboxSlot,
                limit: 2
            })
        ));
        assert_eq!(
            governance.usage(0),
            Some(NanoObjectUsage {
                mailbox_slots: 2,
                alarms: 0,
                kv_entries: 0,
            })
        );
        assert_eq!(
            governance.clear_task(0),
            Ok(NanoObjectUsage {
                mailbox_slots: 2,
                alarms: 0,
                kv_entries: 0,
            })
        );
        assert!(matches!(
            governance.release(0, ObjectKind::MailboxSlot),
            Err(NanoGovernanceError::QuotaUnderflow {
                task_index: 0,
                kind: ObjectKind::MailboxSlot
            })
        ));
    }

    #[test]
    fn independently_selected_service_reports_compose() {
        let report = NanoSubsystemReport::GUARDED
            .union(NanoSubsystemReport::GOVERNED)
            .union(NanoSubsystemReport::SUPERVISED);
        assert_eq!(report.stack_guard, SUBSYSTEM_PRESENT);
        assert_eq!(report.capability, SUBSYSTEM_PRESENT);
        assert_eq!(report.quota, SUBSYSTEM_PRESENT);
        assert_eq!(report.recovery, SUBSYSTEM_PRESENT);
        assert_eq!(report.health, SUBSYSTEM_PRESENT);
        assert_eq!(report.trace, SUBSYSTEM_ABSENT);
    }

    #[test]
    fn recovery_maps_tasks_and_restores_lifecycle_without_a_trace() {
        let nano = NanoKernel::new(&GOVERNED_WORKLOAD, 0).unwrap();
        let mut recovery = nano
            .recovery(
                FaultThresholds {
                    notify_after: 1,
                    reboot_after: 2,
                },
                10,
            )
            .unwrap();
        assert_eq!(recovery.state(), SystemState::Running);

        let first = recovery
            .record_error(0, KernelError::DeadlineMissed, 20)
            .unwrap();
        assert_eq!(first.task_index, 0);
        assert_eq!(first.task_id, 7);
        assert_eq!(first.action, Action::NotifyUserTask);
        assert_eq!(first.state, SystemState::Degraded);

        let second = recovery
            .record_error(0, KernelError::DeadlineMissed, 30)
            .unwrap();
        assert_eq!(second.action, Action::RebootModule);
        assert_eq!(second.state, SystemState::Recovering);
        assert_eq!(recovery.counters(0).unwrap().unwrap().consecutive_errors, 2);

        recovery.record_ok(0, 40).unwrap();
        recovery.restore_running(50).unwrap();
        assert_eq!(recovery.state(), SystemState::Running);
        assert_eq!(recovery.counters(0).unwrap().unwrap().consecutive_errors, 0);
        assert_eq!(recovery.subsystem_report(), NanoSubsystemReport::SUPERVISED);
        assert_eq!(
            recovery.record_error(1, KernelError::ModuleCrash, 60),
            Err(NanoRecoveryError::InvalidTask(1))
        );
    }

    #[test]
    fn recovery_rejects_invalid_thresholds_before_startup() {
        let nano = NanoKernel::new(&GOVERNED_WORKLOAD, 0).unwrap();
        assert!(matches!(
            nano.recovery(
                FaultThresholds {
                    notify_after: 0,
                    reboot_after: 1,
                },
                0
            ),
            Err(NanoRecoveryError::InvalidThresholds(
                FaultThresholdError::NotifyZero
            ))
        ));
    }

    #[test]
    fn in_place_recovery_matches_the_value_constructor() {
        let nano = NanoKernel::new(&GOVERNED_WORKLOAD, 0).unwrap();
        let thresholds = FaultThresholds {
            notify_after: 1,
            reboot_after: 2,
        };
        let mut storage = core::mem::MaybeUninit::uninit();
        let in_place = nano.recovery_into(&mut storage, thresholds, 10).unwrap();
        let mut by_value = nano.recovery(thresholds, 10).unwrap();

        assert_eq!(
            in_place
                .record_error(0, KernelError::DeadlineMissed, 20)
                .unwrap(),
            by_value
                .record_error(0, KernelError::DeadlineMissed, 20)
                .unwrap()
        );
        assert_eq!(in_place.counters(0), by_value.counters(0));
        assert_eq!(in_place.state(), by_value.state());
    }

    #[test]
    fn retained_recovery_trace_is_an_independent_capacity_choice() {
        let nano = NanoKernel::new(&GOVERNED_WORKLOAD, 0).unwrap();
        let mut storage = core::mem::MaybeUninit::uninit();
        let recovery = nano
            .recovery_with_trace_into::<8>(
                &mut storage,
                FaultThresholds {
                    notify_after: 1,
                    reboot_after: 2,
                },
                10,
            )
            .unwrap();
        assert_eq!(recovery.events().len(), 3);
        assert_eq!(recovery.subsystem_report().trace, SUBSYSTEM_PRESENT);

        let planned = recovery
            .record_error_with_plan::<1>(
                0,
                KernelError::DeadlineMissed,
                20,
                RecoveryPlanPolicy::DEFAULT,
            )
            .unwrap();
        assert_eq!(planned.plan.len, 1);
        let mut recent = [crate::EventRecord::new(
            0,
            ModuleId::Kernel,
            crate::EventSeverity::Trace,
            crate::EventKind::Boot,
            crate::EventPayload::None,
        ); 8];
        let copied = recovery.events().copy_recent(&mut recent);
        assert_eq!(copied, 6);
        assert!(recent[..copied]
            .iter()
            .any(|event| event.kind == crate::EventKind::Health));
        assert!(recent[..copied]
            .iter()
            .any(|event| event.kind == crate::EventKind::Recovery));
    }

    #[test]
    fn dependency_plans_use_task_indices_and_bounded_transitive_order() {
        let nano = NanoKernel::new(&WORKLOAD, 0).unwrap();
        let mut dependencies = nano.recovery_dependencies();
        dependencies
            .depends_on(1, 0)
            .unwrap()
            .depends_on(2, 1)
            .unwrap();
        let mut recovery = nano
            .recovery(
                FaultThresholds {
                    notify_after: 1,
                    reboot_after: 2,
                },
                10,
            )
            .unwrap();
        recovery
            .record_error(0, KernelError::ModuleCrash, 20)
            .unwrap();
        let planned = recovery
            .record_error_with_dependencies::<8>(
                0,
                KernelError::ModuleCrash,
                200_000,
                RecoveryPlanPolicy::DEFAULT,
                &dependencies,
            )
            .unwrap();

        assert_eq!(planned.outcome.task_index, 0);
        assert_eq!(planned.outcome.action, Action::RebootModule);
        assert_eq!(planned.plan.len, 8);
        let expected = [
            (ModuleId::App(2), crate::RecoveryStepKind::QuiesceModule),
            (ModuleId::App(1), crate::RecoveryStepKind::QuiesceModule),
            (ModuleId::App(0), crate::RecoveryStepKind::QuiesceModule),
            (ModuleId::App(0), crate::RecoveryStepKind::RestartModule),
            (ModuleId::App(0), crate::RecoveryStepKind::VerifyHeartbeat),
            (ModuleId::App(0), crate::RecoveryStepKind::ResumeModule),
            (ModuleId::App(1), crate::RecoveryStepKind::ResumeModule),
            (ModuleId::App(2), crate::RecoveryStepKind::ResumeModule),
        ];
        for (step, expected) in planned.plan.steps.iter().flatten().zip(expected) {
            assert_eq!((step.module, step.kind), expected);
        }
    }

    #[test]
    fn invalid_dependency_graph_fails_before_health_state_changes() {
        let nano = NanoKernel::new(&WORKLOAD, 0).unwrap();
        let mut dependencies = nano.recovery_dependencies();
        dependencies.depends_on(1, 0).unwrap();
        assert!(matches!(
            dependencies.depends_on(1, 0),
            Err(NanoRecoveryError::DuplicateDependency {
                task_index: 1,
                dependency_index: 0,
            })
        ));
        dependencies.depends_on(0, 1).unwrap();
        let mut recovery = nano.recovery(FaultThresholds::DEFAULT, 10).unwrap();

        assert_eq!(
            recovery.record_error_with_dependencies::<8>(
                0,
                KernelError::ModuleCrash,
                20,
                RecoveryPlanPolicy::DEFAULT,
                &dependencies,
            ),
            Err(NanoRecoveryError::DependencyCycle)
        );
        assert_eq!(recovery.state(), SystemState::Running);
        assert_eq!(recovery.counters(0), Ok(None));
    }
}
