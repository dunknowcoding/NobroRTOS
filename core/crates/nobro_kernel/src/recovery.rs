//! Recovery coordinator for health, lifecycle, and watchdog-triggered faults.

use crate::{
    Action, DependencyImpact, EventLog, FaultThresholds, KernelError, Lifecycle, LifecycleError,
    ModuleId, Supervisor, SupervisorSnapshot, SystemState,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryOutcome {
    pub module: ModuleId,
    pub error: KernelError,
    pub action: Action,
    pub state: SystemState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryStepKind {
    Observe,
    Notify,
    Retry,
    QuiesceModule,
    RestartModule,
    VerifyHeartbeat,
    ResumeModule,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryStep {
    pub module: ModuleId,
    pub kind: RecoveryStepKind,
    pub due_us: u64,
    pub budget_us: u32,
}

impl RecoveryStep {
    pub const fn new(
        module: ModuleId,
        kind: RecoveryStepKind,
        due_us: u64,
        budget_us: u32,
    ) -> Self {
        Self {
            module,
            kind,
            due_us,
            budget_us,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryPlanPolicy {
    pub notify_budget_us: u32,
    pub retry_budget_us: u32,
    pub restart_budget_us: u32,
    pub verify_budget_us: u32,
    pub resume_budget_us: u32,
    pub max_total_budget_us: u32,
}

impl RecoveryPlanPolicy {
    pub const DEFAULT: Self = Self {
        notify_budget_us: 500,
        retry_budget_us: 1_000,
        restart_budget_us: 5_000,
        verify_budget_us: 1_000,
        resume_budget_us: 500,
        max_total_budget_us: 20_000,
    };
}

impl Default for RecoveryPlanPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryPlanError {
    Full,
    BudgetExceeded { required_us: u64, limit_us: u32 },
    ImpactRootMismatch { outcome: ModuleId, impact: ModuleId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryPlan<const N: usize> {
    pub outcome: RecoveryOutcome,
    pub steps: [Option<RecoveryStep>; N],
    pub len: usize,
    pub deadline_us: u64,
    pub required_budget_us: u64,
}

impl<const N: usize> RecoveryPlan<N> {
    pub fn from_outcome(
        outcome: RecoveryOutcome,
        now_us: u64,
        policy: RecoveryPlanPolicy,
    ) -> Result<Self, RecoveryPlanError> {
        let mut plan = Self {
            outcome,
            steps: [None; N],
            len: 0,
            deadline_us: now_us,
            required_budget_us: 0,
        };

        match outcome.action {
            Action::Ignore => {
                plan.push(RecoveryStep::new(
                    outcome.module,
                    RecoveryStepKind::Observe,
                    now_us,
                    0,
                ))?;
            }
            Action::NotifyUserTask => {
                plan.push(RecoveryStep::new(
                    outcome.module,
                    RecoveryStepKind::Notify,
                    now_us,
                    policy.notify_budget_us,
                ))?;
            }
            Action::RetryNow => {
                plan.push(RecoveryStep::new(
                    outcome.module,
                    RecoveryStepKind::Retry,
                    now_us,
                    policy.retry_budget_us,
                ))?;
                plan.push(RecoveryStep::new(
                    outcome.module,
                    RecoveryStepKind::VerifyHeartbeat,
                    now_us.saturating_add(u64::from(policy.retry_budget_us)),
                    policy.verify_budget_us,
                ))?;
            }
            Action::RetryDelay(delay_us) => {
                let retry_due = now_us.saturating_add(u64::from(delay_us));
                plan.required_budget_us =
                    plan.required_budget_us.saturating_add(u64::from(delay_us));
                plan.push(RecoveryStep::new(
                    outcome.module,
                    RecoveryStepKind::Retry,
                    retry_due,
                    policy.retry_budget_us,
                ))?;
                plan.push(RecoveryStep::new(
                    outcome.module,
                    RecoveryStepKind::VerifyHeartbeat,
                    retry_due.saturating_add(u64::from(policy.retry_budget_us)),
                    policy.verify_budget_us,
                ))?;
            }
            Action::RebootModule => {
                plan.push(RecoveryStep::new(
                    outcome.module,
                    RecoveryStepKind::QuiesceModule,
                    now_us,
                    policy.notify_budget_us,
                ))?;
                plan.push(RecoveryStep::new(
                    outcome.module,
                    RecoveryStepKind::RestartModule,
                    now_us.saturating_add(u64::from(policy.notify_budget_us)),
                    policy.restart_budget_us,
                ))?;
                plan.push(RecoveryStep::new(
                    outcome.module,
                    RecoveryStepKind::VerifyHeartbeat,
                    now_us
                        .saturating_add(u64::from(policy.notify_budget_us))
                        .saturating_add(u64::from(policy.restart_budget_us)),
                    policy.verify_budget_us,
                ))?;
                plan.push(RecoveryStep::new(
                    outcome.module,
                    RecoveryStepKind::ResumeModule,
                    now_us
                        .saturating_add(u64::from(policy.notify_budget_us))
                        .saturating_add(u64::from(policy.restart_budget_us))
                        .saturating_add(u64::from(policy.verify_budget_us)),
                    policy.resume_budget_us,
                ))?;
            }
        }

        if plan.required_budget_us > u64::from(policy.max_total_budget_us) {
            return Err(RecoveryPlanError::BudgetExceeded {
                required_us: plan.required_budget_us,
                limit_us: policy.max_total_budget_us,
            });
        }
        plan.deadline_us = now_us.saturating_add(plan.required_budget_us);
        Ok(plan)
    }

    pub fn from_outcome_with_impact<const IMPACT: usize>(
        outcome: RecoveryOutcome,
        impact: &DependencyImpact<IMPACT>,
        now_us: u64,
        policy: RecoveryPlanPolicy,
    ) -> Result<Self, RecoveryPlanError> {
        if outcome.action != Action::RebootModule {
            return Self::from_outcome(outcome, now_us, policy);
        }
        if impact.root != outcome.module {
            return Err(RecoveryPlanError::ImpactRootMismatch {
                outcome: outcome.module,
                impact: impact.root,
            });
        }
        if impact.is_empty() {
            return Self::from_outcome(outcome, now_us, policy);
        }

        let mut plan = Self {
            outcome,
            steps: [None; N],
            len: 0,
            deadline_us: now_us,
            required_budget_us: 0,
        };
        let mut due_us = now_us;

        for module in impact.affected.iter().copied().take(impact.affected_count) {
            let Some(module) = module else {
                continue;
            };
            plan.push(RecoveryStep::new(
                module,
                RecoveryStepKind::QuiesceModule,
                due_us,
                policy.notify_budget_us,
            ))?;
            due_us = due_us.saturating_add(u64::from(policy.notify_budget_us));
        }

        plan.push(RecoveryStep::new(
            outcome.module,
            RecoveryStepKind::QuiesceModule,
            due_us,
            policy.notify_budget_us,
        ))?;
        due_us = due_us.saturating_add(u64::from(policy.notify_budget_us));
        plan.push(RecoveryStep::new(
            outcome.module,
            RecoveryStepKind::RestartModule,
            due_us,
            policy.restart_budget_us,
        ))?;
        due_us = due_us.saturating_add(u64::from(policy.restart_budget_us));
        plan.push(RecoveryStep::new(
            outcome.module,
            RecoveryStepKind::VerifyHeartbeat,
            due_us,
            policy.verify_budget_us,
        ))?;
        due_us = due_us.saturating_add(u64::from(policy.verify_budget_us));
        plan.push(RecoveryStep::new(
            outcome.module,
            RecoveryStepKind::ResumeModule,
            due_us,
            policy.resume_budget_us,
        ))?;
        due_us = due_us.saturating_add(u64::from(policy.resume_budget_us));

        for module in impact
            .affected
            .iter()
            .copied()
            .take(impact.affected_count)
            .rev()
        {
            let Some(module) = module else {
                continue;
            };
            plan.push(RecoveryStep::new(
                module,
                RecoveryStepKind::ResumeModule,
                due_us,
                policy.resume_budget_us,
            ))?;
            due_us = due_us.saturating_add(u64::from(policy.resume_budget_us));
        }

        if plan.required_budget_us > u64::from(policy.max_total_budget_us) {
            return Err(RecoveryPlanError::BudgetExceeded {
                required_us: plan.required_budget_us,
                limit_us: policy.max_total_budget_us,
            });
        }
        plan.deadline_us = now_us.saturating_add(plan.required_budget_us);
        Ok(plan)
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn first(&self) -> Option<RecoveryStep> {
        self.steps.first().copied().flatten()
    }

    pub fn last(&self) -> Option<RecoveryStep> {
        if self.len == 0 {
            None
        } else {
            self.steps[self.len - 1]
        }
    }

    pub fn due_count(&self, now_us: u64) -> usize {
        self.steps
            .iter()
            .copied()
            .take(self.len)
            .flatten()
            .filter(|step| step.due_us <= now_us)
            .count()
    }

    pub fn remaining_count(&self, now_us: u64) -> usize {
        self.len.saturating_sub(self.due_count(now_us))
    }

    pub fn next_due(&self, now_us: u64) -> Option<RecoveryStep> {
        #[allow(clippy::manual_find)] // clarity: early-return with side-conditions
        for step in self.steps.iter().copied().take(self.len).flatten() {
            if step.due_us <= now_us {
                return Some(step);
            }
        }
        None
    }

    pub fn copy_due(&self, now_us: u64, out: &mut [RecoveryStep]) -> usize {
        let mut copied = 0;
        for step in self.steps.iter().copied().take(self.len).flatten() {
            if step.due_us > now_us {
                continue;
            }
            if copied == out.len() {
                break;
            }
            out[copied] = step;
            copied += 1;
        }
        copied
    }

    fn push(&mut self, step: RecoveryStep) -> Result<(), RecoveryPlanError> {
        if self.len == N {
            return Err(RecoveryPlanError::Full);
        }
        self.required_budget_us = self
            .required_budget_us
            .saturating_add(u64::from(step.budget_us));
        self.steps[self.len] = Some(step);
        self.len += 1;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryPlanDispatch {
    pub dispatched: usize,
    pub remaining: usize,
    pub next_due_us: u64,
    pub consumed_budget_us: u64,
    pub overdue_us: u64,
    pub completed: bool,
}

impl RecoveryPlanDispatch {
    pub const fn is_blocked_by_output(&self) -> bool {
        self.overdue_us != 0 && self.remaining != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryPlanExecution<const N: usize> {
    plan: RecoveryPlan<N>,
    next_step: usize,
    consumed_budget_us: u64,
    last_dispatch_us: u64,
}

impl<const N: usize> RecoveryPlanExecution<N> {
    pub const fn from_plan(plan: RecoveryPlan<N>) -> Self {
        Self {
            plan,
            next_step: 0,
            consumed_budget_us: 0,
            last_dispatch_us: 0,
        }
    }

    pub const fn plan(&self) -> &RecoveryPlan<N> {
        &self.plan
    }

    pub const fn dispatched_count(&self) -> usize {
        self.next_step
    }

    pub const fn remaining_count(&self) -> usize {
        self.plan.len.saturating_sub(self.next_step)
    }

    pub const fn consumed_budget_us(&self) -> u64 {
        self.consumed_budget_us
    }

    pub const fn last_dispatch_us(&self) -> u64 {
        self.last_dispatch_us
    }

    pub const fn is_complete(&self) -> bool {
        self.next_step >= self.plan.len
    }

    pub fn next_pending(&self) -> Option<RecoveryStep> {
        if self.next_step >= self.plan.len {
            None
        } else {
            self.plan.steps[self.next_step]
        }
    }

    pub fn due_pending_count(&self, now_us: u64) -> usize {
        self.plan
            .steps
            .iter()
            .copied()
            .take(self.plan.len)
            .skip(self.next_step)
            .flatten()
            .filter(|step| step.due_us <= now_us)
            .count()
    }

    pub fn dispatch_due(&mut self, now_us: u64, out: &mut [RecoveryStep]) -> RecoveryPlanDispatch {
        let mut dispatched = 0;
        while self.next_step < self.plan.len && dispatched < out.len() {
            let Some(step) = self.plan.steps[self.next_step] else {
                self.next_step += 1;
                continue;
            };
            if step.due_us > now_us {
                break;
            }
            out[dispatched] = step;
            dispatched += 1;
            self.next_step += 1;
            self.consumed_budget_us = self
                .consumed_budget_us
                .saturating_add(u64::from(step.budget_us));
            self.last_dispatch_us = now_us;
        }

        let remaining = self.remaining_count();
        let next_due_us = self.next_pending().map(|step| step.due_us).unwrap_or(0);
        let overdue_us = if next_due_us != 0 && next_due_us < now_us {
            now_us.saturating_sub(next_due_us)
        } else {
            0
        };

        RecoveryPlanDispatch {
            dispatched,
            remaining,
            next_due_us,
            consumed_budget_us: self.consumed_budget_us,
            overdue_us,
            completed: remaining == 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryError {
    Lifecycle(LifecycleError),
}

pub struct RecoveryCoordinator<const HEALTH_SLOTS: usize, const LOG_SLOTS: usize> {
    supervisor: Supervisor<HEALTH_SLOTS, LOG_SLOTS>,
    lifecycle: Lifecycle,
}

impl<const HEALTH_SLOTS: usize, const LOG_SLOTS: usize>
    RecoveryCoordinator<HEALTH_SLOTS, LOG_SLOTS>
{
    pub const fn new(thresholds: FaultThresholds) -> Self {
        Self {
            supervisor: Supervisor::with_default_policy(thresholds),
            lifecycle: Lifecycle::new(),
        }
    }

    pub fn transition(&mut self, to: SystemState, now_us: u64) -> Result<(), RecoveryError> {
        let event = self
            .lifecycle
            .transition(to, now_us)
            .map_err(RecoveryError::Lifecycle)?;
        self.supervisor.events_mut().push(event);
        Ok(())
    }

    pub fn record_ok(&mut self, module: ModuleId, now_us: u64) {
        self.supervisor.record_ok(module, now_us);
    }

    pub fn record_error(
        &mut self,
        module: ModuleId,
        error: KernelError,
        now_us: u64,
    ) -> Result<RecoveryOutcome, RecoveryError> {
        let action = self.supervisor.record_error(module, error, now_us);
        let event = self
            .lifecycle
            .apply_action(module, error, action, now_us)
            .map_err(RecoveryError::Lifecycle)?;
        self.supervisor.events_mut().push(event);

        Ok(RecoveryOutcome {
            module,
            error,
            action,
            state: self.lifecycle.state(),
        })
    }

    pub fn record_watchdog_expired(
        &mut self,
        module: ModuleId,
        now_us: u64,
    ) -> Result<RecoveryOutcome, RecoveryError> {
        self.record_error(module, KernelError::DeadlineMissed, now_us)
    }

    pub const fn state(&self) -> SystemState {
        self.lifecycle.state()
    }

    pub const fn lifecycle(&self) -> &Lifecycle {
        &self.lifecycle
    }

    pub fn snapshot(&self, module: ModuleId) -> Option<SupervisorSnapshot> {
        self.supervisor.snapshot(module)
    }

    pub fn events(&self) -> &EventLog<LOG_SLOTS> {
        self.supervisor.events()
    }
}

impl<const HEALTH_SLOTS: usize, const LOG_SLOTS: usize> Default
    for RecoveryCoordinator<HEALTH_SLOTS, LOG_SLOTS>
{
    fn default() -> Self {
        Self::new(FaultThresholds::DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EventKind, EventPayload, EventRecord, EventSeverity, StartupGraph};

    fn running_coordinator() -> RecoveryCoordinator<2, 12> {
        let mut recovery = RecoveryCoordinator::<2, 12>::new(FaultThresholds {
            notify_after: 1,
            reboot_after: 3,
        });
        recovery
            .transition(SystemState::ValidateManifest, 10)
            .unwrap();
        recovery.transition(SystemState::InitDrivers, 20).unwrap();
        recovery.transition(SystemState::Running, 30).unwrap();
        recovery
    }

    #[test]
    fn notify_action_moves_running_to_degraded() {
        let mut recovery = running_coordinator();

        let outcome = recovery
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 40)
            .unwrap();

        assert_eq!(outcome.action, Action::NotifyUserTask);
        assert_eq!(outcome.state, SystemState::Degraded);
        assert_eq!(recovery.state(), SystemState::Degraded);
        assert!(
            recovery
                .snapshot(ModuleId::Sensor)
                .expect("sensor snapshot")
                .log_len
                >= 6
        );
    }

    #[test]
    fn repeated_errors_move_to_recovering() {
        let mut recovery = running_coordinator();

        recovery
            .record_error(ModuleId::Radio, KernelError::RadioTxFail, 40)
            .unwrap();
        recovery
            .record_error(ModuleId::Radio, KernelError::RadioTxFail, 50)
            .unwrap();
        let outcome = recovery
            .record_error(ModuleId::Radio, KernelError::RadioTxFail, 60)
            .unwrap();

        assert_eq!(outcome.action, Action::RebootModule);
        assert_eq!(outcome.state, SystemState::Recovering);
    }

    #[test]
    fn watchdog_expiry_is_routed_as_deadline_fault() {
        let mut recovery = running_coordinator();

        let outcome = recovery
            .record_watchdog_expired(ModuleId::Actuator, 40)
            .unwrap();

        assert_eq!(outcome.error, KernelError::DeadlineMissed);
        assert_eq!(outcome.action, Action::NotifyUserTask);
        assert_eq!(outcome.state, SystemState::Degraded);
    }

    #[test]
    fn recovery_plan_notifies_without_heap_allocation() {
        let outcome = RecoveryOutcome {
            module: ModuleId::Sensor,
            error: KernelError::SensorReadFail,
            action: Action::NotifyUserTask,
            state: SystemState::Degraded,
        };

        let plan =
            RecoveryPlan::<2>::from_outcome(outcome, 100, RecoveryPlanPolicy::DEFAULT).unwrap();

        assert_eq!(plan.len, 1);
        assert_eq!(plan.required_budget_us, 500);
        assert_eq!(plan.deadline_us, 600);
        assert_eq!(
            plan.first(),
            Some(RecoveryStep::new(
                ModuleId::Sensor,
                RecoveryStepKind::Notify,
                100,
                500
            ))
        );
    }

    #[test]
    fn recovery_plan_delays_retry_and_verifies_heartbeat() {
        let outcome = RecoveryOutcome {
            module: ModuleId::Radio,
            error: KernelError::RadioTxFail,
            action: Action::RetryDelay(2_000),
            state: SystemState::Running,
        };

        let plan =
            RecoveryPlan::<2>::from_outcome(outcome, 10_000, RecoveryPlanPolicy::DEFAULT).unwrap();

        assert_eq!(plan.len, 2);
        assert_eq!(plan.required_budget_us, 4_000);
        assert_eq!(plan.deadline_us, 14_000);
        assert_eq!(
            plan.steps[0],
            Some(RecoveryStep::new(
                ModuleId::Radio,
                RecoveryStepKind::Retry,
                12_000,
                1_000
            ))
        );
        assert_eq!(
            plan.steps[1],
            Some(RecoveryStep::new(
                ModuleId::Radio,
                RecoveryStepKind::VerifyHeartbeat,
                13_000,
                1_000
            ))
        );
    }

    #[test]
    fn recovery_plan_reboot_sequence_is_ordered_and_bounded() {
        let outcome = RecoveryOutcome {
            module: ModuleId::Actuator,
            error: KernelError::DeadlineMissed,
            action: Action::RebootModule,
            state: SystemState::Recovering,
        };

        let plan =
            RecoveryPlan::<4>::from_outcome(outcome, 50, RecoveryPlanPolicy::DEFAULT).unwrap();

        assert_eq!(plan.len, 4);
        assert_eq!(plan.required_budget_us, 7_000);
        assert_eq!(plan.deadline_us, 7_050);
        assert_eq!(
            plan.steps[0],
            Some(RecoveryStep::new(
                ModuleId::Actuator,
                RecoveryStepKind::QuiesceModule,
                50,
                500
            ))
        );
        assert_eq!(
            plan.steps[1],
            Some(RecoveryStep::new(
                ModuleId::Actuator,
                RecoveryStepKind::RestartModule,
                550,
                5_000
            ))
        );
        assert_eq!(
            plan.steps[2],
            Some(RecoveryStep::new(
                ModuleId::Actuator,
                RecoveryStepKind::VerifyHeartbeat,
                5_550,
                1_000
            ))
        );
        assert_eq!(
            plan.last(),
            Some(RecoveryStep::new(
                ModuleId::Actuator,
                RecoveryStepKind::ResumeModule,
                6_550,
                500
            ))
        );
    }

    #[test]
    fn recovery_plan_uses_dependency_impact_for_quiesce_and_resume_order() {
        let mut graph = StartupGraph::<4>::from_modules(&[
            ModuleId::Kernel,
            ModuleId::Bus,
            ModuleId::Sensor,
            ModuleId::App(1),
        ])
        .unwrap();
        graph
            .add_dependency(ModuleId::Bus, ModuleId::Kernel)
            .unwrap();
        graph
            .add_dependency(ModuleId::Sensor, ModuleId::Bus)
            .unwrap();
        graph
            .add_dependency(ModuleId::App(1), ModuleId::Sensor)
            .unwrap();
        let impact = graph.dependency_impact::<2>(ModuleId::Bus).unwrap();
        let outcome = RecoveryOutcome {
            module: ModuleId::Bus,
            error: KernelError::BusTimeout,
            action: Action::RebootModule,
            state: SystemState::Recovering,
        };

        let plan = RecoveryPlan::<8>::from_outcome_with_impact(
            outcome,
            &impact,
            100,
            RecoveryPlanPolicy::DEFAULT,
        )
        .unwrap();

        assert_eq!(
            impact.affected,
            [Some(ModuleId::App(1)), Some(ModuleId::Sensor)]
        );
        assert_eq!(plan.len, 8);
        assert_eq!(plan.required_budget_us, 9_000);
        assert_eq!(plan.deadline_us, 9_100);
        assert_eq!(
            plan.steps[0],
            Some(RecoveryStep::new(
                ModuleId::App(1),
                RecoveryStepKind::QuiesceModule,
                100,
                500
            ))
        );
        assert_eq!(
            plan.steps[1],
            Some(RecoveryStep::new(
                ModuleId::Sensor,
                RecoveryStepKind::QuiesceModule,
                600,
                500
            ))
        );
        assert_eq!(
            plan.steps[2],
            Some(RecoveryStep::new(
                ModuleId::Bus,
                RecoveryStepKind::QuiesceModule,
                1_100,
                500
            ))
        );
        assert_eq!(
            plan.steps[5],
            Some(RecoveryStep::new(
                ModuleId::Bus,
                RecoveryStepKind::ResumeModule,
                7_600,
                500
            ))
        );
        assert_eq!(
            plan.steps[6],
            Some(RecoveryStep::new(
                ModuleId::Sensor,
                RecoveryStepKind::ResumeModule,
                8_100,
                500
            ))
        );
        assert_eq!(
            plan.steps[7],
            Some(RecoveryStep::new(
                ModuleId::App(1),
                RecoveryStepKind::ResumeModule,
                8_600,
                500
            ))
        );
    }

    #[test]
    fn recovery_plan_with_impact_reports_capacity_and_budget_failures() {
        let mut graph = StartupGraph::<3>::from_modules(&[
            ModuleId::Kernel,
            ModuleId::Sensor,
            ModuleId::App(1),
        ])
        .unwrap();
        graph
            .add_dependency(ModuleId::Sensor, ModuleId::Kernel)
            .unwrap();
        graph
            .add_dependency(ModuleId::App(1), ModuleId::Sensor)
            .unwrap();
        let impact = graph.dependency_impact::<1>(ModuleId::Sensor).unwrap();
        let outcome = RecoveryOutcome {
            module: ModuleId::Sensor,
            error: KernelError::DeadlineMissed,
            action: Action::RebootModule,
            state: SystemState::Recovering,
        };

        assert_eq!(
            RecoveryPlan::<4>::from_outcome_with_impact(
                outcome,
                &impact,
                0,
                RecoveryPlanPolicy::DEFAULT
            ),
            Err(RecoveryPlanError::Full)
        );

        let tight = RecoveryPlanPolicy {
            max_total_budget_us: 7_000,
            ..RecoveryPlanPolicy::DEFAULT
        };
        assert_eq!(
            RecoveryPlan::<6>::from_outcome_with_impact(outcome, &impact, 0, tight),
            Err(RecoveryPlanError::BudgetExceeded {
                required_us: 8_000,
                limit_us: 7_000,
            })
        );

        let wrong_root = DependencyImpact::<1>::new(ModuleId::Bus);
        assert_eq!(
            RecoveryPlan::<6>::from_outcome_with_impact(
                outcome,
                &wrong_root,
                0,
                RecoveryPlanPolicy::DEFAULT
            ),
            Err(RecoveryPlanError::ImpactRootMismatch {
                outcome: ModuleId::Sensor,
                impact: ModuleId::Bus,
            })
        );
    }

    #[test]
    fn recovery_plan_reports_due_steps_without_mutation() {
        let outcome = RecoveryOutcome {
            module: ModuleId::Actuator,
            error: KernelError::DeadlineMissed,
            action: Action::RebootModule,
            state: SystemState::Recovering,
        };
        let plan =
            RecoveryPlan::<4>::from_outcome(outcome, 100, RecoveryPlanPolicy::DEFAULT).unwrap();
        let empty = RecoveryStep::new(ModuleId::Kernel, RecoveryStepKind::Observe, 0, 0);
        let mut due = [empty; 2];

        assert_eq!(plan.next_due(99), None);
        assert_eq!(plan.due_count(99), 0);
        assert_eq!(plan.remaining_count(99), 4);
        assert_eq!(
            plan.next_due(600),
            Some(RecoveryStep::new(
                ModuleId::Actuator,
                RecoveryStepKind::QuiesceModule,
                100,
                500
            ))
        );
        assert_eq!(plan.due_count(6_100), 3);
        assert_eq!(plan.remaining_count(6_100), 1);
        assert_eq!(plan.copy_due(6_100, &mut due), 2);
        assert_eq!(
            due,
            [
                RecoveryStep::new(
                    ModuleId::Actuator,
                    RecoveryStepKind::QuiesceModule,
                    100,
                    500,
                ),
                RecoveryStep::new(
                    ModuleId::Actuator,
                    RecoveryStepKind::RestartModule,
                    600,
                    5_000,
                ),
            ]
        );
    }

    #[test]
    fn recovery_plan_execution_dispatches_due_steps_once() {
        let outcome = RecoveryOutcome {
            module: ModuleId::Actuator,
            error: KernelError::DeadlineMissed,
            action: Action::RebootModule,
            state: SystemState::Recovering,
        };
        let plan =
            RecoveryPlan::<4>::from_outcome(outcome, 100, RecoveryPlanPolicy::DEFAULT).unwrap();
        let empty = RecoveryStep::new(ModuleId::Kernel, RecoveryStepKind::Observe, 0, 0);
        let mut due = [empty; 2];
        let mut execution = RecoveryPlanExecution::from_plan(plan);

        assert_eq!(execution.due_pending_count(99), 0);
        assert_eq!(execution.dispatch_due(99, &mut due).dispatched, 0);
        assert_eq!(execution.dispatched_count(), 0);

        let dispatch = execution.dispatch_due(100, &mut due);
        assert_eq!(dispatch.dispatched, 1);
        assert_eq!(dispatch.remaining, 3);
        assert_eq!(dispatch.next_due_us, 600);
        assert_eq!(dispatch.consumed_budget_us, 500);
        assert!(!dispatch.completed);
        assert_eq!(
            due[0],
            RecoveryStep::new(
                ModuleId::Actuator,
                RecoveryStepKind::QuiesceModule,
                100,
                500
            )
        );
        assert_eq!(execution.dispatched_count(), 1);
        assert_eq!(execution.last_dispatch_us(), 100);

        let dispatch = execution.dispatch_due(6_100, &mut due);
        assert_eq!(dispatch.dispatched, 2);
        assert_eq!(dispatch.remaining, 1);
        assert_eq!(dispatch.next_due_us, 6_600);
        assert_eq!(dispatch.consumed_budget_us, 6_500);
        assert_eq!(
            execution.next_pending().map(|step| step.kind),
            Some(RecoveryStepKind::ResumeModule)
        );
    }

    #[test]
    fn recovery_plan_execution_preserves_overdue_steps_when_output_is_full() {
        let outcome = RecoveryOutcome {
            module: ModuleId::Sensor,
            error: KernelError::DeadlineMissed,
            action: Action::RebootModule,
            state: SystemState::Recovering,
        };
        let plan =
            RecoveryPlan::<4>::from_outcome(outcome, 10, RecoveryPlanPolicy::DEFAULT).unwrap();
        let empty = RecoveryStep::new(ModuleId::Kernel, RecoveryStepKind::Observe, 0, 0);
        let mut one = [empty; 1];
        let mut execution = RecoveryPlanExecution::from_plan(plan);

        let dispatch = execution.dispatch_due(10_000, &mut one);
        assert_eq!(dispatch.dispatched, 1);
        assert_eq!(dispatch.remaining, 3);
        assert_eq!(dispatch.next_due_us, 510);
        assert_eq!(dispatch.overdue_us, 9_490);
        assert!(dispatch.is_blocked_by_output());
        assert_eq!(
            execution.next_pending(),
            Some(RecoveryStep::new(
                ModuleId::Sensor,
                RecoveryStepKind::RestartModule,
                510,
                5_000
            ))
        );

        let dispatch = execution.dispatch_due(10_000, &mut one);
        assert_eq!(dispatch.dispatched, 1);
        assert_eq!(dispatch.remaining, 2);
        assert_eq!(dispatch.next_due_us, 5_510);
        assert_eq!(dispatch.consumed_budget_us, 5_500);
    }

    #[test]
    fn recovery_plan_execution_does_not_advance_with_empty_output() {
        let outcome = RecoveryOutcome {
            module: ModuleId::Radio,
            error: KernelError::RadioTxFail,
            action: Action::RetryDelay(1_000),
            state: SystemState::Running,
        };
        let plan =
            RecoveryPlan::<2>::from_outcome(outcome, 100, RecoveryPlanPolicy::DEFAULT).unwrap();
        let mut execution = RecoveryPlanExecution::from_plan(plan);

        let dispatch = execution.dispatch_due(2_000, &mut []);
        assert_eq!(dispatch.dispatched, 0);
        assert_eq!(dispatch.remaining, 2);
        assert_eq!(dispatch.next_due_us, 1_100);
        assert_eq!(dispatch.overdue_us, 900);
        assert_eq!(execution.dispatched_count(), 0);
        assert_eq!(execution.consumed_budget_us(), 0);
        assert_eq!(
            execution.next_pending(),
            Some(RecoveryStep::new(
                ModuleId::Radio,
                RecoveryStepKind::Retry,
                1_100,
                1_000
            ))
        );
    }

    #[test]
    fn recovery_plan_reports_capacity_and_budget_failures() {
        let outcome = RecoveryOutcome {
            module: ModuleId::Bus,
            error: KernelError::BusTimeout,
            action: Action::RebootModule,
            state: SystemState::Recovering,
        };

        assert_eq!(
            RecoveryPlan::<3>::from_outcome(outcome, 0, RecoveryPlanPolicy::DEFAULT),
            Err(RecoveryPlanError::Full)
        );

        let tight = RecoveryPlanPolicy {
            max_total_budget_us: 1_000,
            ..RecoveryPlanPolicy::DEFAULT
        };
        assert_eq!(
            RecoveryPlan::<4>::from_outcome(outcome, 0, tight),
            Err(RecoveryPlanError::BudgetExceeded {
                required_us: 7_000,
                limit_us: 1_000,
            })
        );
    }

    #[test]
    fn transition_events_are_written_to_log() {
        let recovery = running_coordinator();
        let empty = EventRecord::new(
            0,
            ModuleId::Kernel,
            EventSeverity::Trace,
            EventKind::Boot,
            EventPayload::None,
        );
        let mut recent = [empty; 3];

        assert_eq!(recovery.events().copy_recent(&mut recent), 3);
        assert_eq!(recent[0].payload, EventPayload::Pair(0, 1));
        assert_eq!(recent[1].payload, EventPayload::Pair(1, 2));
        assert_eq!(recent[2].payload, EventPayload::Pair(2, 3));
    }
}
