//! Fixed-plan hot module reload support.

use crate::{ModuleId, SystemBudget};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotReloadStepKind {
    QuiesceModule,
    ReleaseLeases,
    UnmountModule,
    MountModule,
    ResumeModule,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HotReloadStep {
    pub module: ModuleId,
    pub kind: HotReloadStepKind,
    pub due_us: u64,
    pub budget_us: u32,
    pub revision: u32,
}

impl HotReloadStep {
    pub const fn new(
        module: ModuleId,
        kind: HotReloadStepKind,
        due_us: u64,
        budget_us: u32,
        revision: u32,
    ) -> Self {
        Self {
            module,
            kind,
            due_us,
            budget_us,
            revision,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HotReloadPolicy {
    pub quiesce_budget_us: u32,
    pub lease_release_budget_us: u32,
    pub unmount_budget_us: u32,
    pub mount_budget_us: u32,
    pub resume_budget_us: u32,
    pub max_total_budget_us: u32,
}

impl HotReloadPolicy {
    pub const DEFAULT: Self = Self {
        quiesce_budget_us: 500,
        lease_release_budget_us: 250,
        unmount_budget_us: 500,
        mount_budget_us: 1_000,
        resume_budget_us: 500,
        max_total_budget_us: 4_000,
    };
}

impl Default for HotReloadPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotReloadError {
    CannotReloadKernel,
    PlanFull,
    BudgetExceeded { required_us: u64, limit_us: u32 },
}

pub trait LeaseReleaser {
    fn release_all_for_owner(&mut self, owner: u8) -> usize;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoopLeaseReleaser;

impl LeaseReleaser for NoopLeaseReleaser {
    fn release_all_for_owner(&mut self, _owner: u8) -> usize {
        0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HotReloadPlan<const N: usize> {
    pub module: ModuleId,
    pub new_revision: u32,
    pub steps: [Option<HotReloadStep>; N],
    pub len: usize,
    pub required_budget_us: u64,
    pub deadline_us: u64,
}

impl<const N: usize> HotReloadPlan<N> {
    pub const fn new(module: ModuleId, new_revision: u32) -> Self {
        Self {
            module,
            new_revision,
            steps: [None; N],
            len: 0,
            required_budget_us: 0,
            deadline_us: 0,
        }
    }

    pub fn build(
        module: ModuleId,
        new_revision: u32,
        now_us: u64,
        policy: HotReloadPolicy,
    ) -> Result<Self, HotReloadError> {
        if module == ModuleId::Kernel {
            return Err(HotReloadError::CannotReloadKernel);
        }

        let mut plan = Self::new(module, new_revision);
        let mut due_us = now_us;
        plan.push(
            HotReloadStepKind::QuiesceModule,
            due_us,
            policy.quiesce_budget_us,
        )?;
        due_us = due_us.saturating_add(u64::from(policy.quiesce_budget_us));
        plan.push(
            HotReloadStepKind::ReleaseLeases,
            due_us,
            policy.lease_release_budget_us,
        )?;
        due_us = due_us.saturating_add(u64::from(policy.lease_release_budget_us));
        plan.push(
            HotReloadStepKind::UnmountModule,
            due_us,
            policy.unmount_budget_us,
        )?;
        due_us = due_us.saturating_add(u64::from(policy.unmount_budget_us));
        plan.push(
            HotReloadStepKind::MountModule,
            due_us,
            policy.mount_budget_us,
        )?;
        due_us = due_us.saturating_add(u64::from(policy.mount_budget_us));
        plan.push(
            HotReloadStepKind::ResumeModule,
            due_us,
            policy.resume_budget_us,
        )?;

        if plan.required_budget_us > u64::from(policy.max_total_budget_us) {
            return Err(HotReloadError::BudgetExceeded {
                required_us: plan.required_budget_us,
                limit_us: policy.max_total_budget_us,
            });
        }
        plan.deadline_us = now_us.saturating_add(plan.required_budget_us);
        Ok(plan)
    }

    pub fn last(&self) -> Option<HotReloadStep> {
        if self.len == 0 {
            None
        } else {
            self.steps[self.len - 1]
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn push(
        &mut self,
        kind: HotReloadStepKind,
        due_us: u64,
        budget_us: u32,
    ) -> Result<(), HotReloadError> {
        if self.len >= N {
            return Err(HotReloadError::PlanFull);
        }
        self.steps[self.len] = Some(HotReloadStep::new(
            self.module,
            kind,
            due_us,
            budget_us,
            self.new_revision,
        ));
        self.len += 1;
        self.required_budget_us = self.required_budget_us.saturating_add(u64::from(budget_us));
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HotReloadOutcome<const N: usize> {
    pub module: ModuleId,
    pub lease_owner: u8,
    pub released_leases: usize,
    pub released_quota: SystemBudget,
    pub new_revision: u32,
    pub plan: HotReloadPlan<N>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_reload_plan_is_ordered_and_bounded() {
        let plan = HotReloadPlan::<5>::build(ModuleId::Sensor, 42, 1_000, HotReloadPolicy::DEFAULT)
            .unwrap();

        assert_eq!(plan.len, 5);
        assert_eq!(plan.required_budget_us, 2_750);
        assert_eq!(plan.deadline_us, 3_750);
        assert_eq!(
            plan.steps[0],
            Some(HotReloadStep::new(
                ModuleId::Sensor,
                HotReloadStepKind::QuiesceModule,
                1_000,
                500,
                42,
            ))
        );
        assert_eq!(
            plan.last().map(|step| step.kind),
            Some(HotReloadStepKind::ResumeModule)
        );
    }

    #[test]
    fn hot_reload_plan_reports_capacity_and_budget_errors() {
        assert_eq!(
            HotReloadPlan::<4>::build(ModuleId::Sensor, 2, 0, HotReloadPolicy::DEFAULT,),
            Err(HotReloadError::PlanFull)
        );

        assert_eq!(
            HotReloadPlan::<5>::build(
                ModuleId::Sensor,
                2,
                0,
                HotReloadPolicy {
                    max_total_budget_us: 1_000,
                    ..HotReloadPolicy::DEFAULT
                },
            ),
            Err(HotReloadError::BudgetExceeded {
                required_us: 2_750,
                limit_us: 1_000,
            })
        );
    }

    #[test]
    fn kernel_is_not_hot_reloadable() {
        assert_eq!(
            HotReloadPlan::<5>::build(ModuleId::Kernel, 1, 0, HotReloadPolicy::DEFAULT,),
            Err(HotReloadError::CannotReloadKernel)
        );
    }
}
