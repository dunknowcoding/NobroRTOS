//! Static system manifest for partitioning, budgets, and capability ownership.

use crate::{FaultThresholds, ModuleId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Criticality {
    BestEffort = 0,
    User = 1,
    Driver = 2,
    System = 3,
    HardRealtime = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Capability {
    Timebase = 0,
    DeadlineTimer = 1,
    EventCapture = 2,
    Bus0 = 3,
    Bus1 = 4,
    Radio = 5,
    ServoPwm = 6,
    Stream = 7,
    Crypto = 8,
    SamplePool = 9,
    HostReport = 10,
}

impl Capability {
    pub const fn bit(self) -> u32 {
        1u32 << (self as u8)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CapabilitySet(u32);

impl CapabilitySet {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn with(self, capability: Capability) -> Self {
        Self(self.0 | capability.bit())
    }

    pub const fn contains(self, capability: Capability) -> bool {
        (self.0 & capability.bit()) != 0
    }

    pub const fn contains_all(self, required: Self) -> bool {
        (self.0 & required.0) == required.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryBudget {
    pub flash_bytes: u32,
    pub ram_bytes: u32,
    pub pool_slots: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemBudget {
    pub flash_bytes: u32,
    pub ram_bytes: u32,
    pub pool_slots: u16,
}

impl SystemBudget {
    pub const ZERO: Self = Self {
        flash_bytes: 0,
        ram_bytes: 0,
        pool_slots: 0,
    };

    pub const fn new(flash_bytes: u32, ram_bytes: u32, pool_slots: u16) -> Self {
        Self {
            flash_bytes,
            ram_bytes,
            pool_slots,
        }
    }

    pub const fn from_memory(memory: MemoryBudget) -> Self {
        Self {
            flash_bytes: memory.flash_bytes,
            ram_bytes: memory.ram_bytes,
            pool_slots: memory.pool_slots,
        }
    }

    pub const fn fits_within(self, limit: Self) -> bool {
        self.flash_bytes <= limit.flash_bytes
            && self.ram_bytes <= limit.ram_bytes
            && self.pool_slots <= limit.pool_slots
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            flash_bytes: self.flash_bytes.checked_add(other.flash_bytes)?,
            ram_bytes: self.ram_bytes.checked_add(other.ram_bytes)?,
            pool_slots: self.pool_slots.checked_add(other.pool_slots)?,
        })
    }

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            flash_bytes: self.flash_bytes.checked_sub(other.flash_bytes)?,
            ram_bytes: self.ram_bytes.checked_sub(other.ram_bytes)?,
            pool_slots: self.pool_slots.checked_sub(other.pool_slots)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemProfile {
    pub flash_limit_bytes: u32,
    pub ram_limit_bytes: u32,
    pub pool_slot_limit: u16,
    pub max_modules: usize,
}

impl SystemProfile {
    pub const NRF52840_CORE: Self = Self {
        flash_limit_bytes: 80 * 1024,
        ram_limit_bytes: 32 * 1024,
        pool_slot_limit: 8,
        max_modules: 16,
    };

    pub const fn new(
        flash_limit_bytes: u32,
        ram_limit_bytes: u32,
        pool_slot_limit: u16,
        max_modules: usize,
    ) -> Self {
        Self {
            flash_limit_bytes,
            ram_limit_bytes,
            pool_slot_limit,
            max_modules,
        }
    }

    pub const fn budget(self) -> SystemBudget {
        SystemBudget::new(
            self.flash_limit_bytes,
            self.ram_limit_bytes,
            self.pool_slot_limit,
        )
    }
}

impl MemoryBudget {
    pub const ZERO: Self = Self {
        flash_bytes: 0,
        ram_bytes: 0,
        pool_slots: 0,
    };

    pub const fn new(flash_bytes: u32, ram_bytes: u32, pool_slots: u16) -> Self {
        Self {
            flash_bytes,
            ram_bytes,
            pool_slots,
        }
    }

    pub const fn is_empty(self) -> bool {
        self.flash_bytes == 0 && self.ram_bytes == 0 && self.pool_slots == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeadlineContract {
    pub period_us: u32,
    pub max_jitter_us: u32,
}

impl DeadlineContract {
    pub const fn new(period_us: u32, max_jitter_us: u32) -> Self {
        Self {
            period_us,
            max_jitter_us,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleSpec {
    pub id: ModuleId,
    pub criticality: Criticality,
    pub requires: CapabilitySet,
    pub owns: CapabilitySet,
    pub memory: MemoryBudget,
    pub deadline: Option<DeadlineContract>,
    pub fault_thresholds: FaultThresholds,
}

impl ModuleSpec {
    pub const fn new(id: ModuleId, criticality: Criticality) -> Self {
        Self {
            id,
            criticality,
            requires: CapabilitySet::empty(),
            owns: CapabilitySet::empty(),
            memory: MemoryBudget::ZERO,
            deadline: None,
            fault_thresholds: FaultThresholds::DEFAULT,
        }
    }

    pub const fn requires(mut self, capabilities: CapabilitySet) -> Self {
        self.requires = capabilities;
        self
    }

    pub const fn owns(mut self, capabilities: CapabilitySet) -> Self {
        self.owns = capabilities;
        self
    }

    pub const fn memory(mut self, memory: MemoryBudget) -> Self {
        self.memory = memory;
        self
    }

    pub const fn deadline(mut self, deadline: DeadlineContract) -> Self {
        self.deadline = Some(deadline);
        self
    }

    pub const fn fault_thresholds(mut self, thresholds: FaultThresholds) -> Self {
        self.fault_thresholds = thresholds;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestError {
    Full,
    DuplicateModule(ModuleId),
    CapabilityOwnershipConflict {
        module: ModuleId,
        capability_bits: u32,
    },
    MissingOwnedCapability {
        module: ModuleId,
        capability_bits: u32,
    },
    MissingDeadline(ModuleId),
    InvalidDeadline(ModuleId),
    InvalidFaultThreshold(ModuleId),
    EmptyMemoryBudget(ModuleId),
    ModuleLimitExceeded {
        modules: usize,
        limit: usize,
    },
    BudgetExceeded {
        used: SystemBudget,
        limit: SystemBudget,
    },
    UserOwnsKernelCapability(ModuleId),
}

pub struct SystemManifest<const N: usize> {
    modules: [Option<ModuleSpec>; N],
}

impl<const N: usize> SystemManifest<N> {
    pub const fn new() -> Self {
        Self { modules: [None; N] }
    }

    pub fn from_specs(specs: &[ModuleSpec]) -> Result<Self, ManifestError> {
        let mut manifest = Self::new();
        for spec in specs {
            manifest.add(*spec)?;
        }
        Ok(manifest)
    }

    pub fn add(&mut self, spec: ModuleSpec) -> Result<(), ManifestError> {
        if self
            .modules
            .iter()
            .flatten()
            .any(|existing| existing.id == spec.id)
        {
            return Err(ManifestError::DuplicateModule(spec.id));
        }

        let Some(slot) = self.modules.iter_mut().find(|slot| slot.is_none()) else {
            return Err(ManifestError::Full);
        };
        *slot = Some(spec);
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        let mut owned = CapabilitySet::empty();
        for spec in self.modules.iter().flatten() {
            if spec.owns.intersects(owned) {
                return Err(ManifestError::CapabilityOwnershipConflict {
                    module: spec.id,
                    capability_bits: spec.owns.intersection(owned).bits(),
                });
            }
            owned = owned.union(spec.owns);
        }

        for spec in self.modules.iter().flatten() {
            self.validate_spec(*spec, owned)?;
        }
        Ok(())
    }

    pub fn validate_profile(&self, profile: SystemProfile) -> Result<(), ManifestError> {
        self.validate()?;
        if self.len() > profile.max_modules {
            return Err(ManifestError::ModuleLimitExceeded {
                modules: self.len(),
                limit: profile.max_modules,
            });
        }

        let used = self.total_budget();
        let limit = profile.budget();
        if !used.fits_within(limit) {
            return Err(ManifestError::BudgetExceeded { used, limit });
        }

        Ok(())
    }

    pub fn total_budget(&self) -> SystemBudget {
        let mut total = SystemBudget::ZERO;
        for spec in self.modules.iter().flatten() {
            total = total
                .checked_add(SystemBudget::from_memory(spec.memory))
                .unwrap_or(SystemBudget {
                    flash_bytes: u32::MAX,
                    ram_bytes: u32::MAX,
                    pool_slots: u16::MAX,
                });
        }
        total
    }

    pub fn iter(&self) -> impl Iterator<Item = ModuleSpec> + '_ {
        self.modules.iter().flatten().copied()
    }

    pub fn provided_capabilities(&self) -> CapabilitySet {
        self.modules
            .iter()
            .flatten()
            .fold(CapabilitySet::empty(), |acc, spec| acc.union(spec.owns))
    }

    pub fn len(&self) -> usize {
        self.modules.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn validate_spec(&self, spec: ModuleSpec, owned: CapabilitySet) -> Result<(), ManifestError> {
        if !owned.contains_all(spec.requires) {
            return Err(ManifestError::MissingOwnedCapability {
                module: spec.id,
                capability_bits: spec.requires.bits() & !owned.bits(),
            });
        }

        if spec.memory.is_empty() {
            return Err(ManifestError::EmptyMemoryBudget(spec.id));
        }

        if spec.fault_thresholds.notify_after == 0
            || spec.fault_thresholds.reboot_after < spec.fault_thresholds.notify_after
        {
            return Err(ManifestError::InvalidFaultThreshold(spec.id));
        }

        if spec.criticality == Criticality::HardRealtime {
            let Some(deadline) = spec.deadline else {
                return Err(ManifestError::MissingDeadline(spec.id));
            };
            if deadline.period_us == 0 || deadline.max_jitter_us == 0 {
                return Err(ManifestError::InvalidDeadline(spec.id));
            }
        }

        if spec.criticality <= Criticality::User
            && spec.owns.intersects(kernel_owned_capabilities())
        {
            return Err(ManifestError::UserOwnsKernelCapability(spec.id));
        }

        Ok(())
    }
}

impl<const N: usize> Default for SystemManifest<N> {
    fn default() -> Self {
        Self::new()
    }
}

pub const fn kernel_owned_capabilities() -> CapabilitySet {
    CapabilitySet::empty()
        .with(Capability::Timebase)
        .with(Capability::DeadlineTimer)
        .with(Capability::SamplePool)
        .with(Capability::HostReport)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kernel_spec() -> ModuleSpec {
        ModuleSpec::new(ModuleId::Kernel, Criticality::HardRealtime)
            .owns(kernel_owned_capabilities())
            .memory(MemoryBudget::new(24 * 1024, 8 * 1024, 8))
            .deadline(DeadlineContract::new(20_000, 10))
    }

    fn sensor_spec() -> ModuleSpec {
        ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
            .requires(
                CapabilitySet::empty()
                    .with(Capability::Bus0)
                    .with(Capability::SamplePool),
            )
            .owns(CapabilitySet::empty().with(Capability::Bus0))
            .memory(MemoryBudget::new(12 * 1024, 2 * 1024, 2))
    }

    #[test]
    fn valid_manifest_accepts_owned_dependencies() {
        let mut manifest = SystemManifest::<4>::new();
        manifest.add(kernel_spec()).unwrap();
        manifest.add(sensor_spec()).unwrap();

        assert_eq!(manifest.len(), 2);
        assert!(manifest
            .provided_capabilities()
            .contains(Capability::DeadlineTimer));
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn duplicate_module_is_rejected() {
        let mut manifest = SystemManifest::<2>::new();
        manifest.add(kernel_spec()).unwrap();
        let err = manifest.add(kernel_spec()).unwrap_err();
        assert_eq!(err, ManifestError::DuplicateModule(ModuleId::Kernel));
    }

    #[test]
    fn duplicate_capability_owner_is_rejected() {
        let mut manifest = SystemManifest::<3>::new();
        manifest.add(kernel_spec()).unwrap();
        manifest.add(sensor_spec()).unwrap();
        manifest
            .add(
                ModuleSpec::new(ModuleId::Radio, Criticality::Driver)
                    .owns(CapabilitySet::empty().with(Capability::Bus0))
                    .memory(MemoryBudget::new(8 * 1024, 2 * 1024, 0)),
            )
            .unwrap();

        assert!(matches!(
            manifest.validate(),
            Err(ManifestError::CapabilityOwnershipConflict { .. })
        ));
    }

    #[test]
    fn hard_realtime_module_needs_deadline_contract() {
        let mut manifest = SystemManifest::<1>::new();
        manifest
            .add(
                ModuleSpec::new(ModuleId::Actuator, Criticality::HardRealtime)
                    .memory(MemoryBudget::new(4 * 1024, 512, 0)),
            )
            .unwrap();

        assert_eq!(
            manifest.validate(),
            Err(ManifestError::MissingDeadline(ModuleId::Actuator))
        );
    }

    #[test]
    fn user_module_cannot_own_kernel_capability() {
        let mut manifest = SystemManifest::<1>::new();
        manifest
            .add(
                ModuleSpec::new(ModuleId::App(1), Criticality::User)
                    .owns(CapabilitySet::empty().with(Capability::DeadlineTimer))
                    .memory(MemoryBudget::new(8 * 1024, 2 * 1024, 0)),
            )
            .unwrap();

        assert_eq!(
            manifest.validate(),
            Err(ManifestError::UserOwnsKernelCapability(ModuleId::App(1)))
        );
    }

    #[test]
    fn profile_budget_accepts_core_manifest() {
        let mut manifest = SystemManifest::<4>::new();
        manifest.add(kernel_spec()).unwrap();
        manifest.add(sensor_spec()).unwrap();

        assert_eq!(
            manifest.total_budget(),
            SystemBudget::new(36 * 1024, 10 * 1024, 10)
        );
        assert!(manifest
            .validate_profile(SystemProfile {
                pool_slot_limit: 10,
                ..SystemProfile::NRF52840_CORE
            })
            .is_ok());
    }

    #[test]
    fn profile_budget_rejects_flash_overflow() {
        let mut manifest = SystemManifest::<2>::new();
        manifest.add(kernel_spec()).unwrap();
        manifest
            .add(
                ModuleSpec::new(ModuleId::App(2), Criticality::User)
                    .requires(CapabilitySet::empty().with(Capability::HostReport))
                    .memory(MemoryBudget::new(90 * 1024, 2 * 1024, 0)),
            )
            .unwrap();

        assert!(matches!(
            manifest.validate_profile(SystemProfile::NRF52840_CORE),
            Err(ManifestError::BudgetExceeded { .. })
        ));
    }

    #[test]
    fn profile_budget_rejects_module_overflow() {
        let mut manifest = SystemManifest::<2>::new();
        manifest.add(kernel_spec()).unwrap();
        manifest.add(sensor_spec()).unwrap();

        assert_eq!(
            manifest.validate_profile(SystemProfile {
                max_modules: 1,
                pool_slot_limit: 10,
                ..SystemProfile::NRF52840_CORE
            }),
            Err(ManifestError::ModuleLimitExceeded {
                modules: 2,
                limit: 1
            })
        );
    }

    #[test]
    fn manifest_can_be_built_from_specs() {
        let manifest = SystemManifest::<2>::from_specs(&[kernel_spec(), sensor_spec()]).unwrap();

        assert_eq!(manifest.len(), 2);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn manifest_from_specs_preserves_duplicate_errors() {
        assert!(matches!(
            SystemManifest::<2>::from_specs(&[kernel_spec(), kernel_spec()]),
            Err(ManifestError::DuplicateModule(ModuleId::Kernel))
        ));
    }
}
