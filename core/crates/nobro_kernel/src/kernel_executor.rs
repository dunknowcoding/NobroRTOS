//! The authoritative execution loop (ARC-01 closure).
//!
//! `KernelExecutor` owns the assembled `Runtime` and a `TaskTable`; nothing else
//! can drive module execution while it runs. One `run_cycle` is one closed path:
//!
//! 1. watchdog edge sweep → recovery records
//! 2. due-alarm dispatch with recovery on backpressure
//! 3. task selection (highest criticality, then earliest phase-anchored release)
//! 4. module run-state check — suspended/recovering/disabled modules have their
//!    release skipped *and counted*, never silently polled
//! 5. the poll body runs inside a dispatcher-owned [`ModuleCtx`], so every
//!    protected operation is authorized, charged, and traced
//! 6. measured execution time feeds the CPU ledger and the task statistics;
//!    a budget overrun records a health fault (recovery thresholds escalate)
//!    and, under the bounded containment profile, disables the module after a
//!    configured number of overruns
//! 7. the outcome names the next due time so the idle decision (sleep/power)
//!    has an authoritative input
//!
//! Admission is fail-closed: the executor refuses to run until the registered
//! task set passes a response-time analysis (fixed priority by criticality,
//! pessimistic same-priority interference) plus the utilization bound — a set
//! whose declared worst-case costs cannot be scheduled never executes.
//!
//! A lock-free [`ExecutionSentinel`] marks the in-flight poll and its deadline
//! so an interrupt (deadline tick, hardware watchdog) can detect a non-yielding
//! module in real time — the bounded containment answer for cooperative
//! execution; preemption is intentionally out of scope for this profile.

use portable_atomic::{AtomicU32, Ordering};

