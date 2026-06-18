//! Boot admission checks that compose manifest, startup, and quota contracts.

use crate::{
    ManifestError, ModuleId, QuotaError, QuotaLedger, StartupError, StartupNode, StartupPlan,
    StartupPlanner, SystemBudget, SystemManifest, SystemProfile,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    Manifest(ManifestError),
    Startup(StartupError),
    Quota(QuotaError),
    MissingStartupNode(ModuleId),
    UnknownStartupNode(ModuleId),
}

#[derive(Debug)]
pub struct AdmissionPlan<const STARTUP: usize, const QUOTAS: usize> {
    pub startup: StartupPlan<STARTUP>,
    pub quotas: QuotaLedger<QUOTAS>,
    pub used: SystemBudget,
    pub profile: SystemProfile,
}

impl<const STARTUP: usize, const QUOTAS: usize> AdmissionPlan<STARTUP, QUOTAS> {
    pub const fn module_count(&self) -> usize {
        self.startup.len
    }
}

pub struct AdmissionController;

impl AdmissionController {
    pub fn admit<const MODULES: usize, const STARTUP: usize, const QUOTAS: usize>(
        manifest: &SystemManifest<MODULES>,
        startup_nodes: &[StartupNode],
        profile: SystemProfile,
    ) -> Result<AdmissionPlan<STARTUP, QUOTAS>, AdmissionError> {
        manifest
            .validate_profile(profile)
            .map_err(AdmissionError::Manifest)?;
        Self::validate_startup_coverage(manifest, startup_nodes)?;

        let startup =
            StartupPlanner::plan::<STARTUP>(startup_nodes).map_err(AdmissionError::Startup)?;
        let mut quotas = QuotaLedger::<QUOTAS>::new();
        quotas
            .register_manifest(manifest)
            .map_err(AdmissionError::Quota)?;

        Ok(AdmissionPlan {
            startup,
            quotas,
            used: manifest.total_budget(),
            profile,
        })
    }

    fn validate_startup_coverage<const MODULES: usize>(
        manifest: &SystemManifest<MODULES>,
        startup_nodes: &[StartupNode],
    ) -> Result<(), AdmissionError> {
        for spec in manifest.iter() {
            if !startup_nodes.iter().any(|node| node.module == spec.id) {
                return Err(AdmissionError::MissingStartupNode(spec.id));
            }
        }

        for node in startup_nodes {
            if !manifest.iter().any(|spec| spec.id == node.module) {
                return Err(AdmissionError::UnknownStartupNode(node.module));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        kernel_owned_capabilities, Capability, CapabilitySet, Criticality, DeadlineContract,
        DependencySet, FaultThresholds, MemoryBudget, ModuleSpec,
    };

    fn profile() -> SystemProfile {
        SystemProfile {
            flash_limit_bytes: 64 * 1024,
            ram_limit_bytes: 16 * 1024,
            pool_slot_limit: 8,
            max_modules: 4,
        }
    }

    fn kernel_spec() -> ModuleSpec {
        ModuleSpec::new(ModuleId::Kernel, Criticality::HardRealtime)
            .owns(kernel_owned_capabilities())
            .memory(MemoryBudget::new(16 * 1024, 4 * 1024, 4))
            .deadline(DeadlineContract::new(20_000, 10))
            .fault_thresholds(FaultThresholds {
                notify_after: 2,
                reboot_after: 4,
            })
    }

    fn sensor_spec() -> ModuleSpec {
        ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
            .requires(
                CapabilitySet::empty()
                    .with(Capability::Bus0)
                    .with(Capability::SamplePool),
            )
            .owns(CapabilitySet::empty().with(Capability::Bus0))
            .memory(MemoryBudget::new(8 * 1024, 2 * 1024, 2))
    }

    fn valid_manifest() -> SystemManifest<4> {
        let mut manifest = SystemManifest::<4>::new();
        manifest.add(kernel_spec()).unwrap();
        manifest.add(sensor_spec()).unwrap();
        manifest
    }

    #[test]
    fn admission_builds_startup_and_quota_plan() {
        let manifest = valid_manifest();
        let startup = [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty()),
            StartupNode::new(ModuleId::Sensor, DependencySet::empty().with_index(0)),
        ];

        let plan = AdmissionController::admit::<4, 4, 4>(&manifest, &startup, profile()).unwrap();

        assert_eq!(plan.module_count(), 2);
        assert_eq!(plan.startup.order[0], Some(ModuleId::Kernel));
        assert_eq!(plan.startup.order[1], Some(ModuleId::Sensor));
        assert_eq!(
            plan.quotas.limit(ModuleId::Sensor),
            Some(SystemBudget::new(8 * 1024, 2 * 1024, 2))
        );
        assert_eq!(plan.used, SystemBudget::new(24 * 1024, 6 * 1024, 6));
    }

    #[test]
    fn admission_rejects_missing_startup_node() {
        let manifest = valid_manifest();
        let startup = [StartupNode::new(ModuleId::Kernel, DependencySet::empty())];

        assert_eq!(
            AdmissionController::admit::<4, 4, 4>(&manifest, &startup, profile()).unwrap_err(),
            AdmissionError::MissingStartupNode(ModuleId::Sensor)
        );
    }

    #[test]
    fn admission_rejects_unknown_startup_node() {
        let manifest = valid_manifest();
        let startup = [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty()),
            StartupNode::new(ModuleId::Sensor, DependencySet::empty().with_index(0)),
            StartupNode::new(ModuleId::App(9), DependencySet::empty()),
        ];

        assert_eq!(
            AdmissionController::admit::<4, 4, 4>(&manifest, &startup, profile()).unwrap_err(),
            AdmissionError::UnknownStartupNode(ModuleId::App(9))
        );
    }

    #[test]
    fn admission_propagates_manifest_budget_failure() {
        let manifest = valid_manifest();
        let startup = [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty()),
            StartupNode::new(ModuleId::Sensor, DependencySet::empty().with_index(0)),
        ];
        let tiny_profile = SystemProfile {
            flash_limit_bytes: 4 * 1024,
            ..profile()
        };

        assert!(matches!(
            AdmissionController::admit::<4, 4, 4>(&manifest, &startup, tiny_profile),
            Err(AdmissionError::Manifest(
                ManifestError::BudgetExceeded { .. }
            ))
        ));
    }

    #[test]
    fn admission_propagates_startup_cycle_failure() {
        let manifest = valid_manifest();
        let startup = [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty().with_index(1)),
            StartupNode::new(ModuleId::Sensor, DependencySet::empty().with_index(0)),
        ];

        assert_eq!(
            AdmissionController::admit::<4, 4, 4>(&manifest, &startup, profile()).unwrap_err(),
            AdmissionError::Startup(StartupError::Cycle)
        );
    }
}
