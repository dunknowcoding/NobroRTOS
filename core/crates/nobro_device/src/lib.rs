//! Extensible device-module framework for NobroRTOS.
//!
//! A new servo, motor, or sensor **brand** is DATA - a `ServoProfile` / `MotorProfile` /
//! `SensorDescriptor` const - not code. Config-driven generic drivers turn that data into
//! working actuation/identification, so common hardware works out of the box and third
//! parties extend the supported-hardware list without touching the core. This is the
//! data-first extensibility of a devicetree with the safety of Rust traits and the
//! approachability of an Arduino library.
#![cfg_attr(not(test), no_std)]

// ---------------------------------------------------------------- provider resources

/// One independently measurable resource dimension owned by a mounted provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ResourceDimension {
    FlashBytes = 0,
    StaticRamBytes = 1,
    RetainedHeapBytes = 2,
    StackBytes = 3,
    VendorReservedRamBytes = 4,
    WorkerThreads = 5,
    InterruptSlots = 6,
    DmaChannels = 7,
    ControllerFirmwareBytes = 8,
    PeripheralChannels = 9,
}

impl ResourceDimension {
    const fn mask(self) -> u16 {
        1_u16 << self as u8
    }
}

/// Fixed admission price for one mounted board-feature provider.
///
/// Values and knowledge are deliberately separate: zero can be a measured or
/// declared result, while [`ProviderResourcePrice::unknown`] keeps every
/// dimension unknown. This prevents a default value from silently becoming a
/// claim that a vendor stack consumes no heap, stack, interrupt, DMA, or
/// peripheral channels. Workload-dependent CPU, transient heap, stack
/// high-water, and latency costs live in [`ProviderRuntimePrice`].
///
/// `stack_bytes` counts fixed stacks of provider-created workers; caller-task
/// use belongs to runtime `stack_high_water_bytes`. `vendor_reserved_ram_bytes`
/// counts RAM removed from the ordinary allocator by opaque firmware or
/// hardware. Driver pools and DMA buffers allocated from the ordinary heap
/// belong to `retained_heap_bytes` instead. `controller_firmware_bytes` counts
/// an image loaded into a separate controller, not code already included in
/// `flash_bytes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderResourcePrice {
    flash_bytes: u32,
    static_ram_bytes: u32,
    retained_heap_bytes: u32,
    stack_bytes: u32,
    vendor_reserved_ram_bytes: u32,
    worker_threads: u8,
    interrupt_slots: u8,
    dma_channels: u8,
    controller_firmware_bytes: u32,
    peripheral_channels: u8,
    known_dimensions: u16,
}

impl ProviderResourcePrice {
    pub const ALL_KNOWN: u16 = (1_u16 << (ResourceDimension::PeripheralChannels as u8 + 1)) - 1;

    /// Construct an unpriced provider. Every numeric field is a placeholder,
    /// not a zero-cost claim, until its typed builder marks the field known.
    pub const fn unknown() -> Self {
        Self {
            flash_bytes: 0,
            static_ram_bytes: 0,
            retained_heap_bytes: 0,
            stack_bytes: 0,
            vendor_reserved_ram_bytes: 0,
            worker_threads: 0,
            interrupt_slots: 0,
            dma_channels: 0,
            controller_firmware_bytes: 0,
            peripheral_channels: 0,
            known_dimensions: 0,
        }
    }

    /// Explicitly declare every resource dimension known and zero.
    ///
    /// Use this only when evidence establishes zero; [`Default`] intentionally
    /// returns [`Self::unknown`].
    pub const fn known_zero() -> Self {
        Self {
            known_dimensions: Self::ALL_KNOWN,
            ..Self::unknown()
        }
    }

    pub const fn is_known(self, dimension: ResourceDimension) -> bool {
        self.known_dimensions & dimension.mask() != 0
    }

    pub const fn is_complete(self) -> bool {
        self.known_dimensions == Self::ALL_KNOWN
    }

    pub const fn known_dimensions(self) -> u16 {
        self.known_dimensions
    }

    pub const fn flash_bytes(self) -> u32 {
        self.flash_bytes
    }

    pub const fn static_ram_bytes(self) -> u32 {
        self.static_ram_bytes
    }

    pub const fn retained_heap_bytes(self) -> u32 {
        self.retained_heap_bytes
    }

    pub const fn stack_bytes(self) -> u32 {
        self.stack_bytes
    }

    pub const fn vendor_reserved_ram_bytes(self) -> u32 {
        self.vendor_reserved_ram_bytes
    }

    pub const fn worker_threads(self) -> u8 {
        self.worker_threads
    }

    pub const fn interrupt_slots(self) -> u8 {
        self.interrupt_slots
    }

    pub const fn dma_channels(self) -> u8 {
        self.dma_channels
    }

    pub const fn controller_firmware_bytes(self) -> u32 {
        self.controller_firmware_bytes
    }

    pub const fn peripheral_channels(self) -> u8 {
        self.peripheral_channels
    }

    pub const fn with_flash_bytes(mut self, value: u32) -> Self {
        self.flash_bytes = value;
        self.known_dimensions |= ResourceDimension::FlashBytes.mask();
        self
    }

    pub const fn with_static_ram_bytes(mut self, value: u32) -> Self {
        self.static_ram_bytes = value;
        self.known_dimensions |= ResourceDimension::StaticRamBytes.mask();
        self
    }

    pub const fn with_retained_heap_bytes(mut self, value: u32) -> Self {
        self.retained_heap_bytes = value;
        self.known_dimensions |= ResourceDimension::RetainedHeapBytes.mask();
        self
    }

    pub const fn with_stack_bytes(mut self, value: u32) -> Self {
        self.stack_bytes = value;
        self.known_dimensions |= ResourceDimension::StackBytes.mask();
        self
    }

    pub const fn with_vendor_reserved_ram_bytes(mut self, value: u32) -> Self {
        self.vendor_reserved_ram_bytes = value;
        self.known_dimensions |= ResourceDimension::VendorReservedRamBytes.mask();
        self
    }

