//! NobroRTOS service abstraction layer with portable capability traits.

#![no_std]

use nobro_kernel::{
    module_tag, CapabilitySet, Criticality, KernelError, ModuleId, ModuleSpec, Sample,
    SystemBudget, SystemProfile,
};

pub const ADAPTER_COMPAT_REPORT_MAGIC: u32 = 0x4E42_4143; // "NBAC"
pub const ADAPTER_COMPAT_REPORT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterDescriptor {
    pub module: ModuleId,
    pub criticality: Criticality,
    pub requires_bits: u32,
    pub owns_bits: u32,
    pub budget: SystemBudget,
}

impl AdapterDescriptor {
    pub const fn from_module_spec(spec: ModuleSpec) -> Self {
        Self {
            module: spec.id,
            criticality: spec.criticality,
            requires_bits: spec.requires.bits(),
            owns_bits: spec.owns.bits(),
            budget: SystemBudget::from_memory(spec.memory),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterSetError {
    Full,
    DuplicateModule(ModuleId),
    CapabilityOwnershipConflict {
        module: ModuleId,
        capability_bits: u32,
    },
    ModuleLimitExceeded {
        modules: usize,
        limit: usize,
    },
    BudgetExceeded {
        used: SystemBudget,
        limit: SystemBudget,
    },
}

impl AdapterSetError {
    pub const fn code(self) -> u32 {
        match self {
            Self::Full => 1,
            Self::DuplicateModule(_) => 2,
            Self::CapabilityOwnershipConflict { .. } => 3,
            Self::ModuleLimitExceeded { .. } => 4,
            Self::BudgetExceeded { .. } => 5,
        }
    }

    pub const fn module(self) -> Option<ModuleId> {
        match self {
            Self::DuplicateModule(module) => Some(module),
            Self::CapabilityOwnershipConflict { module, .. } => Some(module),
            Self::Full | Self::ModuleLimitExceeded { .. } | Self::BudgetExceeded { .. } => None,
        }
    }

    pub const fn capability_bits(self) -> u32 {
        match self {
            Self::CapabilityOwnershipConflict {
                capability_bits, ..
            } => capability_bits,
            Self::Full
            | Self::DuplicateModule(_)
            | Self::ModuleLimitExceeded { .. }
            | Self::BudgetExceeded { .. } => 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdapterCompatibilityReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub compatible: u32,
    pub adapter_count: u32,
    pub required_bits: u32,
    pub owned_bits: u32,
    pub flash_used_bytes: u32,
    pub ram_used_bytes: u32,
    pub pool_used_slots: u32,
    pub error_code: u32,
    pub error_module_tag: u32,
    pub error_capability_bits: u32,
    pub checksum: u32,
}

impl AdapterCompatibilityReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            compatible: 0,
            adapter_count: 0,
            required_bits: 0,
            owned_bits: 0,
            flash_used_bytes: 0,
            ram_used_bytes: 0,
            pool_used_slots: 0,
            error_code: 0,
            error_module_tag: 0,
            error_capability_bits: 0,
            checksum: 0,
        }
    }

    pub fn from_result<const N: usize>(
        adapters: &AdapterSet<N>,
        result: Result<(), AdapterSetError>,
    ) -> Self {
        let budget = adapters.total_budget();
        let error = result.err();
        let mut report = Self {
            compatible: error.is_none() as u32,
            adapter_count: adapters.len() as u32,
            required_bits: adapters.required_capabilities().bits(),
            owned_bits: adapters.owned_capabilities().bits(),
            flash_used_bytes: budget.flash_bytes,
            ram_used_bytes: budget.ram_bytes,
            pool_used_slots: u32::from(budget.pool_slots),
            error_code: error.map(AdapterSetError::code).unwrap_or(0),
            ..Self::zeroed()
        };

        if let Some(error) = error {
            report.error_module_tag = error.module().map(module_tag).unwrap_or(0);
            report.error_capability_bits = error.capability_bits();
        }

        report.seal();
        report
    }

    pub fn seal(&mut self) {
        self.magic = ADAPTER_COMPAT_REPORT_MAGIC;
        self.version = ADAPTER_COMPAT_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == ADAPTER_COMPAT_REPORT_MAGIC
            && self.version == ADAPTER_COMPAT_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.compatible
            ^ self.adapter_count
            ^ self.required_bits
            ^ self.owned_bits
            ^ self.flash_used_bytes
            ^ self.ram_used_bytes
            ^ self.pool_used_slots
            ^ self.error_code
            ^ self.error_module_tag
            ^ self.error_capability_bits
    }
}

pub struct AdapterSet<const N: usize> {
    descriptors: [Option<AdapterDescriptor>; N],
}

impl<const N: usize> AdapterSet<N> {
    pub const fn new() -> Self {
        Self {
            descriptors: [None; N],
        }
    }

    pub fn add(&mut self, descriptor: AdapterDescriptor) -> Result<(), AdapterSetError> {
        if self
            .descriptors
            .iter()
            .flatten()
            .any(|existing| existing.module == descriptor.module)
        {
            return Err(AdapterSetError::DuplicateModule(descriptor.module));
        }

        let Some(slot) = self.descriptors.iter_mut().find(|slot| slot.is_none()) else {
            return Err(AdapterSetError::Full);
        };
        *slot = Some(descriptor);
        Ok(())
    }

    pub fn add_manifest<A: AdapterManifest>(&mut self) -> Result<(), AdapterSetError> {
        self.add(A::descriptor())
    }

    pub fn validate_profile(&self, profile: SystemProfile) -> Result<(), AdapterSetError> {
        if self.len() > profile.max_modules {
            return Err(AdapterSetError::ModuleLimitExceeded {
                modules: self.len(),
                limit: profile.max_modules,
            });
        }

        self.validate_ownership()?;

        let used = self.total_budget();
        let limit = profile.budget();
        if !used.fits_within(limit) {
            return Err(AdapterSetError::BudgetExceeded { used, limit });
        }

        Ok(())
    }

    pub fn compatibility_report(&self, profile: SystemProfile) -> AdapterCompatibilityReport {
        AdapterCompatibilityReport::from_result(self, self.validate_profile(profile))
    }

    pub fn descriptor(&self, module: ModuleId) -> Option<AdapterDescriptor> {
        self.descriptors
            .iter()
            .flatten()
            .find(|descriptor| descriptor.module == module)
            .copied()
    }

    pub fn copy_descriptors(&self, out: &mut [AdapterDescriptor]) -> usize {
        let mut written = 0;
        for descriptor in self.descriptors.iter().flatten() {
            if written >= out.len() {
                break;
            }
            out[written] = *descriptor;
            written += 1;
        }
        written
    }

    pub fn total_budget(&self) -> SystemBudget {
        let mut total = SystemBudget::ZERO;
        for descriptor in self.descriptors.iter().flatten() {
            total = total
                .checked_add(descriptor.budget)
                .unwrap_or(SystemBudget {
                    flash_bytes: u32::MAX,
                    ram_bytes: u32::MAX,
                    pool_slots: u16::MAX,
                });
        }
        total
    }

    pub fn owned_capabilities(&self) -> CapabilitySet {
        self.descriptors
            .iter()
            .flatten()
            .fold(CapabilitySet::empty(), |acc, descriptor| {
                acc.union(CapabilitySet::from_bits(descriptor.owns_bits))
            })
    }

    pub fn required_capabilities(&self) -> CapabilitySet {
        self.descriptors
            .iter()
            .flatten()
            .fold(CapabilitySet::empty(), |acc, descriptor| {
                acc.union(CapabilitySet::from_bits(descriptor.requires_bits))
            })
    }

    pub fn len(&self) -> usize {
        self.descriptors.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    fn validate_ownership(&self) -> Result<(), AdapterSetError> {
        let mut owned = CapabilitySet::empty();
        for descriptor in self.descriptors.iter().flatten() {
            let owns = CapabilitySet::from_bits(descriptor.owns_bits);
            if owns.intersects(owned) {
                return Err(AdapterSetError::CapabilityOwnershipConflict {
                    module: descriptor.module,
                    capability_bits: owns.intersection(owned).bits(),
                });
            }
            owned = owned.union(owns);
        }
        Ok(())
    }
}

pub struct AdapterPreflight<const N: usize> {
    adapters: AdapterSet<N>,
    first_error: Option<AdapterSetError>,
}

impl<const N: usize> AdapterPreflight<N> {
    pub const fn new() -> Self {
        Self {
            adapters: AdapterSet::new(),
            first_error: None,
        }
    }

    pub fn add(&mut self, descriptor: AdapterDescriptor) -> Result<(), AdapterSetError> {
        if let Some(error) = self.first_error {
            return Err(error);
        }

        let result = self.adapters.add(descriptor);
        if let Err(error) = result {
            self.first_error = Some(error);
        }
        result
    }

    pub fn add_manifest<A: AdapterManifest>(&mut self) -> Result<(), AdapterSetError> {
        self.add(A::descriptor())
    }

    pub fn compatibility_report(&self, profile: SystemProfile) -> AdapterCompatibilityReport {
        let result = match self.first_error {
            Some(error) => Err(error),
            None => self.adapters.validate_profile(profile),
        };
        AdapterCompatibilityReport::from_result(&self.adapters, result)
    }

    pub const fn first_error(&self) -> Option<AdapterSetError> {
        self.first_error
    }

    pub const fn adapters(&self) -> &AdapterSet<N> {
        &self.adapters
    }

    pub fn descriptor(&self, module: ModuleId) -> Option<AdapterDescriptor> {
        self.adapters.descriptor(module)
    }

    pub fn copy_descriptors(&self, out: &mut [AdapterDescriptor]) -> usize {
        self.adapters.copy_descriptors(out)
    }
}

impl<const N: usize> Default for AdapterPreflight<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Default for AdapterSet<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Static adapter admission data used by app assembly and boot checks.
pub trait AdapterManifest {
    fn module_spec() -> ModuleSpec;

    fn descriptor() -> AdapterDescriptor {
        AdapterDescriptor::from_module_spec(Self::module_spec())
    }
}

/// I2C / SPI / UART bus transactions with lease guard.
pub trait BusSal {
    type Error;

    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<(), Self::Error>;
    fn write(&mut self, addr: u8, buf: &[u8]) -> Result<(), Self::Error>;
}

/// Host-facing byte streams (IronEngine, INA JSONL, debug).
pub trait StreamSal {
    type Error;

    fn poll(&mut self) -> Result<Option<usize>, Self::Error>;
    fn read_frame(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error>;
    fn write_frame(&mut self, buf: &[u8]) -> Result<(), Self::Error>;
}

/// BLE / 802.15.4 radio pump with compile-time exclusive backends.
pub trait RadioSal {
    type Error;

    fn process(&mut self) -> Result<(), Self::Error>;
    fn rx_available(&self) -> bool;
}

/// Actuators: servo PWM, motor duty with deadline.
pub trait ActuatorSal {
    type Error;

    fn set_duty_us(
        &mut self,
        channel: u8,
        pulse_us: u32,
        deadline_us: u64,
    ) -> Result<(), Self::Error>;
}

/// Sensors return optional Sample tickets.
pub trait SensorSal {
    type Error;

    fn poll(&mut self) -> Result<Option<Sample>, Self::Error>;
}

/// Crypto hardware/software backend.
pub trait CryptoSal {
    type Error;

    fn random(&mut self, dest: &mut [u8]) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AiBackendKind {
    OnDevice = 1,
    RemoteApi = 2,
    EdgeSidecar = 3,
    Hybrid = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AiRoutePreference {
    LocalOnly = 1,
    PreferLocal = 2,
    PreferRemote = 3,
    HybridFallback = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AiRouteTarget {
    OnDevice = 1,
    RemoteApi = 2,
    EdgeSidecar = 3,
    StaleSnapshot = 4,
    DegradedFallback = 5,
    Unavailable = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRuntimeState {
    pub local_ready: bool,
    pub endpoint_ready: bool,
    pub last_success_age_us: u32,
    pub consecutive_endpoint_failures: u8,
}

impl AiRuntimeState {
    pub const fn new(
        local_ready: bool,
        endpoint_ready: bool,
        last_success_age_us: u32,
        consecutive_endpoint_failures: u8,
    ) -> Self {
        Self {
            local_ready,
            endpoint_ready,
            last_success_age_us,
            consecutive_endpoint_failures,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRouteDecision {
    pub target: AiRouteTarget,
    pub endpoint_circuit_open: bool,
    pub uses_stale_snapshot: bool,
}

impl AiRouteDecision {
    pub const fn new(
        target: AiRouteTarget,
        endpoint_circuit_open: bool,
        uses_stale_snapshot: bool,
    ) -> Self {
        Self {
            target,
            endpoint_circuit_open,
            uses_stale_snapshot,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiRoutePolicy {
    pub preference: AiRoutePreference,
    pub stale_after_us: u32,
    pub endpoint_failure_limit: u8,
}

impl AiRoutePolicy {
    pub const fn new(
        preference: AiRoutePreference,
        stale_after_us: u32,
        endpoint_failure_limit: u8,
    ) -> Self {
        Self {
            preference,
            stale_after_us,
            endpoint_failure_limit,
        }
    }

    pub fn decide(
        &self,
        contract: AiModelContract,
        state: AiRuntimeState,
        budget_us: u32,
    ) -> AiRouteDecision {
        let endpoint_failure_limit = if self.endpoint_failure_limit == 0 {
            1
        } else {
            self.endpoint_failure_limit
        };
        let endpoint_circuit_open = state.consecutive_endpoint_failures >= endpoint_failure_limit;
        let fits_budget = contract.timeout_us <= budget_us;
        let stale_ready = state.last_success_age_us <= self.stale_after_us;

        if !fits_budget {
            return self.fallback(endpoint_circuit_open, stale_ready);
        }

        match contract.backend {
            AiBackendKind::OnDevice => {
                if state.local_ready {
                    AiRouteDecision::new(AiRouteTarget::OnDevice, endpoint_circuit_open, false)
                } else {
                    self.fallback(endpoint_circuit_open, stale_ready)
                }
            }
            AiBackendKind::RemoteApi => self.remote_or_fallback(
                AiRouteTarget::RemoteApi,
                state,
                endpoint_circuit_open,
                stale_ready,
            ),
            AiBackendKind::EdgeSidecar => self.remote_or_fallback(
                AiRouteTarget::EdgeSidecar,
                state,
                endpoint_circuit_open,
                stale_ready,
            ),
            AiBackendKind::Hybrid => {
                self.hybrid_decision(state, endpoint_circuit_open, stale_ready)
            }
        }
    }

    fn remote_or_fallback(
        &self,
        target: AiRouteTarget,
        state: AiRuntimeState,
        endpoint_circuit_open: bool,
        stale_ready: bool,
    ) -> AiRouteDecision {
        if self.preference != AiRoutePreference::LocalOnly
            && state.endpoint_ready
            && !endpoint_circuit_open
        {
            AiRouteDecision::new(target, endpoint_circuit_open, false)
        } else {
            self.fallback(endpoint_circuit_open, stale_ready)
        }
    }

    fn hybrid_decision(
        &self,
        state: AiRuntimeState,
        endpoint_circuit_open: bool,
        stale_ready: bool,
    ) -> AiRouteDecision {
        match self.preference {
            AiRoutePreference::LocalOnly | AiRoutePreference::PreferLocal => {
                if state.local_ready {
                    AiRouteDecision::new(AiRouteTarget::OnDevice, endpoint_circuit_open, false)
                } else {
                    self.remote_or_fallback(
                        AiRouteTarget::RemoteApi,
                        state,
                        endpoint_circuit_open,
                        stale_ready,
                    )
                }
            }
            AiRoutePreference::PreferRemote | AiRoutePreference::HybridFallback => {
                if state.endpoint_ready && !endpoint_circuit_open {
                    AiRouteDecision::new(AiRouteTarget::RemoteApi, endpoint_circuit_open, false)
                } else if state.local_ready {
                    AiRouteDecision::new(AiRouteTarget::OnDevice, endpoint_circuit_open, false)
                } else {
                    self.fallback(endpoint_circuit_open, stale_ready)
                }
            }
        }
    }

    fn fallback(&self, endpoint_circuit_open: bool, stale_ready: bool) -> AiRouteDecision {
        if stale_ready {
            AiRouteDecision::new(AiRouteTarget::StaleSnapshot, endpoint_circuit_open, true)
        } else if self.preference == AiRoutePreference::LocalOnly {
            AiRouteDecision::new(AiRouteTarget::Unavailable, endpoint_circuit_open, false)
        } else {
            AiRouteDecision::new(
                AiRouteTarget::DegradedFallback,
                endpoint_circuit_open,
                false,
            )
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiModelContract {
    pub backend: AiBackendKind,
    pub model_id: u32,
    pub input_bytes_max: u16,
    pub output_bytes_max: u16,
    pub arena_bytes: u32,
    pub timeout_us: u32,
}

impl AiModelContract {
    pub const fn new(
        backend: AiBackendKind,
        model_id: u32,
        input_bytes_max: u16,
        output_bytes_max: u16,
        arena_bytes: u32,
        timeout_us: u32,
    ) -> Self {
        Self {
            backend,
            model_id,
            input_bytes_max,
            output_bytes_max,
            arena_bytes,
            timeout_us,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiInferenceRequest<'a> {
    pub model_id: u32,
    pub input: &'a [u8],
    pub deadline_us: u64,
    pub flags: u32,
}

impl<'a> AiInferenceRequest<'a> {
    pub const fn new(model_id: u32, input: &'a [u8], deadline_us: u64) -> Self {
        Self {
            model_id,
            input,
            deadline_us,
            flags: 0,
        }
    }

    pub const fn with_flags(mut self, flags: u32) -> Self {
        self.flags = flags;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiInferenceResult {
    pub output_len: u16,
    pub confidence_q15: u16,
    pub latency_us: u32,
    pub flags: u32,
}

impl AiInferenceResult {
    pub const fn new(output_len: u16, confidence_q15: u16, latency_us: u32) -> Self {
        Self {
            output_len,
            confidence_q15,
            latency_us,
            flags: 0,
        }
    }

    pub const fn with_flags(mut self, flags: u32) -> Self {
        self.flags = flags;
        self
    }
}

/// Bounded AI inference session for on-device, remote API, or edge-sidecar backends.
pub trait AiInferenceSal {
    type Error;

    fn contract(&self) -> AiModelContract;
    fn infer(
        &mut self,
        request: AiInferenceRequest<'_>,
        output: &mut [u8],
    ) -> Result<AiInferenceResult, Self::Error>;
}

/// Map kernel errors to actions (registered per adapter in later phases).
pub fn default_action(err: &KernelError) -> nobro_kernel::Action {
    use nobro_kernel::Action::*;
    match err {
        KernelError::LeaseConflict => Ignore,
        KernelError::BusTimeout => RetryDelay(1000),
        KernelError::RadioTxFail => RetryDelay(1000),
        KernelError::SensorReadFail => Ignore,
        KernelError::DeadlineMissed => NotifyUserTask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nobro_kernel::{Capability, CapabilitySet, MemoryBudget};

    struct FakeAdapter;

    impl AdapterManifest for FakeAdapter {
        fn module_spec() -> ModuleSpec {
            ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
                .requires(CapabilitySet::empty().with(Capability::SamplePool))
                .owns(CapabilitySet::empty().with(Capability::Bus0))
                .memory(MemoryBudget::new(2048, 512, 2))
        }
    }

    struct FakeAiAdapter;

    impl AdapterManifest for FakeAiAdapter {
        fn module_spec() -> ModuleSpec {
            ModuleSpec::new(ModuleId::Ai, Criticality::User)
                .requires(
                    CapabilitySet::empty()
                        .with(Capability::AiInference)
                        .with(Capability::AiEndpoint)
                        .with(Capability::Stream),
                )
                .owns(CapabilitySet::empty().with(Capability::AiEndpoint))
                .memory(MemoryBudget::new(16 * 1024, 6 * 1024, 1))
        }
    }

    struct EchoAi;

    impl AiInferenceSal for EchoAi {
        type Error = ();

        fn contract(&self) -> AiModelContract {
            AiModelContract::new(AiBackendKind::OnDevice, 42, 8, 8, 4096, 20_000)
        }

        fn infer(
            &mut self,
            request: AiInferenceRequest<'_>,
            output: &mut [u8],
        ) -> Result<AiInferenceResult, Self::Error> {
            if request.input.len() > output.len() {
                return Err(());
            }

            output[..request.input.len()].copy_from_slice(request.input);
            Ok(AiInferenceResult::new(
                request.input.len() as u16,
                0x7FFF,
                120,
            ))
        }
    }

    #[test]
    fn adapter_descriptor_is_derived_from_module_spec() {
        let descriptor = FakeAdapter::descriptor();

        assert_eq!(descriptor.module, ModuleId::Sensor);
        assert_eq!(descriptor.criticality, Criticality::Driver);
        assert_eq!(descriptor.requires_bits, Capability::SamplePool.bit());
        assert_eq!(descriptor.owns_bits, Capability::Bus0.bit());
        assert_eq!(descriptor.budget, SystemBudget::new(2048, 512, 2));
    }

    #[test]
    fn ai_adapter_declares_inference_and_endpoint_contracts() {
        let descriptor = FakeAiAdapter::descriptor();

        assert_eq!(descriptor.module, ModuleId::Ai);
        assert_eq!(descriptor.criticality, Criticality::User);
        assert_eq!(
            descriptor.requires_bits,
            Capability::AiInference.bit() | Capability::AiEndpoint.bit() | Capability::Stream.bit()
        );
        assert_eq!(descriptor.owns_bits, Capability::AiEndpoint.bit());
        assert_eq!(descriptor.budget, SystemBudget::new(16 * 1024, 6 * 1024, 1));
    }

    #[test]
    fn ai_inference_sal_uses_caller_owned_buffers() {
        let mut ai = EchoAi;
        let input = [1, 2, 3, 4];
        let mut output = [0; 8];
        let contract = ai.contract();
        let result = ai
            .infer(AiInferenceRequest::new(42, &input, 10_000), &mut output)
            .unwrap();

        assert_eq!(contract.backend, AiBackendKind::OnDevice);
        assert_eq!(contract.arena_bytes, 4096);
        assert_eq!(result.output_len, 4);
        assert_eq!(result.confidence_q15, 0x7FFF);
        assert_eq!(&output[..4], &input);
    }

    #[test]
    fn ai_route_policy_keeps_hard_local_only_work_on_device() {
        let policy = AiRoutePolicy::new(AiRoutePreference::LocalOnly, 10_000, 3);
        let contract = AiModelContract::new(AiBackendKind::Hybrid, 7, 16, 16, 4096, 5_000);
        let state = AiRuntimeState::new(true, true, 1_000, 0);

        let decision = policy.decide(contract, state, 6_000);

        assert_eq!(decision.target, AiRouteTarget::OnDevice);
        assert!(!decision.endpoint_circuit_open);
        assert!(!decision.uses_stale_snapshot);
    }

    #[test]
    fn ai_route_policy_trips_endpoint_to_fresh_snapshot() {
        let policy = AiRoutePolicy::new(AiRoutePreference::PreferRemote, 50_000, 2);
        let contract = AiModelContract::new(AiBackendKind::RemoteApi, 7, 32, 32, 0, 20_000);
        let state = AiRuntimeState::new(false, true, 10_000, 2);

        let decision = policy.decide(contract, state, 30_000);

        assert_eq!(decision.target, AiRouteTarget::StaleSnapshot);
        assert!(decision.endpoint_circuit_open);
        assert!(decision.uses_stale_snapshot);
    }

    #[test]
    fn ai_route_policy_uses_degraded_fallback_when_budget_is_too_small() {
        let policy = AiRoutePolicy::new(AiRoutePreference::HybridFallback, 1_000, 3);
        let contract = AiModelContract::new(AiBackendKind::EdgeSidecar, 7, 32, 32, 0, 20_000);
        let state = AiRuntimeState::new(false, true, 5_000, 0);

        let decision = policy.decide(contract, state, 5_000);

        assert_eq!(decision.target, AiRouteTarget::DegradedFallback);
        assert!(!decision.endpoint_circuit_open);
        assert!(!decision.uses_stale_snapshot);
    }

    #[test]
    fn ai_route_policy_treats_zero_failure_limit_as_one() {
        let policy = AiRoutePolicy::new(AiRoutePreference::PreferRemote, 50_000, 0);
        let contract = AiModelContract::new(AiBackendKind::RemoteApi, 7, 32, 32, 0, 20_000);
        let state = AiRuntimeState::new(false, true, 100, 1);

        let decision = policy.decide(contract, state, 30_000);

        assert_eq!(decision.target, AiRouteTarget::StaleSnapshot);
        assert!(decision.endpoint_circuit_open);
    }

    #[test]
    fn adapter_set_validates_budget_and_ownership() {
        let mut adapters = AdapterSet::<2>::new();
        adapters.add_manifest::<FakeAdapter>().unwrap();
        let actuator = AdapterDescriptor {
            module: ModuleId::Actuator,
            criticality: Criticality::HardRealtime,
            requires_bits: Capability::DeadlineTimer.bit(),
            owns_bits: Capability::ServoPwm.bit(),
            budget: SystemBudget::new(1024, 256, 0),
        };
        adapters
            .add(AdapterDescriptor {
                module: ModuleId::Actuator,
                criticality: Criticality::HardRealtime,
                requires_bits: Capability::DeadlineTimer.bit(),
                owns_bits: Capability::ServoPwm.bit(),
                budget: SystemBudget::new(1024, 256, 0),
            })
            .unwrap();

        assert_eq!(adapters.len(), 2);
        assert_eq!(adapters.capacity(), 2);
        assert_eq!(adapters.descriptor(ModuleId::Actuator), Some(actuator));
        assert_eq!(adapters.descriptor(ModuleId::Radio), None);
        assert_eq!(adapters.total_budget(), SystemBudget::new(3072, 768, 2));
        assert!(adapters
            .required_capabilities()
            .contains(Capability::SamplePool));
        assert!(adapters.owned_capabilities().contains(Capability::ServoPwm));
        assert!(adapters
            .validate_profile(SystemProfile::new(4096, 1024, 4, 2))
            .is_ok());

        let mut copied = [FakeAdapter::descriptor(); 1];
        assert_eq!(adapters.copy_descriptors(&mut copied), 1);
        assert_eq!(copied[0], FakeAdapter::descriptor());
    }

    #[test]
    fn adapter_set_rejects_duplicate_modules() {
        let mut adapters = AdapterSet::<2>::new();
        adapters.add_manifest::<FakeAdapter>().unwrap();

        assert_eq!(
            adapters.add_manifest::<FakeAdapter>(),
            Err(AdapterSetError::DuplicateModule(ModuleId::Sensor))
        );
    }

    #[test]
    fn adapter_set_rejects_capability_ownership_conflicts() {
        let mut adapters = AdapterSet::<2>::new();
        adapters.add_manifest::<FakeAdapter>().unwrap();
        adapters
            .add(AdapterDescriptor {
                module: ModuleId::Bus,
                criticality: Criticality::Driver,
                requires_bits: 0,
                owns_bits: Capability::Bus0.bit(),
                budget: SystemBudget::new(512, 128, 0),
            })
            .unwrap();

        assert_eq!(
            adapters.validate_profile(SystemProfile::new(4096, 1024, 4, 2)),
            Err(AdapterSetError::CapabilityOwnershipConflict {
                module: ModuleId::Bus,
                capability_bits: Capability::Bus0.bit(),
            })
        );
    }

    #[test]
    fn adapter_set_rejects_profile_over_budget() {
        let mut adapters = AdapterSet::<1>::new();
        adapters.add_manifest::<FakeAdapter>().unwrap();

        assert_eq!(
            adapters.validate_profile(SystemProfile::new(1024, 1024, 4, 1)),
            Err(AdapterSetError::BudgetExceeded {
                used: SystemBudget::new(2048, 512, 2),
                limit: SystemBudget::new(1024, 1024, 4),
            })
        );
    }

    #[test]
    fn adapter_compatibility_report_seals_success() {
        let mut adapters = AdapterSet::<2>::new();
        adapters.add_manifest::<FakeAdapter>().unwrap();

        let report = adapters.compatibility_report(SystemProfile::new(4096, 1024, 4, 2));

        assert!(report.verify_checksum());
        assert_eq!(report.magic, ADAPTER_COMPAT_REPORT_MAGIC);
        assert_eq!(report.version, ADAPTER_COMPAT_REPORT_VERSION);
        assert_eq!(report.compatible, 1);
        assert_eq!(report.adapter_count, 1);
        assert_eq!(report.required_bits, Capability::SamplePool.bit());
        assert_eq!(report.owned_bits, Capability::Bus0.bit());
        assert_eq!(report.flash_used_bytes, 2048);
        assert_eq!(report.ram_used_bytes, 512);
        assert_eq!(report.pool_used_slots, 2);
        assert_eq!(report.error_code, 0);
    }

    #[test]
    fn adapter_compatibility_report_preserves_failure_context() {
        let mut adapters = AdapterSet::<2>::new();
        adapters.add_manifest::<FakeAdapter>().unwrap();
        adapters
            .add(AdapterDescriptor {
                module: ModuleId::Bus,
                criticality: Criticality::Driver,
                requires_bits: 0,
                owns_bits: Capability::Bus0.bit(),
                budget: SystemBudget::new(512, 128, 0),
            })
            .unwrap();

        let report = adapters.compatibility_report(SystemProfile::new(4096, 1024, 4, 2));

        assert!(report.verify_checksum());
        assert_eq!(report.compatible, 0);
        assert_eq!(
            report.error_code,
            AdapterSetError::CapabilityOwnershipConflict {
                module: ModuleId::Bus,
                capability_bits: Capability::Bus0.bit()
            }
            .code()
        );
        assert_eq!(report.error_module_tag, module_tag(ModuleId::Bus));
        assert_eq!(report.error_capability_bits, Capability::Bus0.bit());
    }

    #[test]
    fn adapter_preflight_reports_add_errors() {
        let mut preflight = AdapterPreflight::<1>::new();
        preflight.add_manifest::<FakeAdapter>().unwrap();

        let duplicate = preflight.add_manifest::<FakeAdapter>();
        let report = preflight.compatibility_report(SystemProfile::new(4096, 1024, 4, 2));

        assert_eq!(
            duplicate,
            Err(AdapterSetError::DuplicateModule(ModuleId::Sensor))
        );
        assert_eq!(
            preflight.first_error(),
            Some(AdapterSetError::DuplicateModule(ModuleId::Sensor))
        );
        assert!(report.verify_checksum());
        assert_eq!(report.compatible, 0);
        assert_eq!(
            report.error_code,
            AdapterSetError::DuplicateModule(ModuleId::Sensor).code()
        );
        assert_eq!(report.error_module_tag, module_tag(ModuleId::Sensor));
        assert_eq!(report.adapter_count, 1);
        assert_eq!(
            preflight.descriptor(ModuleId::Sensor),
            Some(FakeAdapter::descriptor())
        );

        let mut copied = [AdapterDescriptor {
            module: ModuleId::Kernel,
            criticality: Criticality::System,
            requires_bits: 0,
            owns_bits: 0,
            budget: SystemBudget::ZERO,
        }];
        assert_eq!(preflight.copy_descriptors(&mut copied), 1);
        assert_eq!(copied[0], FakeAdapter::descriptor());
    }
}
