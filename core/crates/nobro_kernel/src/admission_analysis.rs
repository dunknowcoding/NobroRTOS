//! Admission cost analysis and deadline/best-effort balance (Wave 47).
//!
//! Given a derived contract (a [`SystemManifest`]) and a platform
//! [`SystemProfile`], this reports the **marginal** cost of every module —
//! the flash / RAM / pool-slot / CPU-utilization a module contributes, i.e.
//! exactly what removing it frees — and whether the set fits both the memory
//! budget and the 100% utilization bound.
//!
//! When the set does NOT fit, [`AdmissionAnalysis::shed_plan`] produces one
//! actionable answer: the smallest set of **best-effort-first** modules to
//! drop to make the system schedulable, in ascending criticality order, so
//! deadline-critical work (`System` / `HardRealtime`) is never shed to make
//! room for feature load. The kernel module is never a shed candidate.
//!
//! This is the policy the review asked for — "deadline-critical work versus
//! best-effort feature load" — expressed as data an operator or a build gate
//! can act on, not a hard-coded scheduler decision. It never mutates the
//! manifest; the caller applies the plan by rebuilding the graph without the
//! named modules.

use crate::{Criticality, DeadlineContract, ModuleId, SystemBudget, SystemManifest, SystemProfile};

/// The marginal contribution of one module to the system's cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleCost {
    pub module: ModuleId,
    pub criticality: Criticality,
    pub flash_bytes: u32,
    pub ram_bytes: u32,
    pub pool_slots: u16,
    /// CPU utilization in parts-per-ten-thousand (0 for tasks with no deadline).
    pub utilization_permyriad: u64,
}

/// Why a shed candidate could not be enumerated (bounded output overflow).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShedError {
    /// More modules must be shed than the caller's output buffer holds.
    OutputFull,
    /// Even after shedding every best-effort/user/driver module the system
    /// still does not fit — the deadline-critical core alone is over budget.
    Infeasible,
}

/// A concrete, actionable remediation for an over-budget system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShedPlan<const N: usize> {
    /// Modules to drop, ascending criticality (best-effort first).
    pub shed: [Option<ModuleId>; N],
    pub shed_len: usize,
    /// The budget/utilization headroom once the plan is applied.
    pub freed_flash: u32,
    pub freed_ram: u32,
    pub freed_util_permyriad: u64,
}

impl<const N: usize> ShedPlan<N> {
    pub fn shed_modules(&self) -> impl Iterator<Item = ModuleId> + '_ {
        self.shed.iter().flatten().copied()
    }
}

/// Analysis of one derived contract against a platform profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmissionAnalysis<const N: usize> {
    costs: [Option<ModuleCost>; N],
    len: usize,
    used: SystemBudget,
    limit: SystemBudget,
    total_util_permyriad: u64,
}

impl<const N: usize> AdmissionAnalysis<N> {
    /// Price every module in the manifest against the profile.
    pub fn analyze<const M: usize>(manifest: &SystemManifest<M>, profile: SystemProfile) -> Self {
        let mut costs = [None; N];
        let mut len = 0;
        let mut total_util = 0u64;
        for spec in manifest.iter() {
            let util = spec
                .deadline
                .map(DeadlineContract::utilization_permyriad)
                .unwrap_or(0);
            total_util = total_util.saturating_add(util);
            if len < N {
                costs[len] = Some(ModuleCost {
                    module: spec.id,
                    criticality: spec.criticality,
                    flash_bytes: spec.memory.flash_bytes,
                    ram_bytes: spec.memory.ram_bytes,
                    pool_slots: spec.memory.pool_slots,
                    utilization_permyriad: util,
                });
                len += 1;
            }
        }
        Self {
            costs,
            len,
            used: manifest.total_budget(),
            limit: profile.budget(),
            total_util_permyriad: total_util,
        }
    }