    pub const fn with_worker_threads(mut self, value: u8) -> Self {
        self.worker_threads = value;
        self.known_dimensions |= ResourceDimension::WorkerThreads.mask();
        self
    }

    pub const fn with_interrupt_slots(mut self, value: u8) -> Self {
        self.interrupt_slots = value;
        self.known_dimensions |= ResourceDimension::InterruptSlots.mask();
        self
    }

    pub const fn with_dma_channels(mut self, value: u8) -> Self {
        self.dma_channels = value;
        self.known_dimensions |= ResourceDimension::DmaChannels.mask();
        self
    }

    pub const fn with_controller_firmware_bytes(mut self, value: u32) -> Self {
        self.controller_firmware_bytes = value;
        self.known_dimensions |= ResourceDimension::ControllerFirmwareBytes.mask();
        self
    }

    pub const fn with_peripheral_channels(mut self, value: u8) -> Self {
        self.peripheral_channels = value;
        self.known_dimensions |= ResourceDimension::PeripheralChannels.mask();
        self
    }
}

impl Default for ProviderResourcePrice {
    fn default() -> Self {
        Self::unknown()
    }
}

/// Traffic pacing used by an exact provider workload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WorkloadPacing {
    /// Offered and observed operation counts must match over one second.
    Fixed,
    /// Offered demand and useful completed work are counted over an exact interval.
    Adaptive,
}

/// Exact provider configuration and admitted traffic observation.
///
/// The fingerprint is deterministic and allocation-free, but is an identity
/// check rather than a cryptographic digest. Providers own the order and
/// meaning of `configuration_words`; the identity combines the resulting
/// fingerprint with the admitted traffic observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderWorkload {
    configuration_fingerprint: u64,
    pacing: WorkloadPacing,
    observation_interval_us: u64,
    offered_operations: u32,
    observed_operations: u32,
}

impl ProviderWorkload {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    pub const fn new(
        namespace: &str,
        configuration_words: &[u32],
        operations_per_second: u32,
    ) -> Self {
        let mut hash = Self::FNV_OFFSET;
        let namespace = namespace.as_bytes();
        let mut index = 0;
        while index < namespace.len() {
            hash ^= namespace[index] as u64;
            hash = hash.wrapping_mul(Self::FNV_PRIME);
            index += 1;
        }
        index = 0;
        while index < configuration_words.len() {
            let bytes = configuration_words[index].to_le_bytes();
            let mut byte = 0;
            while byte < bytes.len() {
                hash ^= bytes[byte] as u64;
                hash = hash.wrapping_mul(Self::FNV_PRIME);
                byte += 1;
            }
            index += 1;
        }
        Self {
            configuration_fingerprint: hash,
            pacing: WorkloadPacing::Fixed,
            observation_interval_us: 1_000_000,
            offered_operations: operations_per_second,
            observed_operations: operations_per_second,
        }
    }

    /// Describe variable-rate work without pretending every offer completed on schedule.
    ///
    /// Timing, retry, expiry, and batching parameters belong in `configuration_words`, so
    /// changing any policy also changes the workload fingerprint.
    pub const fn adaptive(
        namespace: &str,
        configuration_words: &[u32],
        observation_interval_us: u64,
        offered_operations: u32,
        observed_operations: u32,
    ) -> Self {
        let fixed = Self::new(namespace, configuration_words, offered_operations);
        Self {
            pacing: WorkloadPacing::Adaptive,
            observation_interval_us,
            observed_operations,
            ..fixed
        }
    }

    pub const fn is_valid(self) -> bool {
        self.observation_interval_us != 0
            && self.offered_operations != 0
            && self.observed_operations <= self.offered_operations
            && match self.pacing {
                WorkloadPacing::Fixed => {
                    self.observation_interval_us == 1_000_000
                        && self.observed_operations == self.offered_operations
                }
                WorkloadPacing::Adaptive => true,
            }
    }

    pub const fn configuration_fingerprint(self) -> u64 {
        self.configuration_fingerprint
    }

    /// Whole offered operations per second, rounded down.
    ///
    /// Use the interval/count accessors when adaptive sub-Hz precision matters.
    pub const fn operations_per_second(self) -> u32 {
        if self.observation_interval_us == 0 {
            return 0;
        }
        let rate =
            (self.offered_operations as u128 * 1_000_000) / self.observation_interval_us as u128;
        if rate > u32::MAX as u128 {
            u32::MAX
        } else {
            rate as u32
        }
    }

    pub const fn pacing(self) -> WorkloadPacing {
        self.pacing
    }

    pub const fn observation_interval_us(self) -> u64 {
        self.observation_interval_us
    }

    pub const fn offered_operations(self) -> u32 {
        self.offered_operations
    }

    pub const fn observed_operations(self) -> u32 {
        self.observed_operations
    }
}

/// Workload-dependent resource price for one exact provider configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderRuntimePrice {
    workload: ProviderWorkload,
    transient_heap_peak_bytes: u32,
    stack_high_water_bytes: u32,
    cpu_cycles_per_second: u64,
    latency_p99_cycles: u64,
    latency_max_cycles: u64,
    known_dimensions: u8,
}

impl ProviderRuntimePrice {
    const TRANSIENT_HEAP: u8 = 1 << 0;
    const STACK_HIGH_WATER: u8 = 1 << 1;
    const CPU_CYCLES: u8 = 1 << 2;
    const LATENCY_P99: u8 = 1 << 3;
    const LATENCY_MAX: u8 = 1 << 4;
    pub const ALL_KNOWN: u8 = (1 << 5) - 1;

    pub const fn unknown(workload: ProviderWorkload) -> Self {
        Self {
            workload,
            transient_heap_peak_bytes: 0,
            stack_high_water_bytes: 0,
            cpu_cycles_per_second: 0,
            latency_p99_cycles: 0,
            latency_max_cycles: 0,
            known_dimensions: 0,
        }
    }

