//! Declarative multicore placement and per-core admission (Wave 57).
//!
//! A single [`AppGraph`](crate::AppGraph) declaration is placed onto `CORES`
//! cores here, before any dual-core *execution* (that is Wave 58). Placement is
//! beginner-safe by default — a task with no `.core(n)` affinity is assigned
//! automatically by balancing CPU utilization — while explicit `.core(n)`
//! affinity is always honored. The output is one [`CorePlan`] per core plus
//! diagnostics:
//!
//! - **Per-core admission**: each core's assigned tasks must fit the 100%
//!   utilization bound; an overloaded core is reported, never silently accepted.
//! - **Cross-core transport**: a declared channel whose endpoints land on
//!   different cores is flagged as needing a bounded cross-core transport
//!   (the [`MpmcChannel`](crate::async_mpmc::MpmcChannel) is exactly that —
//!   `critical-section` based, so it already works across cores).
//! - **Placement diagnostics**: pinned-task overload and unknown affinity are
//!   attributed to the offending task, so a bad placement fails with one clear
//!   reason (MC-01 / SCH-09).
//!
//! The planner is deterministic (no clock, no randomness): the auto pass is
//! worst-fit-decreasing by utilization, so the same graph always places the
//! same way — reproducible admission evidence.

use crate::{AppGraph, TaskDecl};

/// The utilization a task contributes, in parts-per-ten-thousand.
fn task_util(decl: &TaskDecl) -> u32 {
    if !decl.has_deadline || decl.period_us == 0 {
        return 0;
    }
    ((decl.execution_budget_us as u64 * 10_000) / decl.period_us as u64) as u32
}

/// One core's assignment: which task labels run on it and the total utilization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CorePlan<const T: usize> {
    pub core: u8,
    pub tasks: [Option<&'static str>; T],
    pub task_len: usize,
    pub utilization_permyriad: u32,
}

impl<const T: usize> CorePlan<T> {
    const fn new(core: u8) -> Self {
        Self {
            core,
            tasks: [None; T],
            task_len: 0,
            utilization_permyriad: 0,
        }
    }

    fn push(&mut self, label: &'static str, util: u32) {
        if self.task_len < T {
            self.tasks[self.task_len] = Some(label);
            self.task_len += 1;
        }
        self.utilization_permyriad = self.utilization_permyriad.saturating_add(util);
    }

    pub fn labels(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.tasks.iter().flatten().copied()
    }

    pub fn overloaded(&self) -> bool {
        self.utilization_permyriad > 10_000
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementError {
    /// A task pinned to a core index that does not exist.
    UnknownCore { task: &'static str, core: u8 },
    /// A core's assigned utilization exceeds 100%.
    CoreOverloaded {
        core: u8,
        utilization_permyriad: u32,
    },
    /// A channel endpoint is not a declared task.
    UnknownEndpoint { endpoint: &'static str },
}

/// A cross-core channel edge that needs a bounded transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrossCoreLink {
    pub from: &'static str,
    pub from_core: u8,
    pub to: &'static str,
    pub to_core: u8,
}

/// The full placement of an [`AppGraph`] onto `CORES` cores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CorePlacement<const CORES: usize, const T: usize> {
    pub cores: [CorePlan<T>; CORES],
    pub cross_core: [Option<CrossCoreLink>; T],
    pub cross_core_len: usize,
}

impl<const CORES: usize, const T: usize> CorePlacement<CORES, T> {
    /// Which core a task label was placed on.
    pub fn core_of(&self, label: &str) -> Option<u8> {
        self.cores
            .iter()
            .find(|plan| plan.labels().any(|l| l == label))
            .map(|plan| plan.core)
    }

    pub fn cross_core_links(&self) -> impl Iterator<Item = &CrossCoreLink> + '_ {
        self.cross_core.iter().flatten()
    }

    /// Cores that exceed the utilization bound (empty when schedulable).
    pub fn overloaded_cores(&self) -> impl Iterator<Item = &CorePlan<T>> + '_ {
        self.cores.iter().filter(|plan| plan.overloaded())
    }
}

