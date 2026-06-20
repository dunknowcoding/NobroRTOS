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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootAssemblyFailure {
    pub error: BootAssemblyError,
    pub manifest_report: ManifestReport,
    pub admission_report: AdmissionReport,
}

impl BootAssemblyFailure {
    pub const fn new(error: BootAssemblyError) -> Self {
        Self {
            error,
            manifest_report: ManifestReport::zeroed(),
            admission_report: AdmissionReport::zeroed(),
        }
    }

    pub const fn with_manifest(error: BootAssemblyError, manifest_report: ManifestReport) -> Self {
        Self {
            error,
            manifest_report,
            admission_report: AdmissionReport::zeroed(),
        }
    }

    pub const fn with_reports(
        error: BootAssemblyError,
        manifest_report: ManifestReport,
        admission_report: AdmissionReport,
    ) -> Self {
        Self {
            error,
            manifest_report,
            admission_report,
        }
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
        Self::build_with_failure(specs, dependencies, profile, thresholds, now_us)
            .map_err(|failure| failure.error)
    }

    pub fn build_with_failure(
        specs: &[ModuleSpec],
        dependencies: &[StartupDependency],
        profile: SystemProfile,
        thresholds: FaultThresholds,
        now_us: u64,
    ) -> Result<Self, BootAssemblyFailure> {
        let manifest = SystemManifest::<MODULES>::from_specs(specs)
            .map_err(|error| BootAssemblyFailure::new(error.into()))?;
        let manifest_result = manifest.validate_profile(profile);
        let manifest_report = ManifestReport::from_result(&manifest, manifest_result);
        if let Err(error) = manifest_result {
            return Err(BootAssemblyFailure::with_manifest(
                error.into(),
                manifest_report,
            ));
        }

        let mut startup = manifest
            .startup_graph::<GRAPH>()
            .map_err(|error| BootAssemblyFailure::with_manifest(error.into(), manifest_report))?;
        for dependency in dependencies {
            startup
                .add_dependency(dependency.module, dependency.depends_on)
                .map_err(|error| {
                    BootAssemblyFailure::with_manifest(error.into(), manifest_report)
                })?;
        }

        let admission = AdmissionController::admit_graph::<MODULES, GRAPH, STARTUP, QUOTAS>(
            &manifest, &startup, profile,
        );
        let admission_report = AdmissionReport::from_result(admission.as_ref().map_err(|e| *e));
        let plan = match admission {
            Ok(plan) => plan,
            Err(error) => {
                return Err(BootAssemblyFailure::with_reports(
                    error.into(),
                    manifest_report,
                    admission_report,
                ));
            }
        };
        let mut runtime = Runtime::from_plan(plan, thresholds).map_err(|error| {
            BootAssemblyFailure::with_reports(error.into(), manifest_report, admission_report)
        })?;
        runtime.boot_to_running(now_us).map_err(|error| {
            BootAssemblyFailure::with_reports(error.into(), manifest_report, admission_report)
        })?;

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
    fn boot_assembly_failure_preserves_manifest_report() {
        let specs = [
            kernel_spec(),
            ModuleSpec::new(ModuleId::App(1), Criticality::User)
                .owns(kernel_owned_capabilities())
                .memory(MemoryBudget::new(1024, 1024, 0)),
        ];

        let failure = match TestAssembly::build_with_failure(
            &specs,
            &[],
            profile(),
            FaultThresholds::DEFAULT,
            0,
        ) {
            Ok(_) => panic!("boot assembly unexpectedly succeeded"),
            Err(failure) => failure,
        };

        assert!(matches!(
            failure.error,
            BootAssemblyError::Manifest(ManifestError::CapabilityOwnershipConflict {
                module: ModuleId::App(1),
                ..
            })
        ));
        assert!(failure.manifest_report.verify_checksum());
        assert_eq!(failure.manifest_report.valid, 0);
        assert_eq!(failure.admission_report, AdmissionReport::zeroed());
    }

    #[test]
    fn boot_assembly_failure_preserves_admission_report() {
        let specs = [kernel_spec(), bus_spec(), sensor_spec()];
        let deps = [
            StartupDependency::new(ModuleId::Kernel, ModuleId::Sensor),
            StartupDependency::new(ModuleId::Sensor, ModuleId::Kernel),
        ];

        let failure = match TestAssembly::build_with_failure(
            &specs,
            &deps,
            profile(),
            FaultThresholds::DEFAULT,
            0,
        ) {
            Ok(_) => panic!("boot assembly unexpectedly succeeded"),
            Err(failure) => failure,
        };

        assert!(matches!(
            failure.error,
            BootAssemblyError::Admission(AdmissionError::Startup(crate::StartupError::Cycle))
        ));
        assert!(failure.manifest_report.verify_checksum());
        assert_eq!(failure.manifest_report.valid, 1);
        assert!(failure.admission_report.verify_checksum());
        assert_eq!(failure.admission_report.admitted, 0);
        assert_eq!(failure.admission_report.error_code, 2);
    }

    #[test]
    fn boot_assembly_preserves_legacy_error_return() {
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
