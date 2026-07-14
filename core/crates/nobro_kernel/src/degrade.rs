//! Degraded-mode planner for fitting module sets into a system profile.

use crate::{Criticality, ModuleId, ModuleSpec, SystemBudget, SystemProfile};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DegradeReason {
    FlashBudget,
    RamBudget,
    PoolBudget,
    ModuleLimit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DegradeError {
    TooManyModules,
    EssentialOverBudget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DegradeDecision<const N: usize> {
    pub enabled: [bool; N],
    pub disabled: [Option<ModuleId>; N],
    pub disabled_count: usize,
    pub budget: SystemBudget,
    pub reason: Option<DegradeReason>,
}

pub struct DegradePlanner;

impl DegradePlanner {
    pub fn fit<const N: usize>(
        modules: &[ModuleSpec],
        profile: SystemProfile,
    ) -> Result<DegradeDecision<N>, DegradeError> {
        if modules.len() > N {
            return Err(DegradeError::TooManyModules);
        }

        let mut decision = DegradeDecision {
            enabled: [false; N],
            disabled: [None; N],
            disabled_count: 0,
            budget: SystemBudget::new(0, 0, 0),
            reason: None,
        };

        for idx in 0..modules.len() {
            decision.enabled[idx] = true;
        }
        decision.budget = total_budget(modules, &decision.enabled);

        while !decision.budget.fits_within(profile.budget())
            || enabled_count(&decision.enabled) > profile.max_modules
        {
            let reason =
                overflow_reason(decision.budget, profile, enabled_count(&decision.enabled));
            decision.reason = Some(reason);

            let Some(drop_idx) = pick_drop_candidate(modules, &decision.enabled) else {
                return Err(DegradeError::EssentialOverBudget);
            };

            decision.enabled[drop_idx] = false;
            decision.disabled[decision.disabled_count] = Some(modules[drop_idx].id);
            decision.disabled_count += 1;
            decision.budget = total_budget(modules, &decision.enabled);
        }

        Ok(decision)
    }
}

fn pick_drop_candidate(modules: &[ModuleSpec], enabled: &[bool]) -> Option<usize> {
    let mut selected = None;
    for (idx, spec) in modules.iter().enumerate() {
        if !enabled.get(idx).copied().unwrap_or(false) {
            continue;
        }
        if spec.criticality >= Criticality::System {
            continue;
        }

        selected = match selected {
            None => Some(idx),
            Some(prev_idx) => {
                let prev = modules[prev_idx];
                if spec.criticality < prev.criticality
                    || (spec.criticality == prev.criticality
                        && spec.memory.flash_bytes > prev.memory.flash_bytes)
                {
                    Some(idx)
                } else {
                    Some(prev_idx)
                }
            }
        };
    }
    selected
}

fn total_budget(modules: &[ModuleSpec], enabled: &[bool]) -> SystemBudget {
    let mut budget = SystemBudget::new(0, 0, 0);
    for (idx, spec) in modules.iter().enumerate() {
        if !enabled.get(idx).copied().unwrap_or(false) {
            continue;
        }
        budget.flash_bytes = budget.flash_bytes.saturating_add(spec.memory.flash_bytes);
        budget.ram_bytes = budget.ram_bytes.saturating_add(spec.memory.ram_bytes);
        budget.pool_slots = budget.pool_slots.saturating_add(spec.memory.pool_slots);
    }
    budget
}

fn enabled_count(enabled: &[bool]) -> usize {
    enabled.iter().filter(|is_enabled| **is_enabled).count()
}

fn overflow_reason(budget: SystemBudget, profile: SystemProfile, modules: usize) -> DegradeReason {
    if modules > profile.max_modules {
        DegradeReason::ModuleLimit
    } else if budget.flash_bytes > profile.flash_limit_bytes {
        DegradeReason::FlashBudget
    } else if budget.ram_bytes > profile.ram_limit_bytes {
        DegradeReason::RamBudget
    } else {
        DegradeReason::PoolBudget
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeadlineContract, MemoryBudget};

    fn module(id: ModuleId, criticality: Criticality, flash: u32, ram: u32) -> ModuleSpec {
        let spec = ModuleSpec::new(id, criticality).memory(MemoryBudget::new(flash, ram, 0));
        if criticality == Criticality::HardRealtime {
            spec.deadline(DeadlineContract::new(20_000, 10))
        } else {
            spec
        }
    }

    #[test]
    fn planner_drops_best_effort_first() {
        let modules = [
            module(ModuleId::Kernel, Criticality::HardRealtime, 20, 4),
            module(ModuleId::Sensor, Criticality::Driver, 20, 4),
            module(ModuleId::App(1), Criticality::BestEffort, 50, 4),
            module(ModuleId::App(2), Criticality::User, 20, 4),
        ];

        let decision = DegradePlanner::fit::<4>(
            &modules,
            SystemProfile {
                flash_limit_bytes: 70,
                ram_limit_bytes: 32,
                pool_slot_limit: 8,
                max_modules: 4,
                wake_latency_us: 0,
            },
        )
        .unwrap();

        assert_eq!(decision.disabled_count, 1);
        assert_eq!(decision.disabled[0], Some(ModuleId::App(1)));
        assert!(decision.enabled[0]);
        assert!(decision.enabled[1]);
        assert!(decision.enabled[3]);
        assert_eq!(decision.reason, Some(DegradeReason::FlashBudget));
    }

    #[test]
    fn planner_reports_essential_over_budget() {
        let modules = [
            module(ModuleId::Kernel, Criticality::HardRealtime, 100, 4),
            module(ModuleId::Hal, Criticality::System, 100, 4),
        ];

        assert_eq!(
            DegradePlanner::fit::<2>(
                &modules,
                SystemProfile {
                    flash_limit_bytes: 50,
                    ram_limit_bytes: 32,
                    pool_slot_limit: 8,
                    max_modules: 2,
                    wake_latency_us: 0,
                },
            ),
            Err(DegradeError::EssentialOverBudget)
        );
    }

    #[test]
    fn planner_can_fit_module_count_limit() {
        let modules = [
            module(ModuleId::Kernel, Criticality::HardRealtime, 10, 1),
            module(ModuleId::App(1), Criticality::BestEffort, 10, 1),
            module(ModuleId::App(2), Criticality::User, 10, 1),
        ];

        let decision = DegradePlanner::fit::<3>(
            &modules,
            SystemProfile {
                flash_limit_bytes: 100,
                ram_limit_bytes: 100,
                pool_slot_limit: 8,
                max_modules: 2,
                wake_latency_us: 0,
            },
        )
        .unwrap();

        assert_eq!(decision.disabled_count, 1);
        assert_eq!(decision.disabled[0], Some(ModuleId::App(1)));
        assert_eq!(decision.reason, Some(DegradeReason::ModuleLimit));
    }
}