/// Place `graph` onto `CORES` cores. `T` bounds tasks-per-core and channel count.
pub fn plan_placement<const CORES: usize, const T: usize, const TASKS: usize>(
    graph: &AppGraph<TASKS>,
) -> Result<CorePlacement<CORES, T>, PlacementError> {
    assert!(CORES >= 1, "at least one core");
    let mut cores: [CorePlan<T>; CORES] = core::array::from_fn(|i| CorePlan::new(i as u8));

    // Pass 1: honor explicit affinity.
    for decl in graph.task_decls() {
        if let Some(core) = decl.core_affinity {
            if core as usize >= CORES {
                return Err(PlacementError::UnknownCore {
                    task: decl.name,
                    core,
                });
            }
            cores[core as usize].push(decl.name, task_util(decl));
        }
    }

    // Pass 2: auto-place the rest, heaviest first, onto the least-loaded core
    // (worst-fit-decreasing — deterministic, spreads load).
    // Collect unpinned tasks with their utilization.
    let mut auto: [Option<(&'static str, u32)>; TASKS] = [None; TASKS];
    let mut auto_len = 0;
    for decl in graph.task_decls() {
        if decl.core_affinity.is_none() {
            auto[auto_len] = Some((decl.name, task_util(decl)));
            auto_len += 1;
        }
    }
    // Descending utilization (insertion sort; bounded, no alloc).
    for i in 1..auto_len {
        let mut j = i;
        while j > 0 && auto[j].unwrap().1 > auto[j - 1].unwrap().1 {
            auto.swap(j, j - 1);
            j -= 1;
        }
    }
    for entry in auto.iter().flatten() {
        let (label, util) = *entry;
        // Pick the least-loaded core.
        let mut best = 0usize;
        for c in 1..CORES {
            if cores[c].utilization_permyriad < cores[best].utilization_permyriad {
                best = c;
            }
        }
        cores[best].push(label, util);
    }

    // Per-core admission: no core may exceed 100%.
    for plan in cores.iter() {
        if plan.overloaded() {
            return Err(PlacementError::CoreOverloaded {
                core: plan.core,
                utilization_permyriad: plan.utilization_permyriad,
            });
        }
    }

    // Build the placement, then detect cross-core channel links.
    let mut placement = CorePlacement {
        cores,
        cross_core: [None; T],
        cross_core_len: 0,
    };
    for (from, to) in graph.channel_pairs() {
        let from_core = placement
            .core_of(from)
            .ok_or(PlacementError::UnknownEndpoint { endpoint: from })?;
        let to_core = placement
            .core_of(to)
            .ok_or(PlacementError::UnknownEndpoint { endpoint: to })?;
        if from_core != to_core && placement.cross_core_len < T {
            placement.cross_core[placement.cross_core_len] = Some(CrossCoreLink {
                from,
                from_core,
                to,
                to_core,
            });
            placement.cross_core_len += 1;
        }
    }

    Ok(placement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Criticality, MemoryBudget};

    fn t(name: &'static str, period: u32, budget: u32) -> TaskDecl {
        TaskDecl::periodic(name, period)
            .budget_us(budget)
            .memory(MemoryBudget::new(512, 128, 0))
    }

    #[test]
    fn auto_placement_balances_utilization_across_cores() {
        // Four 25% tasks over two cores -> 50% each, deterministic.
        let graph = AppGraph::<4>::new()
            .task(t("a", 10_000, 2_500))
            .unwrap()
            .task(t("b", 10_000, 2_500))
            .unwrap()
            .task(t("c", 10_000, 2_500))
            .unwrap()
            .task(t("d", 10_000, 2_500))
            .unwrap();
        let placement = plan_placement::<2, 4, 4>(&graph).unwrap();
        assert_eq!(placement.cores[0].utilization_permyriad, 5_000);
        assert_eq!(placement.cores[1].utilization_permyriad, 5_000);
        assert!(placement.overloaded_cores().count() == 0);
    }

    #[test]
    fn explicit_affinity_is_honored_and_auto_fills_the_rest() {
        let graph = AppGraph::<3>::new()
            .task(t("pinned", 10_000, 6_000).core(1))
            .unwrap()
            .task(t("auto1", 10_000, 1_000))
            .unwrap()
            .task(t("auto2", 10_000, 1_000))
            .unwrap();
        let placement = plan_placement::<2, 4, 3>(&graph).unwrap();
        assert_eq!(placement.core_of("pinned"), Some(1));
        // Both auto tasks go to core 0 (least loaded after the 60% pin).
        assert_eq!(placement.core_of("auto1"), Some(0));
        assert_eq!(placement.core_of("auto2"), Some(0));
    }

    #[test]
    fn cross_core_channel_is_detected_for_bounded_transport() {
        let graph = AppGraph::<2>::new()
            .task(t("producer", 10_000, 9_000).core(0))
            .unwrap()
            .task(t("consumer", 10_000, 9_000).core(1))
            .unwrap()
            .channel("producer", "consumer")
            .unwrap();
        let placement = plan_placement::<2, 4, 2>(&graph).unwrap();
        let links: std::vec::Vec<_> = placement.cross_core_links().copied().collect();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].from_core, 0);
        assert_eq!(links[0].to_core, 1);
    }

    #[test]
    fn per_core_overload_is_rejected_with_attribution() {
        // Two 70% tasks both pinned to core 0 -> 140% -> rejected.
        let graph = AppGraph::<2>::new()
            .task(t("x", 10_000, 7_000).core(0))
            .unwrap()
            .task(t("y", 10_000, 7_000).core(0))
            .unwrap();
        assert_eq!(
            plan_placement::<2, 4, 2>(&graph),
            Err(PlacementError::CoreOverloaded {
                core: 0,
                utilization_permyriad: 14_000,
            })
        );
    }

    #[test]
    fn unknown_core_affinity_is_attributed() {
        let graph = AppGraph::<1>::new()
            .task(t("z", 10_000, 1_000).core(5))
            .unwrap();
        assert_eq!(
            plan_placement::<2, 4, 1>(&graph),
            Err(PlacementError::UnknownCore { task: "z", core: 5 })
        );
    }

    #[test]
    fn best_effort_tasks_carry_no_utilization_but_still_place() {
        let graph = AppGraph::<2>::new()
            .task(TaskDecl::service("bg", 50_000).memory(MemoryBudget::new(512, 128, 0)))
            .unwrap()
            .task(t("rt", 10_000, 5_000).criticality(Criticality::HardRealtime))
            .unwrap();
        let placement = plan_placement::<2, 4, 2>(&graph).unwrap();
        // The service adds 0% util; placement still assigns it a core.
        assert!(placement.core_of("bg").is_some());
    }
}