    /// Explicitly declare every runtime dimension known and zero.
    ///
    /// This is useful for allocation-free host fakes. Physical providers
    /// should use the typed builders with evidence for their exact workload.
    pub const fn known_zero(workload: ProviderWorkload) -> Self {
        Self {
            known_dimensions: Self::ALL_KNOWN,
            ..Self::unknown(workload)
        }
    }

    pub const fn workload(self) -> ProviderWorkload {
        self.workload
    }

    pub const fn transient_heap_peak_bytes(self) -> u32 {
        self.transient_heap_peak_bytes
    }

    pub const fn stack_high_water_bytes(self) -> u32 {
        self.stack_high_water_bytes
    }

    pub const fn cpu_cycles_per_second(self) -> u64 {
        self.cpu_cycles_per_second
    }

    pub const fn cpu_cycles_per_operation(self) -> Option<u64> {
        let observed = self.workload.observed_operations as u128;
        if observed == 0 || self.known_dimensions & Self::CPU_CYCLES == 0 {
            None
        } else {
            let numerator =
                self.cpu_cycles_per_second as u128 * self.workload.observation_interval_us as u128;
            let denominator = observed * 1_000_000;
            let cycles = numerator.div_ceil(denominator);
            Some(if cycles > u64::MAX as u128 {
                u64::MAX
            } else {
                cycles as u64
            })
        }
    }

    pub const fn latency_p99_cycles(self) -> u64 {
        self.latency_p99_cycles
    }

    pub const fn latency_max_cycles(self) -> u64 {
        self.latency_max_cycles
    }

    pub const fn is_complete(self) -> bool {
        self.workload.is_valid()
            && self.known_dimensions == Self::ALL_KNOWN
            && self.latency_p99_cycles <= self.latency_max_cycles
    }

    pub const fn matches(self, workload: ProviderWorkload) -> bool {
        self.is_complete()
            && self.workload.configuration_fingerprint == workload.configuration_fingerprint
            && self.workload.pacing as u8 == workload.pacing as u8
            && self.workload.observation_interval_us == workload.observation_interval_us
            && self.workload.offered_operations == workload.offered_operations
            && self.workload.observed_operations == workload.observed_operations
    }

    pub const fn with_transient_heap_peak_bytes(mut self, value: u32) -> Self {
        self.transient_heap_peak_bytes = value;
        self.known_dimensions |= Self::TRANSIENT_HEAP;
        self
    }

    pub const fn with_stack_high_water_bytes(mut self, value: u32) -> Self {
        self.stack_high_water_bytes = value;
        self.known_dimensions |= Self::STACK_HIGH_WATER;
        self
    }

    pub const fn with_cpu_cycles_per_second(mut self, value: u64) -> Self {
        self.cpu_cycles_per_second = value;
        self.known_dimensions |= Self::CPU_CYCLES;
        self
    }

    pub const fn with_latency_p99_cycles(mut self, value: u64) -> Self {
        self.latency_p99_cycles = value;
        self.known_dimensions |= Self::LATENCY_P99;
        self
    }

    pub const fn with_latency_max_cycles(mut self, value: u64) -> Self {
        self.latency_max_cycles = value;
        self.known_dimensions |= Self::LATENCY_MAX;
        self
    }
}

/// Fixed ownership plus runtime evidence for one exact workload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderAdmissionPrice {
    fixed: ProviderResourcePrice,
    runtime: ProviderRuntimePrice,
}

impl ProviderAdmissionPrice {
    pub const fn new(fixed: ProviderResourcePrice, runtime: ProviderRuntimePrice) -> Self {
        Self { fixed, runtime }
    }

    pub const fn known_zero(workload: ProviderWorkload) -> Self {
        Self::new(
            ProviderResourcePrice::known_zero(),
            ProviderRuntimePrice::known_zero(workload),
        )
    }

    pub const fn fixed(self) -> ProviderResourcePrice {
        self.fixed
    }

    pub const fn runtime(self) -> ProviderRuntimePrice {
        self.runtime
    }

    pub const fn is_complete_for(self, workload: ProviderWorkload) -> bool {
        self.fixed.is_complete() && self.runtime.matches(workload)
    }
}

impl Default for ProviderAdmissionPrice {
    fn default() -> Self {
        Self::new(
            ProviderResourcePrice::unknown(),
            ProviderRuntimePrice::unknown(ProviderWorkload::new("unpriced-provider", &[], 0)),
        )
    }
}

// ---------------------------------------------------------------- taxonomy

/// Actuator category (extend as new kinds appear).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActuatorKind {
    /// Angular servo (position by pulse width).
    Servo,
    /// Continuous-rotation servo (speed by pulse width; center = stop).
    ContinuousServo,
    /// Brushless ESC (throttle by pulse width; needs arming).
    Esc,
    /// Brushed DC motor via an H-bridge (PWM duty + direction).
    DcMotor,
    /// Stepper (steps; profile carries steps/rev).
    Stepper,
}

/// Sensor category (extend as new categories appear).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorKind {
    Imu,
    Accelerometer,
    Gyroscope,
    Magnetometer,
    Power,
    Pressure,
    Temperature,
    Humidity,
    Distance,
    Light,
    Sound,
}

/// Bus a device sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    I2c,
    Spi,
    Analog,
    Pwm,
}

#[cfg(test)]
mod resource_price_tests {
    use super::*;

    #[test]
    fn unknown_zero_and_measured_zero_are_distinct() {
        let unknown = ProviderResourcePrice::default();
        assert_eq!(unknown.flash_bytes(), 0);
        assert!(!unknown.is_complete());
        assert!(!unknown.is_known(ResourceDimension::FlashBytes));

        let zero = ProviderResourcePrice::known_zero();
        assert!(zero.is_complete());
        assert!(zero.is_known(ResourceDimension::PeripheralChannels));
        assert_eq!(zero.known_dimensions(), ProviderResourcePrice::ALL_KNOWN);
    }

