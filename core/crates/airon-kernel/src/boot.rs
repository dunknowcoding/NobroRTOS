//! Software boot assembly helpers for fixed-capacity applications.

use crate::{
    AdmissionController, AdmissionError, AdmissionReport, FaultThresholds, ManifestError,
    ManifestReport, ModuleId, ModuleSpec, Runtime, RuntimeError, StartupGraph, StartupGraphError,
    SystemManifest, SystemProfile,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartupDependency {
    pub module: ModuleId,
    pub depends_on: ModuleId,
}

impl StartupDependency {
    pub const fn new(module: ModuleId, depends_on: ModuleId) -> Self {
        Self { module, depends_on }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootAssemblyError {
    Manifest(ManifestError),
    StartupGraph(StartupGraphError),
    Admission(AdmissionError),
    Runtime(RuntimeError),
}

impl From<ManifestError> for BootAssemblyError {
    fn from(error: ManifestError) -> Self {
        Self::Manifest(error)
    }
}

impl From<StartupGraphError> for BootAssemblyError {
    fn from(error: StartupGraphError) -> Self {
        Self::StartupGraph(error)
    }
}

impl From<AdmissionError> for BootAssemblyError {
    fn from(error: AdmissionError) -> Self {
        Self::Admission(error)
    }
}

impl From<RuntimeError> for BootAssemblyError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

pub struct BootAssembly<
    const MODULES: usize,
    const GRAPH: usize,
    const STARTUP: usize,
    const QUOTAS: usize,
    const MAILBOX: usize,
    const ALARMS: usize,
    const KV: usize,
    const HEALTH: usize,
    const LOG: usize,
> {
    pub manifest: SystemManifest<MODULES>,
    pub startup: StartupGraph<GRAPH>,
    pub runtime: Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
    pub manifest_report: ManifestReport,
    pub admission_report: AdmissionReport,
}

impl<
        const MODULES: usize,
        const GRAPH: usize,
        const STARTUP: usize,
        const QUOTAS: usize,
        const MAILBOX: usize,
        const ALARMS: usize,
        const KV: usize,
        const HEALTH: usize,
        const LOG: usize,
    > BootAssembly<MODULES, GRAPH, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>
{
    pub fn build(
        specs: &[ModuleSpec],
        dependencies: &[StartupDependency],
        profile: SystemProfile,
        thresholds: FaultThresholds,
        now_us: u64,
    ) -> Result<Self, BootAssemblyError> {
        let manifest = SystemManifest::<MODULES>::from_specs(specs)?;
        let manifest_result = manifest.validate_profile(profile);
        let manifest_report = ManifestReport::from_result(&manifest, manifest_result);
        manifest_result?;

        let mut startup = manifest.startup_graph::<GRAPH>()?;
        for dependency in dependencies {
            startup.add_dependency(dependency.module, dependency.depends_on)?;
        }

        let admission = AdmissionController::admit_graph::<MODULES, GRAPH, STARTUP, QUOTAS>(
            &manifest, &startup, profile,
        );
        let admission_report = AdmissionReport::from_result(admission.as_ref().map_err(|e| *e));
        let plan = admission?;
        let mut runtime = Runtime::from_plan(plan, thresholds)?;
        runtime.boot_to_running(now_us)?;

        Ok(Self {
            manifest,
            startup,
            runtime,
            manifest_report,
            admission_report,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        kernel_module_spec, kernel_owned_capabilities, Capability, CapabilitySet, Criticality,
        DeadlineContract, MemoryBudget, ModuleRunState,
    };

    type TestAssembly = BootAssembly<4, 4, 4, 4, 4, 4, 4, 4, 8>;

    fn profile() -> SystemProfile {
        SystemProfile::new(64 * 1024, 16 * 1024, 8, 4)
    }

    fn kernel_spec() -> ModuleSpec {
        kernel_module_spec(
            MemoryBudget::new(16 * 1024, 4 * 1024, 4),
            DeadlineContract::new(20_000, 10),
        )
    }

    fn bus_spec() -> ModuleSpec {
        ModuleSpec::new(ModuleId::Bus, Criticality::Driver)
            .owns(CapabilitySet::empty().with(Capability::Bus0))
            .memory(MemoryBudget::new(4 * 1024, 1024, 0))
    }

    fn sensor_spec() -> ModuleSpec {
        ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
            .requires(
                CapabilitySet::empty()
                    .with(Capability::Bus0)
                    .with(Capability::SamplePool),
            )
            .memory(MemoryBudget::new(8 * 1024, 2 * 1024, 2))
    }

    #[test]
    fn boot_assembly_builds_manifest_admission_reports_and_running_runtime() {
        let specs = [kernel_spec(), bus_spec(), sensor_spec()];
        let deps = [
            StartupDependency::new(ModuleId::Bus, ModuleId::Kernel),
            StartupDependency::new(ModuleId::Sensor, ModuleId::Bus),
        ];

        let assembly =
            TestAssembly::build(&specs, &deps, profile(), FaultThresholds::DEFAULT, 42).unwrap();

        assert_eq!(assembly.manifest.len(), 3);
        assert_eq!(assembly.startup.len(), 3);
        assert!(assembly.manifest_report.verify_checksum());
        assert_eq!(assembly.manifest_report.valid, 1);
        assert!(assembly.admission_report.verify_checksum());
        assert_eq!(assembly.admission_report.admitted, 1);
        assert_eq!(assembly.runtime.state(), crate::SystemState::Running);
        assert_eq!(
            assembly
                .runtime
                .module_runtime_entry(ModuleId::Sensor)
                .unwrap()
                .state,
            ModuleRunState::Active
        );
    }

    #[test]
    fn boot_assembly_preserves_manifest_errors() {
        let specs = [
            kernel_spec(),
            ModuleSpec::new(ModuleId::App(1), Criticality::User)
                .owns(kernel_owned_capabilities())
                .memory(MemoryBudget::new(1024, 1024, 0)),
        ];

        let error = match TestAssembly::build(&specs, &[], profile(), FaultThresholds::DEFAULT, 0) {
            Ok(_) => panic!("boot assembly unexpectedly succeeded"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            BootAssemblyError::Manifest(ManifestError::CapabilityOwnershipConflict {
                module: ModuleId::App(1),
                ..
            })
        ));
    }

    #[test]
    fn boot_assembly_preserves_startup_dependency_errors() {
        let specs = [kernel_spec(), bus_spec()];
        let deps = [StartupDependency::new(ModuleId::Sensor, ModuleId::Bus)];

        let error = match TestAssembly::build(&specs, &deps, profile(), FaultThresholds::DEFAULT, 0)
        {
            Ok(_) => panic!("boot assembly unexpectedly succeeded"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            BootAssemblyError::StartupGraph(StartupGraphError::UnknownModule(ModuleId::Sensor))
        );
    }
}
