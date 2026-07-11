//! NobroRTOS service abstraction layer with portable capability traits.

#![no_std]

use nobro_kernel::{
    module_tag, CapabilitySet, Criticality, KernelError, ModuleId, ModuleSpec, Sample,
    SystemBudget, SystemProfile,
};

pub const ADAPTER_COMPAT_REPORT_MAGIC: u32 = 0x4E42_4143; // "NBAC"
pub const ADAPTER_COMPAT_REPORT_VERSION: u32 = 1;
pub const AI_MODEL_REPORT_MAGIC: u32 = 0x4E42_4149; // "NBAI"
pub const AI_MODEL_REPORT_VERSION: u32 = 1;
pub const ROS_BRIDGE_REPORT_MAGIC: u32 = 0x4E42_5253; // "NBRS"
pub const ROS_BRIDGE_REPORT_VERSION: u32 = 1;

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

/// One IMU reading in category-level units (backend-independent).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuSample {
    /// Acceleration per axis in milli-g.
    pub accel_mg: [i32; 3],
    /// Acceleration magnitude in milli-g (~1000 at rest).
    pub accel_mag_mg: u32,
}

/// The IMU **category** interface of the Universal Driver Interface: one trait per
/// sensor category, parts described as catalog data (`nobro_device::SensorDescriptor`),
/// and N mountable backends per part - native HAL driver, an `embedded-hal` driver, a
/// C/C++ module, or an Arduino-library shim - all selected by cargo feature, all
/// producing the same category units. App code written against `ImuSal` never names a
/// transport or a vendor library, so swapping the backend cannot change the app.
pub trait ImuSal {
    type Error;

    /// The part's identity register value (e.g. MPU-9250 WHO_AM_I = 0x71), used with
    /// the catalog descriptor to confirm the right silicon before trusting samples.
    fn who_am_i(&mut self) -> Result<u8, Self::Error>;
    /// One blocking category-level sample.
    fn sample(&mut self) -> Result<ImuSample, Self::Error>;
}

/// The temperature **category** of the Universal Driver Interface (second category,
/// same rule as [`ImuSal`]): whatever the part - an IMU die sensor, a BMP280, a
/// thermocouple front-end - a backend reports centi-degrees-Celsius, so app code and
/// plausibility checks are part-independent.
pub trait TempSal {
    type Error;

    /// One blocking temperature read in centi-degrees Celsius (2534 = 25.34 C).
    fn read_temp_centi_c(&mut self) -> Result<i32, Self::Error>;
}

/// Crypto hardware/software backend.
pub trait CryptoSal {
    type Error;