    #[test]
    fn typed_builders_complete_every_dimension_without_sentinels() {
        let price = ProviderResourcePrice::unknown()
            .with_flash_bytes(1)
            .with_static_ram_bytes(2)
            .with_retained_heap_bytes(3)
            .with_stack_bytes(4)
            .with_vendor_reserved_ram_bytes(5)
            .with_worker_threads(1)
            .with_interrupt_slots(1)
            .with_dma_channels(1)
            .with_controller_firmware_bytes(7)
            .with_peripheral_channels(1);
        assert!(price.is_complete());
        assert_eq!(price.retained_heap_bytes(), 3);
        assert_eq!(price.peripheral_channels(), 1);
    }

    #[test]
    fn runtime_price_is_bound_to_one_exact_workload() {
        let workload = ProviderWorkload::new("adc", &[2, 12, 16, 20_000], 625);
        let other_rate = ProviderWorkload::new("adc", &[2, 12, 16, 20_000], 1_250);
        let runtime = ProviderRuntimePrice::unknown(workload)
            .with_transient_heap_peak_bytes(128)
            .with_stack_high_water_bytes(96)
            .with_cpu_cycles_per_second(62_500)
            .with_latency_p99_cycles(120)
            .with_latency_max_cycles(180);
        assert!(runtime.is_complete());
        assert!(runtime.matches(workload));
        assert!(!runtime.matches(other_rate));
        assert_eq!(runtime.cpu_cycles_per_operation(), Some(100));

        let admission = ProviderAdmissionPrice::new(ProviderResourcePrice::known_zero(), runtime);
        assert!(admission.is_complete_for(workload));
        assert!(!admission.is_complete_for(other_rate));
    }

    #[test]
    fn runtime_price_rejects_invalid_rate_and_latency_order() {
        let no_rate = ProviderWorkload::new("pwm", &[1_000, 10], 0);
        assert!(!ProviderRuntimePrice::known_zero(no_rate).is_complete());

        let workload = ProviderWorkload::new("pwm", &[1_000, 10], 100);
        let reversed = ProviderRuntimePrice::unknown(workload)
            .with_transient_heap_peak_bytes(0)
            .with_stack_high_water_bytes(0)
            .with_cpu_cycles_per_second(0)
            .with_latency_p99_cycles(11)
            .with_latency_max_cycles(10);
        assert!(!reversed.is_complete());
    }

    #[test]
    fn adaptive_workload_preserves_exact_interval_and_counts() {
        let workload = ProviderWorkload::adaptive(
            "adaptive-radio",
            &[8, 100_000, 3, 10_000, 500_000],
            15_458_794,
            20,
            15,
        );
        assert!(workload.is_valid());
        assert_eq!(workload.pacing(), WorkloadPacing::Adaptive);
        assert_eq!(workload.observation_interval_us(), 15_458_794);
        assert_eq!(workload.offered_operations(), 20);
        assert_eq!(workload.observed_operations(), 15);
        assert_eq!(workload.operations_per_second(), 1);

        let runtime =
            ProviderRuntimePrice::known_zero(workload).with_cpu_cycles_per_second(240_000_000);
        assert_eq!(runtime.cpu_cycles_per_operation(), Some(247_340_704));
        assert!(!runtime.matches(ProviderWorkload::new(
            "adaptive-radio",
            &[8, 100_000, 3, 10_000, 500_000],
            20,
        )));
        assert!(!ProviderWorkload::adaptive("adaptive-radio", &[1], 1_000_000, 10, 11).is_valid());
        assert!(!ProviderWorkload::adaptive("adaptive-radio", &[1], 0, 10, 1).is_valid());
        let outage = ProviderWorkload::adaptive("adaptive-radio", &[1], 1_000_000, 10, 0);
        assert!(outage.is_valid());
        assert_eq!(
            ProviderRuntimePrice::known_zero(outage).cpu_cycles_per_operation(),
            None
        );
    }
}

// ---------------------------------------------------------------- actuators

/// A servo brand as data. `angle_to_pulse` is the generic driver every angular servo
/// shares - adding a brand is just another `ServoProfile` const.
#[derive(Clone, Copy, Debug)]
pub struct ServoProfile {
    pub name: &'static str,
    pub kind: ActuatorKind,
    pub min_us: u16,
    pub max_us: u16,
    pub center_us: u16,
    pub min_deg: i16,
    pub max_deg: i16,
    pub reversed: bool,
}

impl ServoProfile {
    /// Map an angle (deg) to a servo pulse (us), clamped to the profile's travel.
    pub fn angle_to_pulse(&self, deg: i16) -> u16 {
        let d = deg.clamp(self.min_deg, self.max_deg);
        let span_deg = (self.max_deg - self.min_deg).max(1) as i32;
        let span_us = self.max_us as i32 - self.min_us as i32;
        let frac = if self.reversed {
            (self.max_deg - d) as i32
        } else {
            (d - self.min_deg) as i32
        };
        (self.min_us as i32 + frac * span_us / span_deg) as u16
    }

    /// Continuous servo: map speed [-1000,1000] to a pulse around center.
    pub fn speed_to_pulse(&self, speed_milli: i32) -> u16 {
        let s = speed_milli.clamp(-1000, 1000);
        let s = if self.reversed { -s } else { s };
        let half = (self.max_us as i32 - self.center_us as i32).max(1);
        (self.center_us as i32 + s * half / 1000) as u16
    }
}

/// A motor / ESC brand as data. `throttle_to_pulse` is the shared driver.
#[derive(Clone, Copy, Debug)]
pub struct MotorProfile {
    pub name: &'static str,
    pub kind: ActuatorKind,
    pub min_us: u16, // full reverse (bidir ESC) or idle (unidir)
    pub max_us: u16, // full forward
    pub arm_us: u16, // pulse to hold during the ESC arm delay
    pub bidirectional: bool,
    pub reversed: bool,
}