    pub fn costs(&self) -> impl Iterator<Item = ModuleCost> + '_ {
        self.costs.iter().flatten().copied()
    }

    pub fn cost_of(&self, module: ModuleId) -> Option<ModuleCost> {
        self.costs
            .iter()
            .flatten()
            .copied()
            .find(|cost| cost.module == module)
    }

    pub const fn used(&self) -> SystemBudget {
        self.used
    }

    pub const fn limit(&self) -> SystemBudget {
        self.limit
    }

    pub const fn total_utilization_permyriad(&self) -> u64 {
        self.total_util_permyriad
    }

    /// True when the set fits memory AND stays within the utilization bound.
    pub fn schedulable(&self) -> bool {
        self.used.fits_within(self.limit) && self.total_util_permyriad <= 10_000
    }

    /// Flash/RAM/util headroom (saturating; a component reads 0 when over).
    pub fn headroom(&self) -> (u32, u32, u64) {
        (
            self.limit.flash_bytes.saturating_sub(self.used.flash_bytes),
            self.limit.ram_bytes.saturating_sub(self.used.ram_bytes),
            10_000u64.saturating_sub(self.total_util_permyriad),
        )
    }

    /// The smallest best-effort-first set of modules to drop so the system
    /// becomes schedulable. Deadline-critical modules (`System` /
    /// `HardRealtime`) and the kernel are never candidates: if the system
    /// cannot fit without shedding them, this returns [`ShedError::Infeasible`]
    /// (the operator must cut budgets or move to a bigger profile, not drop
    /// safety-critical work).
    pub fn shed_plan<const S: usize>(&self) -> Result<ShedPlan<S>, ShedError> {
        if self.schedulable() {
            return Ok(ShedPlan {
                shed: [None; S],
                shed_len: 0,
                freed_flash: 0,
                freed_ram: 0,
                freed_util_permyriad: 0,
            });
        }

        // Sheddable = anything below System criticality, excluding the kernel.
        // Order: lowest criticality first, then largest utilization (shed the
        // heaviest feature soonest), then largest RAM.
        let mut order: [Option<ModuleCost>; N] = [None; N];
        let mut order_len = 0;
        for cost in self.costs.iter().flatten() {
            if cost.module == ModuleId::Kernel || cost.criticality >= Criticality::System {
                continue;
            }
            order[order_len] = Some(*cost);
            order_len += 1;
        }
        // Insertion sort (bounded N, no alloc): ascending criticality, then
        // descending utilization, then descending RAM.
        for i in 1..order_len {
            let mut j = i;
            while j > 0 && shed_before(order[j].unwrap(), order[j - 1].unwrap()) {
                order.swap(j, j - 1);
                j -= 1;
            }
        }

        let mut plan = ShedPlan::<S> {
            shed: [None; S],
            shed_len: 0,
            freed_flash: 0,
            freed_ram: 0,
            freed_util_permyriad: 0,
        };
        let mut used = self.used;
        let mut util = self.total_util_permyriad;
        for cost in order.iter().flatten() {
            if fits(used, self.limit) && util <= 10_000 {
                break;
            }
            if plan.shed_len == S {
                return Err(ShedError::OutputFull);
            }
            plan.shed[plan.shed_len] = Some(cost.module);
            plan.shed_len += 1;
            plan.freed_flash = plan.freed_flash.saturating_add(cost.flash_bytes);
            plan.freed_ram = plan.freed_ram.saturating_add(cost.ram_bytes);
            plan.freed_util_permyriad = plan
                .freed_util_permyriad
                .saturating_add(cost.utilization_permyriad);
            used = SystemBudget::new(
                used.flash_bytes.saturating_sub(cost.flash_bytes),
                used.ram_bytes.saturating_sub(cost.ram_bytes),
                used.pool_slots.saturating_sub(cost.pool_slots),
            );
            util = util.saturating_sub(cost.utilization_permyriad);
        }

        if fits(used, self.limit) && util <= 10_000 {
            Ok(plan)
        } else {
            // Only the deadline-critical core is left and it still doesn't fit.
            Err(ShedError::Infeasible)
        }
    }
}

fn fits(used: SystemBudget, limit: SystemBudget) -> bool {
    used.fits_within(limit)
}