    fn random(&mut self, dest: &mut [u8]) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RosBridgeTransport {
    Serial = 1,
    Udp = 2,
    Radio = 3,
    SharedMemory = 4,
    Custom = 255,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RosTopicContract {
    pub name_hash: u32,
    pub message_type_hash: u32,
    pub depth: u8,
    pub max_message_bytes: u16,
}

impl RosTopicContract {
    pub const fn new(
        name_hash: u32,
        message_type_hash: u32,
        depth: u8,
        max_message_bytes: u16,
    ) -> Self {
        Self {
            name_hash,
            message_type_hash,
            depth,
            max_message_bytes,
        }
    }

    pub fn buffer_bytes(self) -> u32 {
        u32::from(self.depth).saturating_mul(u32::from(self.max_message_bytes))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RosServiceContract {
    pub name_hash: u32,
    pub request_bytes_max: u16,
    pub response_bytes_max: u16,
    pub timeout_us: u32,
}

impl RosServiceContract {
    pub const fn new(
        name_hash: u32,
        request_bytes_max: u16,
        response_bytes_max: u16,
        timeout_us: u32,
    ) -> Self {
        Self {
            name_hash,
            request_bytes_max,
            response_bytes_max,
            timeout_us,
        }
    }

    pub fn buffer_bytes(self) -> u32 {
        u32::from(self.request_bytes_max).saturating_add(u32::from(self.response_bytes_max))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RosActionContract {
    pub name_hash: u32,
    pub goal_bytes_max: u16,
    pub feedback_bytes_max: u16,
    pub result_bytes_max: u16,
    pub timeout_us: u32,
}

impl RosActionContract {
    pub const fn new(
        name_hash: u32,
        goal_bytes_max: u16,
        feedback_bytes_max: u16,
        result_bytes_max: u16,
        timeout_us: u32,
    ) -> Self {
        Self {
            name_hash,
            goal_bytes_max,
            feedback_bytes_max,
            result_bytes_max,
            timeout_us,
        }
    }

    pub fn buffer_bytes(self) -> u32 {
        u32::from(self.goal_bytes_max)
            .saturating_add(u32::from(self.feedback_bytes_max))
            .saturating_add(u32::from(self.result_bytes_max))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RosParameterContract {
    pub name_hash: u32,
    pub value_bytes_max: u16,
}

impl RosParameterContract {
    pub const fn new(name_hash: u32, value_bytes_max: u16) -> Self {
        Self {
            name_hash,
            value_bytes_max,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RosBridgeContract {
    pub transport: RosBridgeTransport,
    pub bridge_id_hash: u32,
    pub topic_count: u8,
    pub service_count: u8,
    pub action_count: u8,
    pub parameter_count: u8,
    pub total_buffer_bytes: u32,
    pub max_timeout_us: u32,
}

impl RosBridgeContract {
    #[allow(clippy::too_many_arguments)] // one-shot const constructor mirroring the ABI struct
    pub const fn new(
        transport: RosBridgeTransport,
        bridge_id_hash: u32,
        topic_count: u8,
        service_count: u8,
        action_count: u8,
        parameter_count: u8,
        total_buffer_bytes: u32,
        max_timeout_us: u32,
    ) -> Self {
        Self {
            transport,
            bridge_id_hash,
            topic_count,
            service_count,
            action_count,
            parameter_count,
            total_buffer_bytes,
            max_timeout_us,
        }
    }

    pub fn from_parts(
        transport: RosBridgeTransport,
        bridge_id_hash: u32,
        topics: &[RosTopicContract],
        services: &[RosServiceContract],
        actions: &[RosActionContract],
        parameters: &[RosParameterContract],
    ) -> Self {
        let mut total_buffer_bytes = 0u32;
        let mut max_timeout_us = 0u32;

        for topic in topics {
            total_buffer_bytes = total_buffer_bytes.saturating_add(topic.buffer_bytes());
        }
        for service in services {
            total_buffer_bytes = total_buffer_bytes.saturating_add(service.buffer_bytes());
            max_timeout_us = max_timeout_us.max(service.timeout_us);
        }
        for action in actions {
            total_buffer_bytes = total_buffer_bytes.saturating_add(action.buffer_bytes());
            max_timeout_us = max_timeout_us.max(action.timeout_us);
        }
        for parameter in parameters {
            total_buffer_bytes =
                total_buffer_bytes.saturating_add(u32::from(parameter.value_bytes_max));
        }

        Self::new(
            transport,
            bridge_id_hash,
            saturated_len(topics.len()),
            saturated_len(services.len()),
            saturated_len(actions.len()),
            saturated_len(parameters.len()),
            total_buffer_bytes,
            max_timeout_us,
        )
    }
}

pub const ROS_PREFLIGHT_PAYLOAD_TOO_LARGE: u32 = 1 << 0;
pub const ROS_PREFLIGHT_RESPONSE_TOO_SMALL: u32 = 1 << 1;
pub const ROS_PREFLIGHT_TIMEOUT_EXCEEDED: u32 = 1 << 2;
pub const ROS_PREFLIGHT_QUEUE_DEPTH_ZERO: u32 = 1 << 3;
pub const ROS_PREFLIGHT_TIMEOUT_ZERO: u32 = 1 << 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RosBridgePreflight {
    pub required_buffer_bytes: u32,
    pub error_bits: u32,
}

impl RosBridgePreflight {
    pub const fn passing(&self) -> bool {
        self.error_bits == 0
    }

    pub const fn has_error(&self, error: u32) -> bool {
        (self.error_bits & error) != 0
    }
}

pub fn preflight_ros_topic(topic: RosTopicContract, payload_bytes: u16) -> RosBridgePreflight {
    let mut error_bits = 0;
    if topic.depth == 0 {
        error_bits |= ROS_PREFLIGHT_QUEUE_DEPTH_ZERO;
    }
    if payload_bytes > topic.max_message_bytes {
        error_bits |= ROS_PREFLIGHT_PAYLOAD_TOO_LARGE;
    }

    RosBridgePreflight {
        required_buffer_bytes: topic.buffer_bytes(),
        error_bits,
    }
}

pub fn preflight_ros_service(
    service: RosServiceContract,
    request_bytes: u16,
    response_capacity_bytes: u16,
    budget_us: u32,
) -> RosBridgePreflight {
    let mut error_bits = 0;
    if request_bytes > service.request_bytes_max {
        error_bits |= ROS_PREFLIGHT_PAYLOAD_TOO_LARGE;
    }
    if response_capacity_bytes < service.response_bytes_max {
        error_bits |= ROS_PREFLIGHT_RESPONSE_TOO_SMALL;
    }
    if service.timeout_us == 0 {
        error_bits |= ROS_PREFLIGHT_TIMEOUT_ZERO;
    }
    if service.timeout_us > budget_us {
        error_bits |= ROS_PREFLIGHT_TIMEOUT_EXCEEDED;
    }

    RosBridgePreflight {
        required_buffer_bytes: service.buffer_bytes(),
        error_bits,
    }
}

pub fn preflight_ros_action(
    action: RosActionContract,
    goal_bytes: u16,
    feedback_capacity_bytes: u16,
    result_capacity_bytes: u16,
    budget_us: u32,
) -> RosBridgePreflight {
    let mut error_bits = 0;
    if goal_bytes > action.goal_bytes_max {
        error_bits |= ROS_PREFLIGHT_PAYLOAD_TOO_LARGE;
    }
    if feedback_capacity_bytes < action.feedback_bytes_max
        || result_capacity_bytes < action.result_bytes_max
    {
        error_bits |= ROS_PREFLIGHT_RESPONSE_TOO_SMALL;
    }
    if action.timeout_us == 0 {
        error_bits |= ROS_PREFLIGHT_TIMEOUT_ZERO;
    }
    if action.timeout_us > budget_us {
        error_bits |= ROS_PREFLIGHT_TIMEOUT_EXCEEDED;
    }

    RosBridgePreflight {
        required_buffer_bytes: action.buffer_bytes(),
        error_bits,
    }
}

pub fn preflight_ros_parameter(
    parameter: RosParameterContract,
    value_bytes: u16,
) -> RosBridgePreflight {
    let mut error_bits = 0;
    if value_bytes > parameter.value_bytes_max {
        error_bits |= ROS_PREFLIGHT_PAYLOAD_TOO_LARGE;
    }

    RosBridgePreflight {
        required_buffer_bytes: u32::from(parameter.value_bytes_max),
        error_bits,
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RosBridgeContractReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub transport: u32,
    pub bridge_id_hash: u32,
    pub topic_count: u32,
    pub service_count: u32,
    pub action_count: u32,
    pub parameter_count: u32,
    pub total_buffer_bytes: u32,
    pub max_timeout_us: u32,
    pub checksum: u32,
}

impl RosBridgeContractReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            transport: 0,
            bridge_id_hash: 0,
            topic_count: 0,
            service_count: 0,
            action_count: 0,
            parameter_count: 0,
            total_buffer_bytes: 0,
            max_timeout_us: 0,
            checksum: 0,
        }
    }

    pub fn from_contract(contract: RosBridgeContract) -> Self {
        let mut report = Self {
            transport: contract.transport as u32,
            bridge_id_hash: contract.bridge_id_hash,
            topic_count: u32::from(contract.topic_count),
            service_count: u32::from(contract.service_count),
            action_count: u32::from(contract.action_count),
            parameter_count: u32::from(contract.parameter_count),
            total_buffer_bytes: contract.total_buffer_bytes,
            max_timeout_us: contract.max_timeout_us,
            ..Self::zeroed()
        };
        report.seal();
        report
    }

    pub fn seal(&mut self) {
        self.magic = ROS_BRIDGE_REPORT_MAGIC;
        self.version = ROS_BRIDGE_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == ROS_BRIDGE_REPORT_MAGIC
            && self.version == ROS_BRIDGE_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.transport
            ^ self.bridge_id_hash
            ^ self.topic_count
            ^ self.service_count
            ^ self.action_count
            ^ self.parameter_count
            ^ self.total_buffer_bytes
            ^ self.max_timeout_us
    }
}

/// Bounded ROS-style bridge surface for topics and request/response calls.
pub trait RosBridgeSal {
    type Error;

    fn contract(&self) -> RosBridgeContract;
    fn publish(
        &mut self,
        topic_hash: u32,
        payload: &[u8],
        deadline_us: u64,
    ) -> Result<(), Self::Error>;
    fn request(
        &mut self,
        service_hash: u32,
        request: &[u8],
        response: &mut [u8],
        deadline_us: u64,
    ) -> Result<usize, Self::Error>;
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
        let stale_ready = state.last_success_age_us <= self.effective_stale_after_us(contract);

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

    pub const fn effective_stale_after_us(&self, contract: AiModelContract) -> u32 {
        if self.stale_after_us == 0 {
            contract.stale_after_us
        } else if contract.stale_after_us == 0 || self.stale_after_us < contract.stale_after_us {
            self.stale_after_us
        } else {
            contract.stale_after_us
        }
    }
}

/// Errors from the [`ModelRegistry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiRegistryError {
    Full,
    DuplicateModel(u32),
    UnknownModel(u32),
}

/// Fixed-capacity registry of AI models keyed by `model_id`. Multiplexes inference
/// routing: resolve a request's model to its contract, then apply an [`AiRoutePolicy`]
/// to choose where it runs (on-device / remote / fallback). (M36)
pub struct ModelRegistry<const N: usize> {
    contracts: [Option<AiModelContract>; N],
}

impl<const N: usize> Default for ModelRegistry<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> ModelRegistry<N> {
    pub const fn new() -> Self {
        Self {
            contracts: [None; N],
        }
    }

    pub fn register(&mut self, contract: AiModelContract) -> Result<(), AiRegistryError> {
        if self.resolve(contract.model_id).is_some() {
            return Err(AiRegistryError::DuplicateModel(contract.model_id));
        }
        match self.contracts.iter_mut().find(|c| c.is_none()) {
            Some(slot) => {
                *slot = Some(contract);
                Ok(())
            }
            None => Err(AiRegistryError::Full),
        }
    }

    pub fn resolve(&self, model_id: u32) -> Option<AiModelContract> {
        self.contracts
            .iter()
            .filter_map(|c| *c)
            .find(|c| c.model_id == model_id)
    }

    pub fn len(&self) -> usize {
        self.contracts.iter().filter(|c| c.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Resolve `request`'s model and route it via `policy`; `UnknownModel` if the model
    /// id is not registered.
    pub fn route(
        &self,
        request: &AiInferenceRequest<'_>,
        policy: &AiRoutePolicy,
        state: AiRuntimeState,
        budget_us: u32,
    ) -> Result<AiRouteDecision, AiRegistryError> {
        let contract = self
            .resolve(request.model_id)
            .ok_or(AiRegistryError::UnknownModel(request.model_id))?;
        Ok(policy.decide(contract, state, budget_us))
    }
}

#[cfg(test)]
mod model_registry_tests {
    use super::*;

    #[test]
    fn registry_multiplexes_routes_and_rejects_unknown() {
        let mut reg = ModelRegistry::<4>::new();
        let nn = AiModelContract::new(AiBackendKind::OnDevice, 0x4E4E_4D31, 12, 4, 512, 2_000);
        let llm = AiModelContract::new(AiBackendKind::Hybrid, 0x4C4C_4D31, 256, 256, 0, 50_000);
        reg.register(nn).unwrap();
        reg.register(llm).unwrap();
        assert_eq!(reg.len(), 2);
        assert_eq!(
            reg.register(nn),
            Err(AiRegistryError::DuplicateModel(0x4E4E_4D31))
        );

        let policy = AiRoutePolicy::new(AiRoutePreference::HybridFallback, 0, 2);
        // on-device NN with local ready -> runs on device
        let local = AiRuntimeState::new(true, false, 0, 0);
        let req = AiInferenceRequest::new(0x4E4E_4D31, &[0u8; 12], 2_000);
        assert_eq!(
            reg.route(&req, &policy, local, 4_000).unwrap().target,
            AiRouteTarget::OnDevice
        );
        // hybrid model, local down + endpoint up -> remote
        let remote = AiRuntimeState::new(false, true, 0, 0);
        let req2 = AiInferenceRequest::new(0x4C4C_4D31, &[0u8; 8], 50_000);
        assert_eq!(
            reg.route(&req2, &policy, remote, 100_000).unwrap().target,
            AiRouteTarget::RemoteApi
        );
        // unknown model id is rejected
        let req3 = AiInferenceRequest::new(0xDEAD, &[], 1_000);
        assert_eq!(
            reg.route(&req3, &policy, local, 4_000),
            Err(AiRegistryError::UnknownModel(0xDEAD))
        );
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
    pub stale_after_us: u32,
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
            stale_after_us: 0,
        }
    }

    pub const fn with_stale_after_us(mut self, stale_after_us: u32) -> Self {
        self.stale_after_us = stale_after_us;
        self
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AiModelContractReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub backend: u32,
    pub model_id: u32,
    pub input_bytes_max: u32,
    pub output_bytes_max: u32,
    pub arena_bytes: u32,
    pub timeout_us: u32,
    pub route_preference: u32,
    pub stale_after_us: u32,
    pub endpoint_failure_limit: u32,
    pub checksum: u32,
}

impl AiModelContractReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            backend: 0,
            model_id: 0,
            input_bytes_max: 0,
            output_bytes_max: 0,
            arena_bytes: 0,
            timeout_us: 0,
            route_preference: 0,
            stale_after_us: 0,
            endpoint_failure_limit: 0,
            checksum: 0,
        }
    }

    pub fn from_contract(contract: AiModelContract) -> Self {
        Self::from_contract_and_policy(contract, None)
    }

    pub fn from_contract_and_policy(
        contract: AiModelContract,
        policy: Option<AiRoutePolicy>,
    ) -> Self {
        let mut report = Self {
            backend: contract.backend as u32,
            model_id: contract.model_id,
            input_bytes_max: u32::from(contract.input_bytes_max),
            output_bytes_max: u32::from(contract.output_bytes_max),
            arena_bytes: contract.arena_bytes,
            timeout_us: contract.timeout_us,
            route_preference: policy.map(|policy| policy.preference as u32).unwrap_or(0),
            stale_after_us: policy
                .map(|policy| policy.effective_stale_after_us(contract))
                .unwrap_or(contract.stale_after_us),
            endpoint_failure_limit: policy
                .map(|policy| u32::from(policy.endpoint_failure_limit))
                .unwrap_or(0),
            ..Self::zeroed()
        };
        report.seal();
        report
    }

    pub fn seal(&mut self) {
        self.magic = AI_MODEL_REPORT_MAGIC;
        self.version = AI_MODEL_REPORT_VERSION;
        self.completed = 1;
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == AI_MODEL_REPORT_MAGIC
            && self.version == AI_MODEL_REPORT_VERSION
            && self.checksum == self.compute_checksum()
    }

    fn compute_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.backend
            ^ self.model_id
            ^ self.input_bytes_max
            ^ self.output_bytes_max
            ^ self.arena_bytes
            ^ self.timeout_us
            ^ self.route_preference
            ^ self.stale_after_us
            ^ self.endpoint_failure_limit
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

pub const AI_PREFLIGHT_MODEL_ID_MISMATCH: u32 = 1 << 0;
pub const AI_PREFLIGHT_INPUT_TOO_LARGE: u32 = 1 << 1;
pub const AI_PREFLIGHT_OUTPUT_TOO_SMALL: u32 = 1 << 2;
pub const AI_PREFLIGHT_RAM_EXCEEDED: u32 = 1 << 3;
pub const AI_PREFLIGHT_ROUTE_UNAVAILABLE: u32 = 1 << 4;
pub const AI_PREFLIGHT_DEGRADED_FALLBACK: u32 = 1 << 5;
pub const AI_PREFLIGHT_STALE_SNAPSHOT: u32 = 1 << 6;
pub const AI_PREFLIGHT_STALE_TOO_OLD: u32 = 1 << 7;
pub const AI_PREFLIGHT_ENDPOINT_CIRCUIT_OPEN: u32 = 1 << 8;
pub const AI_PREFLIGHT_LOCAL_ARENA_MISSING: u32 = 1 << 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiInvocationLimits {
    pub output_capacity_bytes: u32,
    pub scratch_bytes: u32,
    pub available_ram_bytes: u32,
    pub budget_us: u32,
    pub max_stale_us: u32,
    pub allow_stale_snapshot: bool,
    pub allow_degraded_fallback: bool,
    pub allow_unavailable: bool,
    pub allow_endpoint_circuit_open: bool,
}

impl AiInvocationLimits {
    pub const fn new(
        output_capacity_bytes: u32,
        scratch_bytes: u32,
        available_ram_bytes: u32,
        budget_us: u32,
    ) -> Self {
        Self {
            output_capacity_bytes,
            scratch_bytes,
            available_ram_bytes,
            budget_us,
            max_stale_us: 0,
            allow_stale_snapshot: false,
            allow_degraded_fallback: false,
            allow_unavailable: false,
            allow_endpoint_circuit_open: false,
        }
    }

    pub const fn with_max_stale_us(mut self, max_stale_us: u32) -> Self {
        self.max_stale_us = max_stale_us;
        self
    }

    pub const fn allow_stale_snapshot(mut self) -> Self {
        self.allow_stale_snapshot = true;
        self
    }

    pub const fn allow_degraded_fallback(mut self) -> Self {
        self.allow_degraded_fallback = true;
        self
    }

    pub const fn allow_unavailable(mut self) -> Self {
        self.allow_unavailable = true;
        self
    }

    pub const fn allow_endpoint_circuit_open(mut self) -> Self {
        self.allow_endpoint_circuit_open = true;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiInvocationPreflight {
    pub route: AiRouteDecision,
    pub required_ram_bytes: u32,
    pub available_ram_bytes: u32,
    pub error_bits: u32,
}

impl AiInvocationPreflight {
    pub const fn passing(&self) -> bool {
        self.error_bits == 0
    }

    pub const fn has_error(&self, error: u32) -> bool {
        (self.error_bits & error) != 0
    }
}

pub fn preflight_ai_invocation(
    contract: AiModelContract,
    policy: AiRoutePolicy,
    state: AiRuntimeState,
    request: AiInferenceRequest<'_>,
    limits: AiInvocationLimits,
) -> AiInvocationPreflight {
    let route = policy.decide(contract, state, limits.budget_us);
    let input_bytes = saturated_usize_to_u32(request.input.len());
    let local_arena_bytes = match contract.backend {
        AiBackendKind::OnDevice | AiBackendKind::Hybrid => contract.arena_bytes,
        AiBackendKind::RemoteApi | AiBackendKind::EdgeSidecar => 0,
    };
    let required_ram_bytes = input_bytes
        .saturating_add(limits.output_capacity_bytes)
        .saturating_add(limits.scratch_bytes)
        .saturating_add(local_arena_bytes);
    let mut error_bits = 0;

    if request.model_id != contract.model_id {
        error_bits |= AI_PREFLIGHT_MODEL_ID_MISMATCH;
    }
    if input_bytes > u32::from(contract.input_bytes_max) {
        error_bits |= AI_PREFLIGHT_INPUT_TOO_LARGE;
    }
    if limits.output_capacity_bytes < u32::from(contract.output_bytes_max) {
        error_bits |= AI_PREFLIGHT_OUTPUT_TOO_SMALL;
    }
    if required_ram_bytes > limits.available_ram_bytes {
        error_bits |= AI_PREFLIGHT_RAM_EXCEEDED;
    }
    if matches!(
        contract.backend,
        AiBackendKind::OnDevice | AiBackendKind::Hybrid
    ) && contract.arena_bytes == 0
    {
        error_bits |= AI_PREFLIGHT_LOCAL_ARENA_MISSING;
    }
    if route.target == AiRouteTarget::Unavailable && !limits.allow_unavailable {
        error_bits |= AI_PREFLIGHT_ROUTE_UNAVAILABLE;
    }
    if route.target == AiRouteTarget::DegradedFallback && !limits.allow_degraded_fallback {
        error_bits |= AI_PREFLIGHT_DEGRADED_FALLBACK;
    }
    if route.uses_stale_snapshot && !limits.allow_stale_snapshot {
        error_bits |= AI_PREFLIGHT_STALE_SNAPSHOT;
    }
    if route.uses_stale_snapshot
        && limits.max_stale_us > 0
        && state.last_success_age_us > limits.max_stale_us
    {
        error_bits |= AI_PREFLIGHT_STALE_TOO_OLD;
    }
    if route.endpoint_circuit_open && !limits.allow_endpoint_circuit_open {
        error_bits |= AI_PREFLIGHT_ENDPOINT_CIRCUIT_OPEN;
    }

    AiInvocationPreflight {
        route,
        required_ram_bytes,
        available_ram_bytes: limits.available_ram_bytes,
        error_bits,
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
        KernelError::ForeignModuleInitFail | KernelError::ForeignModulePollFail => RebootModule,
    }
}

fn saturated_len(len: usize) -> u8 {
    if len > u8::MAX as usize {
        u8::MAX
    } else {
        len as u8
    }
}

fn saturated_usize_to_u32(len: usize) -> u32 {
    if len > u32::MAX as usize {
        u32::MAX
    } else {
        len as u32
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

    struct LoopbackRos;

    impl RosBridgeSal for LoopbackRos {
        type Error = ();

        fn contract(&self) -> RosBridgeContract {
            RosBridgeContract::from_parts(
                RosBridgeTransport::Serial,
                0xA11CE,
                &[RosTopicContract::new(0x10, 0x20, 4, 64)],
                &[RosServiceContract::new(0x30, 16, 16, 50_000)],
                &[RosActionContract::new(0x40, 16, 8, 16, 100_000)],
                &[RosParameterContract::new(0x50, 12)],
            )
        }

        fn publish(
            &mut self,
            _topic_hash: u32,
            _payload: &[u8],
            _deadline_us: u64,
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        fn request(
            &mut self,
            _service_hash: u32,
            request: &[u8],
            response: &mut [u8],
            _deadline_us: u64,
        ) -> Result<usize, Self::Error> {
            let len = request.len().min(response.len());
            response[..len].copy_from_slice(&request[..len]);
            Ok(len)
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
    fn ai_invocation_preflight_accepts_bounded_local_call() {
        let contract = AiModelContract::new(AiBackendKind::OnDevice, 42, 8, 8, 4096, 20_000);
        let policy = AiRoutePolicy::new(AiRoutePreference::LocalOnly, 50_000, 2);
        let state = AiRuntimeState::new(true, false, 1_000, 0);
        let input = [1, 2, 3, 4];
        let limits = AiInvocationLimits::new(8, 128, 8 * 1024, 25_000);

        let report = preflight_ai_invocation(
            contract,
            policy,
            state,
            AiInferenceRequest::new(42, &input, 10_000),
            limits,
        );

        assert!(report.passing());
        assert_eq!(report.route.target, AiRouteTarget::OnDevice);
        assert_eq!(report.required_ram_bytes, 4 + 8 + 128 + 4096);
    }

    #[test]
    fn ai_invocation_preflight_rejects_buffers_ram_and_model_mismatch() {
        let contract = AiModelContract::new(AiBackendKind::Hybrid, 42, 4, 8, 4096, 20_000);
        let policy = AiRoutePolicy::new(AiRoutePreference::PreferLocal, 50_000, 2);
        let state = AiRuntimeState::new(true, false, 1_000, 0);
        let input = [0u8; 12];
        let limits = AiInvocationLimits::new(4, 128, 512, 25_000);

        let report = preflight_ai_invocation(
            contract,
            policy,
            state,
            AiInferenceRequest::new(7, &input, 10_000),
            limits,
        );

        assert!(!report.passing());
        assert!(report.has_error(AI_PREFLIGHT_MODEL_ID_MISMATCH));
        assert!(report.has_error(AI_PREFLIGHT_INPUT_TOO_LARGE));
        assert!(report.has_error(AI_PREFLIGHT_OUTPUT_TOO_SMALL));
        assert!(report.has_error(AI_PREFLIGHT_RAM_EXCEEDED));
        assert_eq!(report.required_ram_bytes, 12 + 4 + 128 + 4096);
    }

    #[test]
    fn ai_invocation_preflight_requires_local_arena_for_local_backends() {
        let contract = AiModelContract::new(AiBackendKind::OnDevice, 42, 8, 8, 0, 20_000);
        let policy = AiRoutePolicy::new(AiRoutePreference::LocalOnly, 50_000, 2);
        let state = AiRuntimeState::new(true, false, 1_000, 0);
        let input = [1, 2, 3, 4];
        let limits = AiInvocationLimits::new(8, 128, 8 * 1024, 25_000);

        let report = preflight_ai_invocation(
            contract,
            policy,
            state,
            AiInferenceRequest::new(42, &input, 10_000),
            limits,
        );

        assert!(!report.passing());
        assert!(report.has_error(AI_PREFLIGHT_LOCAL_ARENA_MISSING));
        assert_eq!(report.required_ram_bytes, 4 + 8 + 128);
    }

    #[test]
    fn ai_invocation_preflight_flags_stale_and_endpoint_policy() {
        let contract = AiModelContract::new(AiBackendKind::RemoteApi, 7, 32, 32, 0, 20_000)
            .with_stale_after_us(100_000);
        let policy = AiRoutePolicy::new(AiRoutePreference::PreferRemote, 80_000, 2);
        let state = AiRuntimeState::new(false, true, 70_000, 2);
        let input = [1u8; 16];
        let limits = AiInvocationLimits::new(32, 64, 1024, 30_000).with_max_stale_us(50_000);

        let report = preflight_ai_invocation(
            contract,
            policy,
            state,
            AiInferenceRequest::new(7, &input, 10_000),
            limits,
        );

        assert!(!report.passing());
        assert_eq!(report.route.target, AiRouteTarget::StaleSnapshot);
        assert!(report.route.endpoint_circuit_open);
        assert!(report.has_error(AI_PREFLIGHT_STALE_SNAPSHOT));
        assert!(report.has_error(AI_PREFLIGHT_STALE_TOO_OLD));
        assert!(report.has_error(AI_PREFLIGHT_ENDPOINT_CIRCUIT_OPEN));

        let allowed = preflight_ai_invocation(
            contract,
            policy,
            state,
            AiInferenceRequest::new(7, &input, 10_000),
            limits
                .allow_stale_snapshot()
                .allow_endpoint_circuit_open()
                .with_max_stale_us(80_000),
        );
        assert!(allowed.passing());
    }

    #[test]
    fn ros_bridge_contract_summarizes_bounded_entities() {
        let bridge = LoopbackRos;
        let contract = bridge.contract();

        assert_eq!(contract.transport, RosBridgeTransport::Serial);
        assert_eq!(contract.topic_count, 1);
        assert_eq!(contract.service_count, 1);
        assert_eq!(contract.action_count, 1);
        assert_eq!(contract.parameter_count, 1);
        assert_eq!(
            contract.total_buffer_bytes,
            4 * 64 + 16 + 16 + 16 + 8 + 16 + 12
        );
        assert_eq!(contract.max_timeout_us, 100_000);
    }

    #[test]
    fn ros_bridge_report_seals_bounded_bridge_contract() {
        let bridge = LoopbackRos;
        let report = RosBridgeContractReport::from_contract(bridge.contract());

        assert!(report.verify_checksum());
        assert_eq!(report.magic, ROS_BRIDGE_REPORT_MAGIC);
        assert_eq!(report.version, ROS_BRIDGE_REPORT_VERSION);
        assert_eq!(report.transport, RosBridgeTransport::Serial as u32);
        assert_eq!(report.topic_count, 1);
        assert_eq!(report.service_count, 1);
        assert_eq!(report.action_count, 1);
        assert_eq!(report.parameter_count, 1);
        assert_eq!(report.total_buffer_bytes, 340);
        assert_eq!(report.max_timeout_us, 100_000);
    }

    #[test]
    fn ros_bridge_sal_uses_caller_owned_buffers() {
        let mut bridge = LoopbackRos;
        let request = [1u8, 2, 3, 4];
        let mut response = [0u8; 8];

        bridge.publish(0x10, &request, 10_000).unwrap();
        let len = bridge
            .request(0x30, &request, &mut response, 10_000)
            .unwrap();

        assert_eq!(len, 4);
        assert_eq!(&response[..4], &request);
    }

    #[test]
    fn ros_topic_preflight_checks_payload_and_depth() {
        let topic = RosTopicContract::new(0x10, 0x20, 4, 64);
        let pass = preflight_ros_topic(topic, 32);

        assert!(pass.passing());
        assert_eq!(pass.required_buffer_bytes, 256);

        let fail = preflight_ros_topic(RosTopicContract::new(0x10, 0x20, 0, 64), 128);
        assert!(!fail.passing());
        assert!(fail.has_error(ROS_PREFLIGHT_PAYLOAD_TOO_LARGE));
        assert!(fail.has_error(ROS_PREFLIGHT_QUEUE_DEPTH_ZERO));
    }

    #[test]
    fn ros_service_preflight_checks_buffers_and_timeout() {
        let service = RosServiceContract::new(0x30, 16, 32, 50_000);
        let pass = preflight_ros_service(service, 16, 32, 60_000);

        assert!(pass.passing());
        assert_eq!(pass.required_buffer_bytes, 48);

        let fail = preflight_ros_service(service, 24, 8, 20_000);
        assert!(fail.has_error(ROS_PREFLIGHT_PAYLOAD_TOO_LARGE));
        assert!(fail.has_error(ROS_PREFLIGHT_RESPONSE_TOO_SMALL));
        assert!(fail.has_error(ROS_PREFLIGHT_TIMEOUT_EXCEEDED));

        let zero_timeout =
            preflight_ros_service(RosServiceContract::new(0x30, 16, 32, 0), 16, 32, 20_000);
        assert!(zero_timeout.has_error(ROS_PREFLIGHT_TIMEOUT_ZERO));
    }

    #[test]
    fn ros_action_and_parameter_preflight_are_bounded() {
        let action = RosActionContract::new(0x40, 16, 8, 24, 100_000);
        let action_fail = preflight_ros_action(action, 32, 4, 8, 50_000);

        assert_eq!(action_fail.required_buffer_bytes, 48);
        assert!(action_fail.has_error(ROS_PREFLIGHT_PAYLOAD_TOO_LARGE));
        assert!(action_fail.has_error(ROS_PREFLIGHT_RESPONSE_TOO_SMALL));
        assert!(action_fail.has_error(ROS_PREFLIGHT_TIMEOUT_EXCEEDED));

        let zero_timeout = preflight_ros_action(
            RosActionContract::new(0x40, 16, 8, 24, 0),
            16,
            8,
            24,
            20_000,
        );
        assert!(zero_timeout.has_error(ROS_PREFLIGHT_TIMEOUT_ZERO));

        let parameter = RosParameterContract::new(0x50, 12);
        assert!(preflight_ros_parameter(parameter, 8).passing());
        assert!(preflight_ros_parameter(parameter, 16).has_error(ROS_PREFLIGHT_PAYLOAD_TOO_LARGE));
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
    fn ai_model_report_seals_model_and_route_policy_contract() {
        let policy = AiRoutePolicy::new(AiRoutePreference::HybridFallback, 30_000, 2);
        let contract = AiModelContract::new(AiBackendKind::Hybrid, 7, 16, 24, 4096, 5_000)
            .with_stale_after_us(100_000);
        let report = AiModelContractReport::from_contract_and_policy(contract, Some(policy));

        assert!(report.verify_checksum());
        assert_eq!(report.magic, AI_MODEL_REPORT_MAGIC);
        assert_eq!(report.version, AI_MODEL_REPORT_VERSION);
        assert_eq!(report.backend, AiBackendKind::Hybrid as u32);
        assert_eq!(report.model_id, 7);
        assert_eq!(report.input_bytes_max, 16);
        assert_eq!(report.output_bytes_max, 24);
        assert_eq!(report.arena_bytes, 4096);
        assert_eq!(report.timeout_us, 5_000);
        assert_eq!(
            report.route_preference,
            AiRoutePreference::HybridFallback as u32
        );
        assert_eq!(report.stale_after_us, 30_000);
        assert_eq!(report.endpoint_failure_limit, 2);
    }

    #[test]
    fn ai_model_report_uses_model_stale_window_without_policy() {
        let contract = AiModelContract::new(AiBackendKind::Hybrid, 7, 16, 24, 4096, 5_000)
            .with_stale_after_us(100_000);
        let report = AiModelContractReport::from_contract(contract);

        assert!(report.verify_checksum());
        assert_eq!(report.route_preference, 0);
        assert_eq!(report.stale_after_us, 100_000);
        assert_eq!(report.endpoint_failure_limit, 0);
    }

    #[test]
    fn ai_route_policy_trips_endpoint_to_fresh_snapshot() {
        let policy = AiRoutePolicy::new(AiRoutePreference::PreferRemote, 50_000, 2);
        let contract = AiModelContract::new(AiBackendKind::RemoteApi, 7, 32, 32, 0, 20_000)
            .with_stale_after_us(100_000);
        let state = AiRuntimeState::new(false, true, 10_000, 2);

        let decision = policy.decide(contract, state, 30_000);

        assert_eq!(decision.target, AiRouteTarget::StaleSnapshot);
        assert!(decision.endpoint_circuit_open);
        assert!(decision.uses_stale_snapshot);
    }

    #[test]
    fn ai_route_policy_inherits_model_stale_window_when_policy_is_unset() {
        let policy = AiRoutePolicy::new(AiRoutePreference::PreferRemote, 0, 2);
        let contract = AiModelContract::new(AiBackendKind::RemoteApi, 7, 32, 32, 0, 20_000)
            .with_stale_after_us(80_000);
        let state = AiRuntimeState::new(false, true, 70_000, 2);

        let decision = policy.decide(contract, state, 30_000);

        assert_eq!(policy.effective_stale_after_us(contract), 80_000);
        assert_eq!(decision.target, AiRouteTarget::StaleSnapshot);
        assert!(decision.uses_stale_snapshot);
    }

    #[test]
    fn ai_route_policy_uses_stricter_stale_window_than_model_contract() {
        let policy = AiRoutePolicy::new(AiRoutePreference::PreferRemote, 10_000, 2);
        let contract = AiModelContract::new(AiBackendKind::RemoteApi, 7, 32, 32, 0, 20_000)
            .with_stale_after_us(80_000);
        let state = AiRuntimeState::new(false, true, 20_000, 2);

        let decision = policy.decide(contract, state, 30_000);

        assert_eq!(policy.effective_stale_after_us(contract), 10_000);
        assert_eq!(decision.target, AiRouteTarget::DegradedFallback);
        assert!(!decision.uses_stale_snapshot);
    }

    #[test]
    fn ai_route_policy_uses_degraded_fallback_when_budget_is_too_small() {
        let policy = AiRoutePolicy::new(AiRoutePreference::HybridFallback, 1_000, 3);
        let contract = AiModelContract::new(AiBackendKind::EdgeSidecar, 7, 32, 32, 0, 20_000)
            .with_stale_after_us(100_000);
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