impl MotorProfile {
    /// Throttle in milli: unidirectional 0..1000, bidirectional -1000..1000.
    pub fn throttle_to_pulse(&self, throttle_milli: i32) -> u16 {
        if self.bidirectional {
            let t = throttle_milli.clamp(-1000, 1000);
            let t = if self.reversed { -t } else { t };
            let mid = (self.min_us as i32 + self.max_us as i32) / 2;
            let half = (self.max_us as i32 - mid).max(1);
            (mid + t * half / 1000) as u16
        } else {
            let t = throttle_milli.clamp(0, 1000);
            let t = if self.reversed { 1000 - t } else { t };
            let span = self.max_us as i32 - self.min_us as i32;
            (self.min_us as i32 + t * span / 1000) as u16
        }
    }
    /// Pulse to emit while arming (ESCs require a low/idle hold before they run).
    pub fn arm_pulse(&self) -> u16 {
        self.arm_us
    }
}

/// Built-in brand catalog. Third parties add a `pub const` of the same shape - that is
/// the entire "add a new servo/motor brand" workflow.
pub mod catalog {
    use super::*;

    pub const SERVO_GENERIC_180: ServoProfile = ServoProfile {
        name: "generic 180 servo",
        kind: ActuatorKind::Servo,
        min_us: 500,
        max_us: 2500,
        center_us: 1500,
        min_deg: 0,
        max_deg: 180,
        reversed: false,
    };
    pub const SERVO_SG90: ServoProfile = ServoProfile {
        name: "SG90-class 9g",
        kind: ActuatorKind::Servo,
        min_us: 500,
        max_us: 2400,
        center_us: 1450,
        min_deg: 0,
        max_deg: 180,
        reversed: false,
    };
    pub const SERVO_MG996R: ServoProfile = ServoProfile {
        name: "MG996R-class metal-gear",
        kind: ActuatorKind::Servo,
        min_us: 1000,
        max_us: 2000,
        center_us: 1500,
        min_deg: 0,
        max_deg: 120,
        reversed: false,
    };
    pub const SERVO_CONTINUOUS: ServoProfile = ServoProfile {
        name: "continuous-rotation servo",
        kind: ActuatorKind::ContinuousServo,
        min_us: 1000,
        max_us: 2000,
        center_us: 1500,
        min_deg: 0,
        max_deg: 0,
        reversed: false,
    };

    pub const ESC_UNIDIR: MotorProfile = MotorProfile {
        name: "generic unidirectional ESC",
        kind: ActuatorKind::Esc,
        min_us: 1000,
        max_us: 2000,
        arm_us: 1000,
        bidirectional: false,
        reversed: false,
    };
    pub const ESC_BIDIR: MotorProfile = MotorProfile {
        name: "bidirectional (3D) ESC",
        kind: ActuatorKind::Esc,
        min_us: 1000,
        max_us: 2000,
        arm_us: 1500,
        bidirectional: true,
        reversed: false,
    };
}

// ---------------------------------------------------------------- sensors

/// A sensor brand as data: category + bus + identity register so a probe can confirm the
/// right chip is present before a driver binds to it.
#[derive(Clone, Copy, Debug)]
pub struct SensorDescriptor {
    pub name: &'static str,
    pub kind: SensorKind,
    pub bus: Bus,
    /// I2C address (or SPI CS id).
    pub address: u8,
    /// Identity register + expected value (0/0 = no identity check).
    pub whoami_reg: u8,
    pub whoami_val: u8,
}

impl SensorDescriptor {
    /// Confirm identity: does `read_reg(whoami_reg)` return `whoami_val`?
    pub fn identify(&self, read_reg: impl Fn(u8) -> u8) -> bool {
        self.whoami_reg == 0 && self.whoami_val == 0 || read_reg(self.whoami_reg) == self.whoami_val
    }
}

/// Built-in sensor catalog (matches the drivers already in the tree). Extend by adding a
/// `pub const`.
pub mod sensor_catalog {
    use super::*;

    pub const MPU9250: SensorDescriptor = SensorDescriptor {
        name: "MPU-9250 9-axis IMU",
        kind: SensorKind::Imu,
        bus: Bus::Spi,
        address: 0,
        whoami_reg: 0x75,
        whoami_val: 0x71,
    };
    pub const INA3221: SensorDescriptor = SensorDescriptor {
        name: "INA3221 3-ch power monitor",
        kind: SensorKind::Power,
        bus: Bus::I2c,
        address: 0x40,
        whoami_reg: 0xFE,
        whoami_val: 0x49, // low byte of the TI manufacturer id
    };
    pub const BMP280: SensorDescriptor = SensorDescriptor {
        name: "BMP280 pressure/temp",
        kind: SensorKind::Pressure,
        bus: Bus::I2c,
        address: 0x76,
        whoami_reg: 0xD0,
        whoami_val: 0x58,
    };
    pub const ICM45686: SensorDescriptor = SensorDescriptor {
        name: "ICM-45686 6-axis IMU",
        kind: SensorKind::Imu,
        bus: Bus::I2c,
        address: 0x68,
        whoami_reg: 0x72,
        whoami_val: 0xE9,
    };
    pub const MPU6050: SensorDescriptor = SensorDescriptor {
        name: "MPU-6050 6-axis IMU",
        kind: SensorKind::Imu,
        bus: Bus::I2c,
        address: 0x68,
        whoami_reg: 0x75,
        whoami_val: 0x68,
    };
    pub const VL53L0X: SensorDescriptor = SensorDescriptor {
        name: "VL53L0X time-of-flight ranger",
        kind: SensorKind::Distance,
        bus: Bus::I2c,
        address: 0x29,
        whoami_reg: 0xC0,
        whoami_val: 0xEE,
    };
}

// ---------------------------------------------------------------- registry

/// A fixed-capacity registry of sensor descriptors: enumerate what a build supports and
/// look one up by category. Third parties push their own descriptors in.
pub struct SensorRegistry<const N: usize> {
    items: [Option<SensorDescriptor>; N],
    len: usize,
}