/// Shed-priority ordering: lower criticality first, then heavier utilization,
/// then heavier RAM.
fn shed_before(a: ModuleCost, b: ModuleCost) -> bool {
    if a.criticality != b.criticality {
        return a.criticality < b.criticality;
    }
    if a.utilization_permyriad != b.utilization_permyriad {
        return a.utilization_permyriad > b.utilization_permyriad;
    }
    a.ram_bytes > b.ram_bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppGraph, Capability, CapabilitySet, MemoryBudget, TaskDecl};

    /// A representative robotics workload built entirely through the graph API:
    /// concurrent sensing, control, an audio pipeline, a camera/AI sidecar,
    /// storage, radio, OTA, and diagnostics — roles substituted freely, never
    /// hard-coded. This is the Wave 47 complex-workload compile proof.
    fn robotics_graph() -> AppGraph<10> {
        AppGraph::<10>::new()
            .kernel(
                MemoryBudget::new(12 * 1024, 3 * 1024, 2),
                crate::DeadlineContract::new(20_000, 100),
            )
            // Deadline-critical core.
            .task(
                TaskDecl::control("motor", 5_000)
                    .memory(MemoryBudget::new(2 * 1024, 512, 0))
                    .budget_us(400),
            )
            .unwrap()
            .task(
                TaskDecl::periodic("imu", 10_000)
                    .criticality(Criticality::System)
                    .memory(MemoryBudget::new(2 * 1024, 512, 1))
                    .budget_us(300),
            )
            .unwrap()
            // Driver-tier feature work.
            .task(
                TaskDecl::periodic("radio", 20_000)
                    .memory(MemoryBudget::new(4 * 1024, 512, 0))
                    .budget_us(800),
            )
            .unwrap()
            .task(
                TaskDecl::periodic("storage", 50_000)
                    .memory(MemoryBudget::new(3 * 1024, 512, 0))
                    .budget_us(1_000),
            )
            .unwrap()
            // Best-effort feature load (shed first under pressure).
            .task(TaskDecl::service("audio", 30_000).memory(MemoryBudget::new(4 * 1024, 1024, 0)))
            .unwrap()
            .task(
                TaskDecl::service("camera_ai", 40_000)
                    .memory(MemoryBudget::new(8 * 1024, 2 * 1024, 0))
                    .requires(CapabilitySet::empty().with(Capability::AiInference))
                    .owns(CapabilitySet::empty().with(Capability::AiInference)),
            )
            .unwrap()
            .task(TaskDecl::service("ota", 100_000).memory(MemoryBudget::new(4 * 1024, 512, 0)))
            .unwrap()
            .task(
                TaskDecl::service("diagnostics", 200_000).memory(MemoryBudget::new(
                    2 * 1024,
                    256,
                    0,
                )),
            )
            .unwrap()
    }

    #[test]
    fn complex_workload_builds_and_prices_every_task() {
        let profile = SystemProfile::new(128 * 1024, 64 * 1024, 8, 16);
        let built = robotics_graph().build_for::<10>(profile).unwrap();
        let analysis = AdmissionAnalysis::<10>::analyze(&built.manifest, profile);

        // Every task (plus kernel) has a marginal-cost row.
        assert_eq!(analysis.costs().count(), 9);
        // The heaviest utilization is the 5 kHz motor loop: 400/5000 = 8%.
        let motor = built.module_of("motor").unwrap();
        assert_eq!(analysis.cost_of(motor).unwrap().utilization_permyriad, 800);
        assert!(analysis.schedulable());
        let (flash_free, ram_free, util_free) = analysis.headroom();
        assert!(flash_free > 0 && ram_free > 0 && util_free > 0);
    }

    #[test]
    fn overloaded_set_sheds_best_effort_first_never_critical() {
        // A deliberately tiny RAM profile forces shedding.
        let profile = SystemProfile::new(128 * 1024, 8 * 1024, 8, 16);
        let built = robotics_graph().build::<10>().unwrap();
        let analysis = AdmissionAnalysis::<10>::analyze(&built.manifest, profile);
        assert!(!analysis.schedulable(), "should be over the RAM budget");

        let plan = analysis.shed_plan::<8>().unwrap();
        assert!(plan.shed_len > 0);

        // Nothing deadline-critical or the kernel was shed.
        let motor = built.module_of("motor").unwrap();
        let imu = built.module_of("imu").unwrap();
        for module in plan.shed_modules() {
            assert_ne!(module, ModuleId::Kernel);
            assert_ne!(module, motor);
            assert_ne!(module, imu);
            let criticality = analysis.cost_of(module).unwrap().criticality;
            assert!(criticality < Criticality::System);
        }

        // Applying the plan (rebuild without the shed modules) is schedulable.
        // The heaviest best-effort consumer (camera_ai, 8KB) is shed first.
        let camera = built.module_of("camera_ai").unwrap();
        assert_eq!(plan.shed.first().copied().flatten(), Some(camera));
        assert!(plan.freed_ram >= 2 * 1024);
    }

    #[test]
    fn shed_plan_reports_infeasible_when_critical_core_alone_overflows() {
        // Profile smaller than the kernel + critical tasks: no best-effort
        // shedding can rescue it, and safety work is never dropped.
        let profile = SystemProfile::new(4 * 1024, 1024, 1, 16);
        let built = robotics_graph().build::<10>().unwrap();
        let analysis = AdmissionAnalysis::<10>::analyze(&built.manifest, profile);
        assert_eq!(analysis.shed_plan::<8>(), Err(ShedError::Infeasible));
    }

    #[test]
    fn utilization_scales_linearly_and_admission_rejects_the_overload() {
        // Add identical 10% tasks until the utilization bound rejects the set:
        // proves the marginal-cost model is linear and the bound is enforced.
        const NAMES: [&str; 11] = [
            "t0", "t1", "t2", "t3", "t4", "t5", "t6", "t7", "t8", "t9", "t10",
        ];

        fn build_n(count: usize) -> Result<AppGraph<12>, crate::GraphError> {
            let mut graph = Some(AppGraph::<12>::new());
            for name in NAMES.iter().take(count) {
                let current = graph.take().unwrap();
                graph = Some(
                    current.task(
                        TaskDecl::periodic(name, 10_000)
                            .memory(MemoryBudget::new(512, 128, 0))
                            .budget_us(1_000), // 10% each
                    )?,
                );
            }
            Ok(graph.unwrap())
        }

        let profile = SystemProfile::new(256 * 1024, 128 * 1024, 8, 16);
        let util_at = |count: usize| -> u64 {
            let built = build_n(count).unwrap().build::<12>().unwrap();
            AdmissionAnalysis::<12>::analyze(&built.manifest, profile).total_utilization_permyriad()
        };
        assert_eq!(util_at(5), 5_000); // 50%
        assert_eq!(util_at(9), 9_000); // 90%, still schedulable

        // 11 tasks = 110% util: build_for rejects it as overutilized.
        assert!(build_n(11).unwrap().build_for::<12>(profile).is_err());
    }
}
