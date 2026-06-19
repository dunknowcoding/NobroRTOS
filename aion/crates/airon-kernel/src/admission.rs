//! Boot admission checks that compose manifest, startup, and quota contracts.

use crate::{
    CapabilityGrantError, CapabilityGrantTable, ManifestError, ModuleId, QuotaError, QuotaLedger,
    StartupError, StartupNode, StartupPlan, StartupPlanner, SystemBudget, SystemManifest,
    SystemProfile,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    Manifest(ManifestError),
    Startup(StartupError),
    Quota(QuotaError),
    Capability(CapabilityGrantError),
    MissingStartupNode(ModuleId),
    UnknownStartupNode(ModuleId),
}

impl AdmissionError {
    pub fn code(self) -> u32 {
        match self {
            Self::Manifest(_) => 1,
            Self::Startup(_) => 2,
            Self::Quota(_) => 3,
            Self::Capability(_) => 4,
            Self::MissingStartupNode(_) => 5,
            Self::UnknownStartupNode(_) => 6,
        }
    }
}

#[derive(Debug)]
pub struct AdmissionPlan<const STARTUP: usize, const QUOTAS: usize> {
    pub startup: StartupPlan<STARTUP>,
    pub quotas: QuotaLedger<QUOTAS>,
    pub grants: CapabilityGrantTable<QUOTAS>,
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
        let grants = CapabilityGrantTable::<QUOTAS>::from_manifest(manifest)
            .map_err(AdmissionError::Capability)?;

        Ok(AdmissionPlan {
            startup,
            quotas,
            grants,
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

pub const ADMISSION_REPORT_MAGIC: u32 = 0x4152_4144; // "ARAD"
pub const ADMISSION_REPORT_VERSION: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdmissionReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub admitted: u32,
    pub module_count: u32,
    pub startup_len: u32,
    pub flash_used_bytes: u32,
    pub flash_limit_bytes: u32,
    pub ram_used_bytes: u32,
    pub ram_limit_bytes: u32,
    pub pool_used_slots: u32,
    pub pool_limit_slots: u32,
    pub error_code: u32,
    pub checksum: u32,
}

impl AdmissionReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            admitted: 0,
            module_count: 0,
            startup_len: 0,
            flash_used_bytes: 0,
            flash_limit_bytes: 0,
            ram_used_bytes: 0,
            ram_limit_bytes: 0,
            pool_used_slots: 0,
            pool_limit_slots: 0,
            error_code: 0,
            checksum: 0,
        }
    }

    pub fn from_plan<const STARTUP: usize, const QUOTAS: usize>(
        plan: &AdmissionPlan<STARTUP, QUOTAS>,
    ) -> Self {
        let mut report = Self {
            admitted: 1,
            module_count: plan.module_count() as u32,
            startup_len: plan.startup.len as u32,
            flash_used_bytes: plan.used.flash_bytes,
            flash_limit_bytes: plan.profile.flash_limit_bytes,
            ram_used_bytes: plan.used.ram_bytes,
            ram_limit_bytes: plan.profile.ram_limit_bytes,
            pool_used_slots: u32::from(plan.used.pool_slots),
            pool_limit_slots: u32::from(plan.profile.pool_slot_limit),
            ..Self::zeroed()
        };
        report.seal();
        report
    }

    pub fn from_error(error: AdmissionError) -> Self {
        let mut report = Self {
            admitted: 0,
            error_code: error.code(),
            ..Self::zeroed()
        };
        report.seal();
        report
    }

    pub fn seal(&mut self) {
        self.magic = ADMISSION_REPORT_MAGIC;
        self.version = ADMISSION_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == ADMISSION_REPORT_MAGIC
            && self.version == ADMISSION_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.admitted
            ^ self.module_count
            ^ self.startup_len
            ^ self.flash_used_bytes
            ^ self.flash_limit_bytes
            ^ self.ram_used_bytes
            ^ self.ram_limit_bytes
            ^ self.pool_used_slots
            ^ self.pool_limit_slots
            ^ self.error_code
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
    fn admission_report_seals_successful_plan() {
        let manifest = valid_manifest();
        let startup = [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty()),
            StartupNode::new(ModuleId::Sensor, DependencySet::empty().with_index(0)),
        ];
        let plan = AdmissionController::admit::<4, 4, 4>(&manifest, &startup, profile()).unwrap();

        let report = AdmissionReport::from_plan(&plan);

        assert!(report.verify_checksum());
        assert_eq!(report.admitted, 1);
        assert_eq!(report.module_count, 2);
        assert_eq!(report.startup_len, 2);
        assert_eq!(report.flash_used_bytes, 24 * 1024);
        assert_eq!(report.error_code, 0);
    }

    #[test]
    fn admission_report_seals_failure_code() {
        let mut report =
            AdmissionReport::from_error(AdmissionError::UnknownStartupNode(ModuleId::App(1)));

        assert!(report.verify_checksum());
        assert_eq!(report.admitted, 0);
        assert_eq!(report.error_code, 6);

        report.error_code = 9;
        assert!(!report.verify_checksum());
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
    fn admission_plan_contains_capability_grants() {
        let manifest = valid_manifest();
        let startup = [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty()),
            StartupNode::new(ModuleId::Sensor, DependencySet::empty().with_index(0)),
        ];

        let plan = AdmissionController::admit::<4, 4, 4>(&manifest, &startup, profile()).unwrap();

        assert_eq!(
            plan.grants
                .authorize(ModuleId::Sensor, Capability::SamplePool),
            Ok(())
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