impl<const N: usize> Default for SensorRegistry<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> SensorRegistry<N> {
    pub const fn new() -> Self {
        Self {
            items: [None; N],
            len: 0,
        }
    }
    pub fn register(&mut self, d: SensorDescriptor) -> bool {
        if self.len >= N {
            return false;
        }
        self.items[self.len] = Some(d);
        self.len += 1;
        true
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// First registered sensor of a category.
    pub fn find_kind(&self, kind: SensorKind) -> Option<SensorDescriptor> {
        self.items
            .iter()
            .flatten()
            .copied()
            .find(|d| d.kind == kind)
    }
    /// Identify which registered sensor is actually on the bus at `address`.
    pub fn identify_at(
        &self,
        address: u8,
        read_reg: impl Fn(u8) -> u8 + Copy,
    ) -> Option<SensorDescriptor> {
        self.items
            .iter()
            .flatten()
            .copied()
            .find(|d| d.address == address && d.identify(read_reg))
    }
}

// ------------------------------------------------------- more device categories

/// A stepper motor brand as data: geometry + limits; `angle_to_steps` is the shared
/// generic conversion every stepper driver uses.
#[derive(Clone, Copy, Debug)]
pub struct StepperProfile {
    pub name: &'static str,
    pub kind: ActuatorKind,
    /// Full steps for one shaft revolution (e.g. 200 for 1.8 deg, 2048 for a 28BYJ-48
    /// behind its gearbox).
    pub steps_per_rev: u32,
    /// Microstepping divisor the driver is strapped to (1 = full steps).
    pub microsteps: u32,
    /// Maximum step rate the motor/driver pair sustains (steps/second).
    pub max_sps: u32,
}

impl StepperProfile {
    /// Signed steps to move by `deg_milli` (millidegrees; 1000 = 1 degree).
    pub fn angle_to_steps(&self, deg_milli: i32) -> i32 {
        let steps = i64::from(self.steps_per_rev) * i64::from(self.microsteps);
        (i64::from(deg_milli) * steps / 360_000) as i32
    }
    /// Step rate for a shaft speed in RPM, clamped to the profile's maximum.
    pub fn rpm_to_sps(&self, rpm: u32) -> u32 {
        let sps = u64::from(rpm) * u64::from(self.steps_per_rev) * u64::from(self.microsteps) / 60;
        (sps as u32).min(self.max_sps)
    }
}

/// Byte order a smart-LED strip expects for each pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorOrder {
    Rgb,
    Grb,
    Rgbw,
    Grbw,
}

/// An addressable-LED strip family as data: wire timing + pixel format. A generic
/// bit-banger/PWM/SPI encoder reads these numbers instead of hardcoding a chip.
#[derive(Clone, Copy, Debug)]
pub struct LedStripProfile {
    pub name: &'static str,
    pub color_order: ColorOrder,
    /// 3 for RGB chips, 4 for RGBW.
    pub bytes_per_pixel: u8,
    /// High time of a 0-bit / 1-bit and the total bit period (ns); all 0 for
    /// clocked (SPI-style) strips that have no timing constraint.
    pub t0h_ns: u16,
    pub t1h_ns: u16,
    pub period_ns: u16,
    /// Low time that latches the frame (microseconds).
    pub reset_us: u16,
}

impl LedStripProfile {
    /// Bytes one frame of `pixels` occupies on the wire.
    pub fn frame_bytes(&self, pixels: usize) -> usize {
        pixels * usize::from(self.bytes_per_pixel)
    }
}

/// A relay/switch module as data: drive polarity + contact settle time.
#[derive(Clone, Copy, Debug)]
pub struct RelayProfile {
    pub name: &'static str,
    /// True if the coil energizes on a HIGH pin (many boards are active-low).
    pub active_high: bool,
    /// Contact settle/debounce time before the switched load is trustworthy (ms).
    pub settle_ms: u16,
}

impl RelayProfile {
    /// Pin level that produces the requested contact state.
    pub fn drive_level(&self, on: bool) -> bool {
        on == self.active_high
    }
}

/// A monochrome OLED/LCD module as data: bus identity + geometry, enough for a generic
/// page-mode framebuffer driver.
#[derive(Clone, Copy, Debug)]
pub struct DisplayProfile {
    pub name: &'static str,
    pub bus: Bus,
    pub address: u8,
    pub width: u16,
    pub height: u16,
    /// Extra column offset some controllers need (SH1106 = 2).
    pub col_offset: u8,
}

impl DisplayProfile {
    /// Bytes a full 1-bpp page-mode framebuffer occupies.
    pub fn framebuffer_bytes(&self) -> usize {
        usize::from(self.width) * usize::from(self.height) / 8
    }
}

/// A GPS/GNSS module as data: link settings a generic NMEA reader needs.
#[derive(Clone, Copy, Debug)]
pub struct GpsProfile {
    pub name: &'static str,
    pub baud: u32,
    pub update_hz: u8,
}

/// XOR checksum of an NMEA sentence body (the bytes between `$` and `*`).
pub fn nmea_checksum(body: &[u8]) -> u8 {
    body.iter().fold(0, |acc, &b| acc ^ b)
}

/// A pulse-echo ranger (HC-SR04 style) as data: acoustic conversion + valid window.
#[derive(Clone, Copy, Debug)]
pub struct RangerProfile {
    pub name: &'static str,
    pub min_mm: u16,
    pub max_mm: u16,
}

impl RangerProfile {
    /// Round-trip echo time -> distance in mm (speed of sound 343 m/s), or None when
    /// outside the sensor's valid window.
    pub fn echo_us_to_mm(&self, echo_us: u32) -> Option<u16> {
        let mm = (echo_us * 343 / 2) / 1000;
        let mm = mm as u16;
        (mm >= self.min_mm && mm <= self.max_mm).then_some(mm)
    }
}

/// Built-in catalog for the categories. Extend by adding a `pub const`.
pub mod device_catalog {
    use super::*;

    pub const STEPPER_NEMA17: StepperProfile = StepperProfile {
        name: "NEMA-17 1.8deg",
        kind: ActuatorKind::Stepper,
        steps_per_rev: 200,
        microsteps: 16,
        max_sps: 40_000,
    };
    pub const STEPPER_28BYJ48: StepperProfile = StepperProfile {
        name: "28BYJ-48 geared",
        kind: ActuatorKind::Stepper,
        steps_per_rev: 2048,
        microsteps: 1,
        max_sps: 1000,
    };