use crate::{
    module_code, KernelError, ModuleCtx, ModuleId, ModuleRunState, Poll, Runtime, RuntimeError,
    TaskMeta, TaskTable, TaskTableError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainmentPolicy {
    /// Smallest profile: overruns are measured, recorded, and escalate through
    /// health thresholds only.
    Cooperative,
    /// Bounded profile for blocking/untrusted work: after `disable_after_overruns`
    /// measured budget overruns the module is disabled and cleaned up.
    Bounded { disable_after_overruns: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecError {
    Task(TaskTableError),
    /// The task set failed response-time or utilization admission.
    Unschedulable {
        module: ModuleId,
        response_us: u64,
        period_us: u32,
    },
    /// `run_cycle` was called before the task set was sealed.
    NotSealed,
    /// A task was registered after sealing.
    Sealed,
    Runtime(RuntimeError),
}

impl From<TaskTableError> for ExecError {
    fn from(error: TaskTableError) -> Self {
        Self::Task(error)
    }
}

impl From<RuntimeError> for ExecError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

/// Lock-free marker of the in-flight poll: an ISR calls [`check`](Self::check)
/// to detect a module running past its declared budget while the cooperative
/// loop cannot regain control.
pub struct ExecutionSentinel {
    /// `module_code` of the polling module; 0 = idle.
    module: AtomicU32,
    /// Absolute time the in-flight poll must have yielded by, split into two
    /// halves so Cortex-M targets without native 64-bit atomics stay lock-free.
    /// The halves only change while `module == 0`, and `module` is the
    /// release/acquire gate, so a reader that sees a nonzero module sees a
    /// consistent deadline.
    deadline_lo: AtomicU32,
    deadline_hi: AtomicU32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StuckPoll {
    pub module_code: u32,
    pub deadline_us: u64,
    pub late_us: u64,
}

impl ExecutionSentinel {
    pub const fn new() -> Self {
        Self {
            module: AtomicU32::new(0),
            deadline_lo: AtomicU32::new(0),
            deadline_hi: AtomicU32::new(0),
        }
    }

    fn arm(&self, module: ModuleId, deadline_us: u64) {
        self.deadline_lo
            .store(deadline_us as u32, Ordering::Relaxed);
        self.deadline_hi
            .store((deadline_us >> 32) as u32, Ordering::Release);
        self.module.store(module_code(module), Ordering::Release);
    }

    fn disarm(&self) {
        self.module.store(0, Ordering::Release);
    }

    /// ISR-safe: returns the in-flight poll iff it has outlived its budget.
    pub fn check(&self, now_us: u64) -> Option<StuckPoll> {
        let module_code = self.module.load(Ordering::Acquire);
        if module_code == 0 {
            return None;
        }
        let deadline_us = u64::from(self.deadline_lo.load(Ordering::Relaxed))
            | (u64::from(self.deadline_hi.load(Ordering::Relaxed)) << 32);
        if now_us <= deadline_us {
            return None;
        }
        Some(StuckPoll {
            module_code,
            deadline_us,
            late_us: now_us - deadline_us,
        })
    }
}

impl Default for ExecutionSentinel {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CycleOutcome {
    pub polled: Option<ModuleId>,
    pub poll: Option<Poll>,
    pub duration_us: u32,
    pub overrun: bool,
    /// The bounded containment profile disabled the module this cycle.
    pub contained: bool,
    pub skipped_release: Option<ModuleId>,
    pub alarms_dispatched: usize,
    pub watchdog_recoveries: usize,
    /// Nothing runnable before this time — the authoritative idle/sleep input.
    pub idle_until_us: Option<u64>,
}

pub struct KernelExecutor<
    const TASKS: usize,
    const STARTUP: usize,
    const QUOTAS: usize,
    const MAILBOX: usize,
    const ALARMS: usize,
    const KV: usize,
    const HEALTH: usize,
    const LOG: usize,
> {
    runtime: Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
    tasks: TaskTable<TASKS>,
    containment: ContainmentPolicy,
    sentinel: ExecutionSentinel,
    sealed: bool,
}

impl<
        const TASKS: usize,
        const STARTUP: usize,
        const QUOTAS: usize,
        const MAILBOX: usize,
        const ALARMS: usize,
        const KV: usize,
        const HEALTH: usize,
        const LOG: usize,
    > KernelExecutor<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>
{
    /// Take ownership of the runtime: from here on, execution has one driver.
    pub fn new(
        runtime: Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        containment: ContainmentPolicy,
    ) -> Self {
        Self {
            runtime,
            tasks: TaskTable::new(),
            containment,
            sentinel: ExecutionSentinel::new(),
            sealed: false,
        }
    }

    pub fn add_task(&mut self, meta: TaskMeta, now_us: u64) -> Result<(), ExecError> {
        if self.sealed {
            return Err(ExecError::Sealed);
        }
        self.runtime.ensure_module_enabled(meta.module)?;
        self.tasks.add(meta, now_us)?;
        Ok(())
    }

    /// Fail-closed schedulability admission: response-time analysis over the
    /// registered set (fixed priority = criticality, same-priority interference
    /// counted pessimistically, deadline = period) plus the utilization bound.
    /// `run_cycle` refuses to execute until this has passed.
    pub fn seal(&mut self) -> Result<(), ExecError> {
        let metas = self.tasks.metas();
        for meta in metas.iter().flatten() {
            let response_us = response_time(*meta, &metas)?;
            if response_us > u64::from(meta.period_us) {
                return Err(ExecError::Unschedulable {
                    module: meta.module,
                    response_us,
                    period_us: meta.period_us,
                });
            }
        }
        self.sealed = true;
        Ok(())
    }

    pub const fn sentinel(&self) -> &ExecutionSentinel {
        &self.sentinel
    }

    pub const fn runtime(&self) -> &Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG> {
        &self.runtime
    }

    pub const fn tasks(&self) -> &TaskTable<TASKS> {
        &self.tasks
    }

    /// Trusted-dispatcher escape hatch for setup that must happen between
    /// cycles (never hand this to module code).
    pub fn runtime_mut(
        &mut self,
    ) -> &mut Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG> {
        &mut self.runtime
    }

    /// One bounded cycle of the authoritative loop. `clock` supplies monotonic
    /// microseconds; `dispatch` is the application's module body, receiving a
    /// context fixed to the selected module's identity.
    pub fn run_cycle(
        &mut self,
        clock: impl Fn() -> u64,
        mut dispatch: impl FnMut(
            &mut ModuleCtx<'_, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        ) -> Result<Poll, KernelError>,
    ) -> Result<CycleOutcome, ExecError> {
        if !self.sealed {
            return Err(ExecError::NotSealed);
        }
        let now_us = clock();
        let mut outcome = CycleOutcome::default();

        let sweep = self.runtime.sweep_watchdogs(now_us)?;
        outcome.watchdog_recoveries = sweep.len;

        let alarms = self.runtime.dispatch_due_alarms_with_recovery(now_us)?;
        outcome.alarms_dispatched = alarms.dispatched;

        let Some(idx) = self.tasks.due_index(now_us) else {
            outcome.idle_until_us = self.next_activity_us();
            return Ok(outcome);
        };
        let meta = self.tasks.meta_at(idx).expect("due task has a slot");

        if self.runtime.module_state(meta.module) != Some(ModuleRunState::Active) {
            // Not runnable: the release is skipped and counted, never executed.
            self.tasks.skip_release(idx, now_us);
            outcome.skipped_release = Some(meta.module);
            outcome.idle_until_us = self.next_activity_us();
            return Ok(outcome);
        }

        let start_us = clock();
        self.sentinel.arm(
            meta.module,
            start_us.saturating_add(u64::from(meta.budget_us)),
        );
        let poll = self
            .runtime
            .with_module(meta.module, start_us, &mut dispatch)?;
        self.sentinel.disarm();
        let end_us = clock();
        let duration_us = end_us.saturating_sub(start_us).min(u64::from(u32::MAX)) as u32;

        // Accounting is unconditional: measured time reaches the CPU ledger and
        // the task statistics whether the poll succeeded or faulted.
        self.runtime.charge_cpu(meta.module, duration_us);
        let stats = self
            .tasks
            .record_poll(
                idx,
                end_us,
                duration_us,
                *poll.as_ref().unwrap_or(&Poll::Pending),
            )
            .expect("polled task has a slot");

        outcome.polled = Some(meta.module);
        outcome.duration_us = duration_us;

        match poll {
            Ok(result) => {
                outcome.poll = Some(result);
                if self.runtime.watchdog_entry(meta.module).is_some() {
                    self.runtime.heartbeat(meta.module, end_us)?;
                } else {
                    self.runtime.record_ok(meta.module, end_us)?;
                }
            }
            Err(error) => {
                let _ = self.runtime.record_error(meta.module, error, end_us)?;
            }
        }

        if duration_us > meta.budget_us {
            outcome.overrun = true;
            match self.containment {
                // Cooperative: the overrun is a deadline-contract violation and
                // enters the health pipeline (default policy faults the module,
                // handing it to recovery).
                ContainmentPolicy::Cooperative => {
                    let _ = self.runtime.record_error(
                        meta.module,
                        KernelError::DeadlineMissed,
                        end_us,
                    )?;
                }
                // Bounded: persistent overrunners are disabled outright after
                // the configured count — containment, not recovery.
                ContainmentPolicy::Bounded {
                    disable_after_overruns,
                } => {
                    if stats.overruns >= disable_after_overruns {
                        self.runtime.disable_module(meta.module, end_us)?;
                        outcome.contained = true;
                    }
                }
            }
        }

        outcome.idle_until_us = self.next_activity_us();
        Ok(outcome)
    }

    fn next_activity_us(&self) -> Option<u64> {
        let task = self.tasks.next_due_us();
        let alarm = self.runtime.alarms().next_due_us();
        match (task, alarm) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// Iterative response-time analysis for one task against the whole set.
/// Priority: higher criticality preempts; equal criticality is counted as
/// interference both ways (pessimistic, since the selector breaks ties by
/// release time, not a fixed order).
fn response_time<const N: usize>(
    task: TaskMeta,
    metas: &[Option<TaskMeta>; N],
) -> Result<u64, ExecError> {
    let cost = u64::from(task.budget_us);
    let mut response = cost;
    // The response never needs more iterations than it has microseconds below
    // the period bound; cap defensively to stay bounded on pathological input.
    for _ in 0..64 {
        let mut next = cost;
        for other in metas.iter().flatten() {
            if other.module == task.module || other.criticality < task.criticality {
                continue;
            }
            let releases = response.div_ceil(u64::from(other.period_us));
            next = next.saturating_add(releases.saturating_mul(u64::from(other.budget_us)));
        }
        if next == response {
            return Ok(response);
        }
        if next > u64::from(task.period_us) {
            return Err(ExecError::Unschedulable {
                module: task.module,
                response_us: next,
                period_us: task.period_us,
            });
        }
        response = next;
    }
    Err(ExecError::Unschedulable {
        module: task.module,
        response_us: response,
        period_us: task.period_us,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        kernel_module_spec, Capability, CapabilitySet, Criticality, DeadlineContract,
        DependencySet, FaultThresholds, MemoryBudget, MessageKind, ModuleSpec, StartupNode,
        SystemManifest, SystemProfile,
    };
    use core::cell::Cell;

    type TestRuntime = Runtime<4, 4, 8, 4, 8, 4, 32>;
    type TestExecutor = KernelExecutor<4, 4, 4, 8, 4, 8, 4, 32>;

    fn runtime() -> TestRuntime {
        let mut manifest = SystemManifest::<3>::new();
        manifest
            .add(kernel_module_spec(
                MemoryBudget::new(4096, 1024, 1),
                DeadlineContract::new(1000, 10),
            ))
            .unwrap();
        manifest
            .add(
                ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
                    .requires(CapabilitySet::empty().with(Capability::Mailbox))
                    .memory(MemoryBudget::new(1024, 256, 2)),
            )
            .unwrap();
        manifest
            .add(
                ModuleSpec::new(ModuleId::Actuator, Criticality::System)
                    .owns(CapabilitySet::empty().with(Capability::Mailbox))
                    .memory(MemoryBudget::new(1024, 256, 0)),
            )
            .unwrap();
        let nodes = [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty()),
            StartupNode::new(ModuleId::Sensor, DependencySet::empty()),
            StartupNode::new(ModuleId::Actuator, DependencySet::empty()),
        ];
        let mut runtime = TestRuntime::admit(
            &manifest,
            &nodes,
            SystemProfile::NRF52840_CORE,
            FaultThresholds::DEFAULT,
        )
        .unwrap();
        runtime.boot_to_running(0).unwrap();
        runtime
    }

    #[test]
    fn executor_refuses_to_run_unsealed_and_rejects_unschedulable_sets() {
        let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1000, 600),
            0,
        )
        .unwrap();
        assert_eq!(
            exec.run_cycle(|| 0, |_| Ok(Poll::Ready)).err(),
            Some(ExecError::NotSealed)
        );

        // A second task pushes the set over the schedulability bound:
        // actuator (System) interferes 700us per 1000us; sensor response
        // 600 + 700 = 1300 > 1000.
        exec.add_task(
            TaskMeta::new(ModuleId::Actuator, Criticality::System, 1000, 700),
            0,
        )
        .unwrap();
        assert!(matches!(
            exec.seal(),
            Err(ExecError::Unschedulable {
                module: ModuleId::Sensor,
                ..
            })
        ));
    }

    #[test]
    fn one_cycle_selects_measures_charges_and_reports_idle() {
        let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 10_000, 2_000),
            0,
        )
        .unwrap();
        exec.add_task(
            TaskMeta::new(ModuleId::Actuator, Criticality::System, 20_000, 2_000),
            0,
        )
        .unwrap();
        exec.seal().unwrap();

        // Deterministic clock: entry, poll start, poll end, ...
        let ticks = Cell::new(0u64);
        let clock = || {
            let t = ticks.get();
            ticks.set(t + 500);
            t
        };
        let outcome = exec
            .run_cycle(clock, |ctx| {
                // Higher criticality wins the tie: actuator runs first and can
                // use its granted mailbox capability through the context.
                assert_eq!(ctx.module(), ModuleId::Actuator);
                ctx.send(ModuleId::Sensor, MessageKind::Command, 7, 0)
                    .map_err(|_| KernelError::SensorReadFail)?;
                Ok(Poll::Ready)
            })
            .unwrap();

        assert_eq!(outcome.polled, Some(ModuleId::Actuator));
        assert_eq!(outcome.poll, Some(Poll::Ready));
        assert_eq!(outcome.duration_us, 500);
        assert!(!outcome.overrun);
        // Measured time reached the CPU ledger.
        assert_eq!(
            exec.runtime()
                .object_usage(ModuleId::Actuator)
                .unwrap()
                .cpu_us,
            500
        );
        // The sensor task is still due: the loop names the next activity.
        assert_eq!(outcome.idle_until_us, Some(0));
    }

    #[test]
    fn bounded_containment_disables_a_persistent_overrunner() {
        let mut exec = TestExecutor::new(
            runtime(),
            ContainmentPolicy::Bounded {
                disable_after_overruns: 2,
            },
        );
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 100_000, 100),
            0,
        )
        .unwrap();
        exec.seal().unwrap();

        // Each poll takes 1000us against a 100us budget.
        let ticks = Cell::new(0u64);
        let clock = || {
            let t = ticks.get();
            ticks.set(t + 1_000);
            t
        };
        let first = exec.run_cycle(&clock, |_| Ok(Poll::Pending)).unwrap();
        assert!(first.overrun && !first.contained);
        // Force the next release to be due immediately.
        ticks.set(200_000);
        let second = exec.run_cycle(&clock, |_| Ok(Poll::Pending)).unwrap();
        assert!(second.overrun && second.contained);
        assert_eq!(
            exec.runtime().module_state(ModuleId::Sensor),
            Some(ModuleRunState::Disabled)
        );

        // The disabled module is never polled again: its releases are skipped
        // and counted.
        ticks.set(400_000);
        let third = exec.run_cycle(&clock, |_| panic!("must not poll")).unwrap();
        assert_eq!(third.skipped_release, Some(ModuleId::Sensor));
        assert_eq!(third.polled, None);
    }

    #[test]
    fn sentinel_flags_a_non_yielding_poll_for_the_isr() {
        let sentinel = ExecutionSentinel::new();
        assert_eq!(sentinel.check(1_000), None);
        sentinel.arm(ModuleId::Radio, 5_000);
        assert_eq!(sentinel.check(4_999), None);
        let stuck = sentinel.check(6_000).expect("stuck poll");
        assert_eq!(stuck.module_code, module_code(ModuleId::Radio));
        assert_eq!(stuck.late_us, 1_000);
        sentinel.disarm();
        assert_eq!(sentinel.check(10_000), None);
    }

    #[test]
    fn schedulable_set_passes_rta_with_interference() {
        // sensor: C=100 T=1000 (Driver); actuator: C=200 T=2000 (System).
        // sensor response = 100 + ceil(R/2000)*200 -> 300 <= 1000: schedulable.
        let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1000, 100),
            0,
        )
        .unwrap();
        exec.add_task(
            TaskMeta::new(ModuleId::Actuator, Criticality::System, 2000, 200),
            0,
        )
        .unwrap();
        exec.seal().unwrap();
        assert_eq!(
            exec.add_task(
                TaskMeta::new(ModuleId::Kernel, Criticality::HardRealtime, 1000, 1),
                0
            ),
            Err(ExecError::Sealed)
        );
    }
}