    pub const LED_WS2812B: LedStripProfile = LedStripProfile {
        name: "WS2812B",
        color_order: ColorOrder::Grb,
        bytes_per_pixel: 3,
        t0h_ns: 400,
        t1h_ns: 800,
        period_ns: 1250,
        reset_us: 300,
    };
    pub const LED_SK6812_RGBW: LedStripProfile = LedStripProfile {
        name: "SK6812 RGBW",
        color_order: ColorOrder::Grbw,
        bytes_per_pixel: 4,
        t0h_ns: 300,
        t1h_ns: 600,
        period_ns: 1250,
        reset_us: 80,
    };
    pub const LED_APA102: LedStripProfile = LedStripProfile {
        name: "APA102 (SPI-clocked)",
        color_order: ColorOrder::Rgb,
        bytes_per_pixel: 3,
        t0h_ns: 0,
        t1h_ns: 0,
        period_ns: 0,
        reset_us: 0,
    };

    pub const RELAY_ACTIVE_LOW: RelayProfile = RelayProfile {
        name: "generic active-low relay board",
        active_high: false,
        settle_ms: 10,
    };
    pub const RELAY_ACTIVE_HIGH: RelayProfile = RelayProfile {
        name: "generic active-high relay board",
        active_high: true,
        settle_ms: 10,
    };

    pub const OLED_SSD1306_128X64: DisplayProfile = DisplayProfile {
        name: "SSD1306 128x64",
        bus: Bus::I2c,
        address: 0x3C,
        width: 128,
        height: 64,
        col_offset: 0,
    };
    pub const OLED_SH1106_128X64: DisplayProfile = DisplayProfile {
        name: "SH1106 128x64",
        bus: Bus::I2c,
        address: 0x3C,
        width: 128,
        height: 64,
        col_offset: 2,
    };

    pub const GPS_NEO6M: GpsProfile = GpsProfile {
        name: "u-blox NEO-6M",
        baud: 9600,
        update_hz: 5,
    };
    pub const GPS_NEO8M: GpsProfile = GpsProfile {
        name: "u-blox NEO-M8N",
        baud: 9600,
        update_hz: 10,
    };

    pub const RANGER_HCSR04: RangerProfile = RangerProfile {
        name: "HC-SR04 ultrasonic",
        min_mm: 20,
        max_mm: 4000,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servo_angle_maps_and_clamps_per_brand() {
        let sg = catalog::SERVO_SG90;
        assert_eq!(sg.angle_to_pulse(0), 500);
        assert_eq!(sg.angle_to_pulse(180), 2400);
        assert_eq!(sg.angle_to_pulse(90), 1450); // midpoint
        assert_eq!(sg.angle_to_pulse(999), 2400); // clamp high
                                                  // a different brand yields different pulses from the SAME angle - data-driven
        let mg = catalog::SERVO_MG996R;
        assert_eq!(mg.angle_to_pulse(0), 1000);
        assert_eq!(mg.angle_to_pulse(120), 2000);
    }

    #[test]
    fn reversed_servo_inverts() {
        let mut rev = catalog::SERVO_GENERIC_180;
        rev.reversed = true;
        assert_eq!(rev.angle_to_pulse(0), 2500);
        assert_eq!(rev.angle_to_pulse(180), 500);
    }

    #[test]
    fn continuous_servo_speed_center_is_stop() {
        let c = catalog::SERVO_CONTINUOUS;
        assert_eq!(c.speed_to_pulse(0), 1500);
        assert_eq!(c.speed_to_pulse(1000), 2000);
        assert_eq!(c.speed_to_pulse(-1000), 1000);
    }

    #[test]
    fn esc_throttle_uni_and_bidir() {
        let uni = catalog::ESC_UNIDIR;
        assert_eq!(uni.arm_pulse(), 1000);
        assert_eq!(uni.throttle_to_pulse(0), 1000);
        assert_eq!(uni.throttle_to_pulse(1000), 2000);
        assert_eq!(uni.throttle_to_pulse(500), 1500);
        let bi = catalog::ESC_BIDIR;
        assert_eq!(bi.throttle_to_pulse(0), 1500); // center = stop
        assert_eq!(bi.throttle_to_pulse(-1000), 1000);
        assert_eq!(bi.throttle_to_pulse(1000), 2000);
    }

    #[test]
    fn sensor_registry_finds_and_identifies() {
        let mut reg = SensorRegistry::<8>::new();
        reg.register(sensor_catalog::INA3221);
        reg.register(sensor_catalog::BMP280);
        reg.register(sensor_catalog::MPU6050);
        assert_eq!(reg.len(), 3);
        assert_eq!(
            reg.find_kind(SensorKind::Power).map(|d| d.name),
            Some("INA3221 3-ch power monitor")
        );
        // two IMUs share address 0x68; identity picks the right one from WHO_AM_I
        let is_mpu6050 = |r: u8| if r == 0x75 { 0x68 } else { 0 };
        assert_eq!(
            reg.identify_at(0x68, is_mpu6050).map(|d| d.name),
            Some("MPU-6050 6-axis IMU")
        );
        // no chip answering -> None
        assert!(reg.identify_at(0x76, |_| 0x00).is_none());
    }
}

// ---------------------------------------------------------------- board modules

/// A dev board as a mountable module: identity + capacity + the platform feature the HAL
/// needs. Mirrors core/boards/<platform>/<id>/board.json so third parties extend the board list by a
/// data drop + a `pub const` here (or their own crate).
#[derive(Clone, Copy, Debug)]
pub struct BoardModule {
    pub board_id: &'static str,
    pub platform_id: &'static str,
    pub feature: &'static str,
    pub flash_budget_bytes: u32,
    pub ram_budget_bytes: u32,
    pub has_usb: bool,
    pub has_radio: bool,
}

/// Fixed-capacity registry of board modules - `enumerate what this build supports` and
/// look one up by id. The "supported board list" is data others append to.
pub struct BoardRegistry<const N: usize> {
    items: [Option<BoardModule>; N],
    len: usize,
}

impl<const N: usize> Default for BoardRegistry<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> BoardRegistry<N> {
    pub const fn new() -> Self {
        Self {
            items: [None; N],
            len: 0,
        }
    }
    pub fn register(&mut self, b: BoardModule) -> bool {
        if self.len >= N || self.find(b.board_id).is_some() {
            return false;
        }
        self.items[self.len] = Some(b);
        self.len += 1;
        true
    }
    pub fn find(&self, board_id: &str) -> Option<BoardModule> {
        self.items
            .iter()
            .flatten()
            .copied()
            .find(|b| b.board_id == board_id)
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Built-in board catalog (matches core/boards/<platform>/*/board.json).
pub mod board_catalog {
    use super::BoardModule;

    pub const NRF52840_NOSD: BoardModule = BoardModule {
        board_id: "promicro_nrf52840_nosd",
        platform_id: "nrf52840",
        feature: "board-promicro-nosd",
        flash_budget_bytes: 81920,
        ram_budget_bytes: 32768,
        has_usb: true,
        has_radio: true,
    };
    pub const NRF52840_S140: BoardModule = BoardModule {
        board_id: "promicro_nrf52840_s140",
        platform_id: "nrf52840",
        feature: "board-nicenano-s140",
        flash_budget_bytes: 81920,
        ram_budget_bytes: 32768,
        has_usb: true,
        has_radio: true,
    };
    pub const RP2350_PICO2W: BoardModule = BoardModule {
        board_id: "rp2350_pico2w",
        platform_id: "rp2350",
        feature: "port-rp2350",
        flash_budget_bytes: 131072,
        ram_budget_bytes: 65536,
        has_usb: true,
        has_radio: false,
    };
    pub const ESP32C3: BoardModule = BoardModule {
        board_id: "esp32c3_supermini",
        platform_id: "esp32c3",
        feature: "port-esp32c3",
        flash_budget_bytes: 131072,
        ram_budget_bytes: 65536,
        has_usb: true,
        has_radio: true,
    };
}

#[cfg(test)]
mod board_tests {
    use super::*;

    #[test]
    fn board_registry_enumerates_and_finds() {
        let mut reg = BoardRegistry::<8>::new();
        assert!(reg.register(board_catalog::NRF52840_NOSD));
        assert!(reg.register(board_catalog::RP2350_PICO2W));
        assert!(reg.register(board_catalog::ESP32C3));
        assert!(!reg.register(board_catalog::NRF52840_NOSD)); // dup id rejected
        assert_eq!(reg.len(), 3);
        assert_eq!(
            reg.find("rp2350_pico2w").map(|b| b.platform_id),
            Some("rp2350")
        );
        assert!(reg.find("rp2350_pico2w").unwrap().has_usb);
        assert!(!reg.find("rp2350_pico2w").unwrap().has_radio);
        assert!(reg.find("unknown").is_none());
    }
}

#[cfg(test)]
mod m203_tests {
    use super::*;

    #[test]
    fn stepper_angle_and_speed_convert_per_brand() {
        let nema = device_catalog::STEPPER_NEMA17; // 200 * 16 = 3200 steps/rev
        assert_eq!(nema.angle_to_steps(360_000), 3200);
        assert_eq!(nema.angle_to_steps(90_000), 800);
        assert_eq!(nema.angle_to_steps(-90_000), -800);
        assert_eq!(nema.rpm_to_sps(60), 3200); // 1 rev/s
        assert_eq!(nema.rpm_to_sps(100_000), nema.max_sps); // clamped
                                                            // the geared 28BYJ-48 needs a different step count for the same angle
        let byj = device_catalog::STEPPER_28BYJ48;
        assert_eq!(byj.angle_to_steps(360_000), 2048);
    }

    #[test]
    fn led_strip_frames_and_timing_are_data() {
        let ws = device_catalog::LED_WS2812B;
        assert_eq!(ws.frame_bytes(60), 180);
        assert_eq!(ws.color_order, ColorOrder::Grb);
        assert!(ws.t1h_ns > ws.t0h_ns);
        let sk = device_catalog::LED_SK6812_RGBW;
        assert_eq!(sk.frame_bytes(60), 240); // RGBW = 4 bytes/pixel
        assert_eq!(device_catalog::LED_APA102.period_ns, 0); // clocked strip
    }

    #[test]
    fn relay_polarity_is_explicit() {
        assert!(!device_catalog::RELAY_ACTIVE_LOW.drive_level(true)); // ON = LOW pin
        assert!(device_catalog::RELAY_ACTIVE_LOW.drive_level(false));
        assert!(device_catalog::RELAY_ACTIVE_HIGH.drive_level(true));
    }

    #[test]
    fn display_geometry_drives_framebuffer_size() {
        assert_eq!(
            device_catalog::OLED_SSD1306_128X64.framebuffer_bytes(),
            1024
        );
        assert_eq!(device_catalog::OLED_SH1106_128X64.col_offset, 2);
    }

    #[test]
    fn nmea_checksum_matches_known_sentence() {
        // $GPGLL,4916.45,N,12311.12,W,225444,A,*1D (classic NMEA reference example)
        let body = b"GPGLL,4916.45,N,12311.12,W,225444,A,";
        assert_eq!(nmea_checksum(body), 0x1D);
    }

    #[test]
    fn ranger_converts_echo_and_rejects_out_of_window() {
        let hc = device_catalog::RANGER_HCSR04;
        // 5830 us round trip ~= 1 m
        assert_eq!(hc.echo_us_to_mm(5830), Some(999));
        assert_eq!(hc.echo_us_to_mm(1), None); // below min
        assert_eq!(hc.echo_us_to_mm(60_000), None); // beyond max
    }

    #[test]
    fn tof_ranger_identity_check() {
        let tof = sensor_catalog::VL53L0X;
        assert!(tof.identify(|reg| if reg == 0xC0 { 0xEE } else { 0 }));
        assert!(!tof.identify(|_| 0x00));
    }
}
