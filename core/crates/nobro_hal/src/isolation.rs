//! Fixed-capacity module-isolation admission and lifecycle.
//!
//! Logical capabilities are not memory isolation. This module issues a receipt
//! only for an exact hardware protection provider and keeps that receipt bound
//! to one module, provider generation, context generation, region plan, DMA
//! owner, and peripheral allowlist. The MPU backend consumes the same receipt;
//! peripheral and DMA leases can therefore reject authority that faulted or was
//! superseded by recovery.

use core::sync::atomic::{AtomicU32, Ordering};

pub const MAX_ISOLATION_REGIONS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IsolationArchitecture {
    None = 0,
    PmsaV7M = 1,
    PmsaV8M = 2,
    Mmu = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IsolationRegionRole {
    Code = 1,
    Data = 2,
    Stack = 3,
    StackGuard = 4,
    Peripheral = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IsolationAccess {
    NoAccess = 0,
    ReadOnly = 1,
    ReadWrite = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IsolationRegion {
    pub base: u32,
    pub size_bytes: u32,
    pub role: IsolationRegionRole,
    pub access: IsolationAccess,
    pub executable: bool,
    /// Stable logical peripheral id. It must be zero for non-peripheral
    /// regions and in `1..=63` for a peripheral region.
    pub peripheral_id: u8,
}

impl IsolationRegion {
    pub const fn code(base: u32, size_bytes: u32) -> Self {
        Self {
            base,
            size_bytes,
            role: IsolationRegionRole::Code,
            access: IsolationAccess::ReadOnly,
            executable: true,
            peripheral_id: 0,
        }
    }

    pub const fn data(base: u32, size_bytes: u32) -> Self {
        Self {
            base,
            size_bytes,
            role: IsolationRegionRole::Data,
            access: IsolationAccess::ReadWrite,
            executable: false,
            peripheral_id: 0,
        }
    }

    pub const fn stack(base: u32, size_bytes: u32) -> Self {
        Self {
            base,
            size_bytes,
            role: IsolationRegionRole::Stack,
            access: IsolationAccess::ReadWrite,
            executable: false,
            peripheral_id: 0,
        }
    }

    pub const fn stack_guard(base: u32, size_bytes: u32) -> Self {
        Self {
            base,
            size_bytes,
            role: IsolationRegionRole::StackGuard,
            access: IsolationAccess::NoAccess,
            executable: false,
            peripheral_id: 0,
        }
    }

    pub const fn peripheral(base: u32, size_bytes: u32, peripheral_id: u8) -> Self {
        Self {
            base,
            size_bytes,
            role: IsolationRegionRole::Peripheral,
            access: IsolationAccess::ReadWrite,
            executable: false,
            peripheral_id,
        }
    }

    pub fn end_exclusive(self) -> Option<u32> {
        self.base.checked_add(self.size_bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IsolationCapabilities {
    pub(crate) provider_id: u32,
    pub(crate) generation: u32,
    pub(crate) architecture: IsolationArchitecture,
    pub(crate) region_count: u8,
    pub(crate) unprivileged_thread: bool,
    pub(crate) attributable_faults: bool,
    pub(crate) restartable_context: bool,
}

impl IsolationCapabilities {
    pub const fn unavailable(provider_id: u32, generation: u32) -> Self {
        Self {
            provider_id,
            generation,
            architecture: IsolationArchitecture::None,
            region_count: 0,
            unprivileged_thread: false,
            attributable_faults: false,
            restartable_context: false,
        }
    }

    #[cfg(any(target_arch = "arm", test))]
    pub(crate) const fn pmsa(
        provider_id: u32,
        generation: u32,
        architecture: IsolationArchitecture,
        region_count: u8,
    ) -> Self {
        Self {
            provider_id,
            generation,
            architecture,
            region_count,
            unprivileged_thread: true,
            attributable_faults: true,
            restartable_context: true,
        }
    }

    pub const fn provider_id(self) -> u32 {
        self.provider_id
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }

    pub const fn architecture(self) -> IsolationArchitecture {
        self.architecture
    }

    pub const fn region_count(self) -> u8 {
        self.region_count
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsolationError {
    MissingModule,
    MissingLeaseOwner,
    MissingProvider,
    MissingProviderGeneration,
    UnsupportedArchitecture,
    HardwareLifecycleUnavailable,
    EmptyPlan,
    TooManyRegions { requested: usize, available: usize },
    InvalidRange { index: usize },
    MisalignedRange { index: usize },
    InvalidRegionContract { index: usize },
    OverlappingRegions { first: usize, second: usize },
    MissingCode,
    MissingData,
    MissingStack,
    DuplicatePeripheral(u8),
    Busy,
    GenerationExhausted,
    StaleReceipt,
    WrongState,
    PlanChanged,
    ProviderChanged,
    PeripheralDenied(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IsolationPlan<const N: usize> {
    module_id: u16,
    lease_owner: u16,
    regions: [Option<IsolationRegion>; N],
    len: usize,
}

impl<const N: usize> IsolationPlan<N> {
    pub const fn new(module_id: u16, lease_owner: u16) -> Self {
        Self {
            module_id,
            lease_owner,
            regions: [None; N],
            len: 0,
        }
    }

    pub fn add(&mut self, region: IsolationRegion) -> Result<usize, IsolationError> {
        if self.len == N || self.len == MAX_ISOLATION_REGIONS {
            return Err(IsolationError::TooManyRegions {
                requested: self.len + 1,
                available: N.min(MAX_ISOLATION_REGIONS),
            });
        }
        let index = self.len;
        self.regions[index] = Some(region);
        self.len += 1;
        Ok(index)
    }

    pub const fn module_id(&self) -> u16 {
        self.module_id
    }

    pub const fn lease_owner(&self) -> u16 {
        self.lease_owner
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn regions(&self) -> impl Iterator<Item = IsolationRegion> + '_ {
        self.regions[..self.len].iter().flatten().copied()
    }

    pub fn region(&self, index: usize) -> Option<IsolationRegion> {
        self.regions.get(index).copied().flatten()
    }

    pub fn fingerprint(&self) -> u32 {
        let mut hash = 0x811C_9DC5u32;
        hash = hash_word(hash, u32::from(self.module_id));
        hash = hash_word(hash, u32::from(self.lease_owner));
        hash = hash_word(hash, self.len as u32);
        for region in self.regions() {
            hash = hash_word(hash, region.base);
            hash = hash_word(hash, region.size_bytes);
            hash = hash_word(hash, region.role as u32);
            hash = hash_word(hash, region.access as u32);
            hash = hash_word(hash, u32::from(region.executable));
            hash = hash_word(hash, u32::from(region.peripheral_id));
        }
        hash
    }

    fn validate(&self, capabilities: IsolationCapabilities) -> Result<u64, IsolationError> {
        if self.module_id == 0 {
            return Err(IsolationError::MissingModule);
        }
        if self.lease_owner == 0 {
            return Err(IsolationError::MissingLeaseOwner);
        }
        if self.len == 0 {
            return Err(IsolationError::EmptyPlan);
        }
        if self.len > usize::from(capabilities.region_count) {
            return Err(IsolationError::TooManyRegions {
                requested: self.len,
                available: usize::from(capabilities.region_count),
            });
        }

        let mut code = false;
        let mut data = false;
        let mut stack = false;
        let mut peripheral_mask = 0u64;
        for index in 0..self.len {
            let region = self.regions[index].ok_or(IsolationError::InvalidRange { index })?;
            validate_region(region, index, capabilities.architecture)?;
            match region.role {
                IsolationRegionRole::Code => code = true,
                IsolationRegionRole::Data => data = true,
                IsolationRegionRole::Stack => stack = true,
                IsolationRegionRole::Peripheral => {
                    let bit = 1u64 << region.peripheral_id;
                    if peripheral_mask & bit != 0 {
                        return Err(IsolationError::DuplicatePeripheral(region.peripheral_id));
                    }
                    peripheral_mask |= bit;
                }
                IsolationRegionRole::StackGuard => {}
            }
            let end = region
                .end_exclusive()
                .ok_or(IsolationError::InvalidRange { index })?;
            for previous in 0..index {
                let other = self.regions[previous]
                    .ok_or(IsolationError::InvalidRange { index: previous })?;
                let other_end = other
                    .end_exclusive()
                    .ok_or(IsolationError::InvalidRange { index: previous })?;
                if region.base < other_end && other.base < end {
                    return Err(IsolationError::OverlappingRegions {
                        first: previous,
                        second: index,
                    });
                }
            }
        }
        if !code {
            return Err(IsolationError::MissingCode);
        }
        if !data {
            return Err(IsolationError::MissingData);
        }
        if !stack {
            return Err(IsolationError::MissingStack);
        }
        Ok(peripheral_mask)
    }
}

const fn hash_word(mut hash: u32, word: u32) -> u32 {
    let mut shift = 0;
    while shift < 32 {
        hash ^= (word >> shift) & 0xFF;
        hash = hash.wrapping_mul(0x0100_0193);
        shift += 8;
    }
    hash
}

fn validate_region(
    region: IsolationRegion,
    index: usize,
    architecture: IsolationArchitecture,
) -> Result<(), IsolationError> {
    if region.size_bytes == 0 || region.end_exclusive().is_none() {
        return Err(IsolationError::InvalidRange { index });
    }
    match architecture {
        IsolationArchitecture::PmsaV7M => {
            if region.size_bytes < 32 || !region.size_bytes.is_power_of_two() {
                return Err(IsolationError::InvalidRange { index });
            }
            if region.base & (region.size_bytes - 1) != 0 {
                return Err(IsolationError::MisalignedRange { index });
            }
        }
        IsolationArchitecture::PmsaV8M => {
            if region.size_bytes < 32 || region.size_bytes & 31 != 0 || region.base & 31 != 0 {
                return Err(IsolationError::MisalignedRange { index });
            }
        }
        IsolationArchitecture::None | IsolationArchitecture::Mmu => {
            return Err(IsolationError::UnsupportedArchitecture);
        }
    }

    let contract_ok = match region.role {
        IsolationRegionRole::Code => {
            region.access == IsolationAccess::ReadOnly
                && region.executable
                && region.peripheral_id == 0
        }
        IsolationRegionRole::Data | IsolationRegionRole::Stack => {
            region.access == IsolationAccess::ReadWrite
                && !region.executable
                && region.peripheral_id == 0
        }
        IsolationRegionRole::StackGuard => {
            region.access == IsolationAccess::NoAccess
                && !region.executable
                && region.peripheral_id == 0
        }
        IsolationRegionRole::Peripheral => {
            region.access == IsolationAccess::ReadWrite
                && !region.executable
                && (1..=63).contains(&region.peripheral_id)
        }
    };
    contract_ok
        .then_some(())
        .ok_or(IsolationError::InvalidRegionContract { index })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum IsolationState {
    Empty = 0,
    Admitted = 1,
    Active = 2,
    Faulted = 3,
    Recovering = 4,
    Stopped = 5,
    /// Internal fail-closed state while the generation and lifecycle state are
    /// advanced as one transaction.
    Reconfiguring = 6,
}

impl IsolationState {
    fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Empty),
            1 => Some(Self::Admitted),
            2 => Some(Self::Active),
            3 => Some(Self::Faulted),
            4 => Some(Self::Recovering),
            5 => Some(Self::Stopped),
            6 => Some(Self::Reconfiguring),
            _ => None,
        }
    }
}

/// Per-module generation authority. Keep this object in static storage for as
/// long as any receipt or MPU context can exist.
#[derive(Debug)]
pub struct IsolationEpoch {
    context_generation: AtomicU32,
    state: AtomicU32,
}

impl IsolationEpoch {
    pub const fn new() -> Self {
        Self {
            context_generation: AtomicU32::new(0),
            state: AtomicU32::new(IsolationState::Empty as u32),
        }
    }

    pub fn state(&self) -> IsolationState {
        IsolationState::from_raw(self.state.load(Ordering::Acquire))
            .unwrap_or(IsolationState::Faulted)
    }

    fn transition_state(&self, current: IsolationState, next: IsolationState) -> Result<(), u32> {
        #[cfg(target_has_atomic = "32")]
        {
            self.state
                .compare_exchange(
                    current as u32,
                    next as u32,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .map(|_| ())
        }

        #[cfg(not(target_has_atomic = "32"))]
        {
            critical_section::with(|_| {
                let observed = self.state.load(Ordering::Relaxed);
                if observed == current as u32 {
                    self.state.store(next as u32, Ordering::Relaxed);
                    Ok(())
                } else {
                    Err(observed)
                }
            })
        }
    }

    pub fn admit<const N: usize>(
        &'static self,
        plan: &IsolationPlan<N>,
        capabilities: IsolationCapabilities,
    ) -> Result<IsolationReceipt, IsolationError> {
        validate_capabilities(capabilities)?;
        let peripheral_mask = plan.validate(capabilities)?;
        let state = self.state();
        if !matches!(state, IsolationState::Empty | IsolationState::Stopped) {
            return Err(IsolationError::Busy);
        }
        self.transition_state(state, IsolationState::Reconfiguring)
            .map_err(|_| IsolationError::Busy)?;
        let Some(generation) = self
            .context_generation
            .load(Ordering::Acquire)
            .checked_add(1)
        else {
            self.state.store(state as u32, Ordering::Release);
            return Err(IsolationError::GenerationExhausted);
        };
        self.context_generation.store(generation, Ordering::Release);
        self.state
            .store(IsolationState::Admitted as u32, Ordering::Release);
        Ok(IsolationReceipt {
            epoch: self,
            provider_id: capabilities.provider_id,
            provider_generation: capabilities.generation,
            context_generation: generation,
            architecture: capabilities.architecture,
            module_id: plan.module_id,
            lease_owner: plan.lease_owner,
            region_count: plan.len as u8,
            plan_fingerprint: plan.fingerprint(),
            peripheral_mask,
        })
    }

    pub fn activate(&self, receipt: IsolationReceipt) -> Result<(), IsolationError> {
        receipt.ensure_current()?;
        match self.transition_state(IsolationState::Admitted, IsolationState::Active) {
            Ok(()) => Ok(()),
            Err(value) if value == IsolationState::Active as u32 => Ok(()),
            Err(_) => Err(IsolationError::WrongState),
        }
    }

    pub fn fault(&self, receipt: IsolationReceipt) -> Result<(), IsolationError> {
        receipt.ensure_current()?;
        self.transition_state(IsolationState::Active, IsolationState::Faulted)
            .map_err(|_| IsolationError::WrongState)
    }

    pub fn begin_recovery(&self, receipt: IsolationReceipt) -> Result<(), IsolationError> {
        receipt.ensure_current()?;
        self.transition_state(IsolationState::Faulted, IsolationState::Recovering)
            .map_err(|_| IsolationError::WrongState)
    }

    pub fn restart(
        &'static self,
        receipt: IsolationReceipt,
        capabilities: IsolationCapabilities,
    ) -> Result<IsolationReceipt, IsolationError> {
        receipt.ensure_current()?;
        validate_capabilities(capabilities)?;
        if receipt.provider_id != capabilities.provider_id
            || receipt.provider_generation != capabilities.generation
            || receipt.architecture != capabilities.architecture
            || receipt.region_count > capabilities.region_count
        {
            return Err(IsolationError::ProviderChanged);
        }
        let state = self.state();
        if !matches!(state, IsolationState::Faulted | IsolationState::Recovering) {
            return Err(IsolationError::WrongState);
        }
        self.transition_state(state, IsolationState::Reconfiguring)
            .map_err(|_| IsolationError::WrongState)?;
        if let Err(error) = receipt.ensure_current() {
            self.state.store(state as u32, Ordering::Release);
            return Err(error);
        }
        let Some(generation) = receipt.context_generation.checked_add(1) else {
            self.state.store(state as u32, Ordering::Release);
            return Err(IsolationError::GenerationExhausted);
        };
        self.context_generation.store(generation, Ordering::Release);
        self.state
            .store(IsolationState::Admitted as u32, Ordering::Release);
        Ok(IsolationReceipt {
            context_generation: generation,
            ..receipt
        })
    }

    pub fn stop(&self, receipt: IsolationReceipt) -> Result<(), IsolationError> {
        receipt.ensure_current()?;
        let state = self.state();
        if matches!(
            state,
            IsolationState::Empty | IsolationState::Stopped | IsolationState::Reconfiguring
        ) {
            return Err(IsolationError::WrongState);
        }
        self.transition_state(state, IsolationState::Reconfiguring)
            .map_err(|_| IsolationError::WrongState)?;
        if let Err(error) = receipt.ensure_current() {
            self.state.store(state as u32, Ordering::Release);
            return Err(error);
        }
        let Some(generation) = receipt.context_generation.checked_add(1) else {
            self.state.store(state as u32, Ordering::Release);
            return Err(IsolationError::GenerationExhausted);
        };
        self.context_generation.store(generation, Ordering::Release);
        self.state
            .store(IsolationState::Stopped as u32, Ordering::Release);
        Ok(())
    }

    pub fn invalidate_provider(&self) -> Result<(), IsolationError> {
        let state = self.state();
        if state == IsolationState::Reconfiguring {
            return Err(IsolationError::Busy);
        }
        self.transition_state(state, IsolationState::Reconfiguring)
            .map_err(|_| IsolationError::Busy)?;
        let Some(generation) = self
            .context_generation
            .load(Ordering::Acquire)
            .checked_add(1)
        else {
            self.state.store(state as u32, Ordering::Release);
            return Err(IsolationError::GenerationExhausted);
        };
        self.context_generation.store(generation, Ordering::Release);
        self.state
            .store(IsolationState::Stopped as u32, Ordering::Release);
        Ok(())
    }
}

impl Default for IsolationEpoch {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_capabilities(capabilities: IsolationCapabilities) -> Result<(), IsolationError> {
    if capabilities.provider_id == 0 {
        return Err(IsolationError::MissingProvider);
    }
    if capabilities.generation == 0 {
        return Err(IsolationError::MissingProviderGeneration);
    }
    if !matches!(
        capabilities.architecture,
        IsolationArchitecture::PmsaV7M | IsolationArchitecture::PmsaV8M
    ) {
        return Err(IsolationError::UnsupportedArchitecture);
    }
    if capabilities.region_count == 0
        || !capabilities.unprivileged_thread
        || !capabilities.attributable_faults
        || !capabilities.restartable_context
    {
        return Err(IsolationError::HardwareLifecycleUnavailable);
    }
    Ok(())
}

/// Non-forgeable proof of one admitted hardware-isolated module context.
#[derive(Clone, Copy, Debug)]
pub struct IsolationReceipt {
    epoch: &'static IsolationEpoch,
    provider_id: u32,
    provider_generation: u32,
    context_generation: u32,
    architecture: IsolationArchitecture,
    module_id: u16,
    lease_owner: u16,
    region_count: u8,
    plan_fingerprint: u32,
    peripheral_mask: u64,
}

impl PartialEq for IsolationReceipt {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.epoch, other.epoch)
            && self.provider_id == other.provider_id
            && self.provider_generation == other.provider_generation
            && self.context_generation == other.context_generation
            && self.architecture == other.architecture
            && self.module_id == other.module_id
            && self.lease_owner == other.lease_owner
            && self.region_count == other.region_count
            && self.plan_fingerprint == other.plan_fingerprint
            && self.peripheral_mask == other.peripheral_mask
    }
}

impl Eq for IsolationReceipt {}

impl IsolationReceipt {
    pub const fn provider_id(self) -> u32 {
        self.provider_id
    }

    pub const fn provider_generation(self) -> u32 {
        self.provider_generation
    }

    pub const fn context_generation(self) -> u32 {
        self.context_generation
    }

    pub const fn architecture(self) -> IsolationArchitecture {
        self.architecture
    }

    pub const fn module_id(self) -> u16 {
        self.module_id
    }

    pub const fn lease_owner(self) -> u16 {
        self.lease_owner
    }

    pub const fn region_count(self) -> u8 {
        self.region_count
    }

    pub const fn plan_fingerprint(self) -> u32 {
        self.plan_fingerprint
    }

    pub fn ensure_current(self) -> Result<(), IsolationError> {
        if self.epoch.context_generation.load(Ordering::Acquire) != self.context_generation {
            return Err(IsolationError::StaleReceipt);
        }
        Ok(())
    }

    pub fn ensure_usable(self) -> Result<(), IsolationError> {
        self.ensure_current()?;
        if matches!(
            self.epoch.state(),
            IsolationState::Admitted | IsolationState::Active
        ) {
            Ok(())
        } else {
            Err(IsolationError::WrongState)
        }
    }

    pub fn revalidate(self, capabilities: IsolationCapabilities) -> Result<Self, IsolationError> {
        self.ensure_usable()?;
        validate_capabilities(capabilities)?;
        if self.provider_id != capabilities.provider_id
            || self.provider_generation != capabilities.generation
            || self.architecture != capabilities.architecture
            || self.region_count > capabilities.region_count
        {
            return Err(IsolationError::ProviderChanged);
        }
        Ok(self)
    }

    pub fn validate_plan<const N: usize>(
        self,
        plan: &IsolationPlan<N>,
    ) -> Result<(), IsolationError> {
        if self.module_id != plan.module_id
            || self.lease_owner != plan.lease_owner
            || self.region_count as usize != plan.len
            || self.plan_fingerprint != plan.fingerprint()
        {
            return Err(IsolationError::PlanChanged);
        }
        self.ensure_usable()
    }

    pub fn permits_peripheral(self, peripheral_id: u8) -> Result<(), IsolationError> {
        self.ensure_usable()?;
        if peripheral_id == 0
            || peripheral_id > 63
            || self.peripheral_mask & (1u64 << peripheral_id) == 0
        {
            return Err(IsolationError::PeripheralDenied(peripheral_id));
        }
        Ok(())
    }

    #[cfg(target_arch = "arm")]
    pub(crate) fn mark_active(self) -> Result<(), IsolationError> {
        self.epoch.activate(self)
    }

    #[cfg(target_arch = "arm")]
    pub(crate) fn mark_faulted(self) -> Result<(), IsolationError> {
        self.epoch.fault(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v7() -> IsolationCapabilities {
        IsolationCapabilities::pmsa(7, 3, IsolationArchitecture::PmsaV7M, 8)
    }

    fn plan() -> IsolationPlan<5> {
        let mut plan = IsolationPlan::new(5, 5);
        plan.add(IsolationRegion::code(0, 1024 * 1024)).unwrap();
        plan.add(IsolationRegion::data(0x2000_0000, 256)).unwrap();
        plan.add(IsolationRegion::stack(0x2000_0100, 256)).unwrap();
        plan.add(IsolationRegion::stack_guard(0x2000_0200, 32))
            .unwrap();
        plan.add(IsolationRegion::peripheral(0x4000_0000, 4096, 7))
            .unwrap();
        plan
    }

    #[test]
    fn admission_lifecycle_restart_and_stale_receipts_are_distinct() {
        static EPOCH: IsolationEpoch = IsolationEpoch::new();
        let plan = plan();
        let receipt = EPOCH.admit(&plan, v7()).unwrap();
        assert_eq!(EPOCH.state(), IsolationState::Admitted);
        assert!(receipt.validate_plan(&plan).is_ok());
        assert!(receipt.permits_peripheral(7).is_ok());
        assert_eq!(
            receipt.permits_peripheral(8),
            Err(IsolationError::PeripheralDenied(8))
        );

        EPOCH.activate(receipt).unwrap();
        EPOCH.fault(receipt).unwrap();
        assert_eq!(receipt.ensure_usable(), Err(IsolationError::WrongState));
        EPOCH.begin_recovery(receipt).unwrap();
        let restarted = EPOCH.restart(receipt, v7()).unwrap();
        assert_eq!(receipt.ensure_current(), Err(IsolationError::StaleReceipt));
        assert_eq!(restarted.context_generation(), 2);
        assert!(restarted.validate_plan(&plan).is_ok());
    }

    #[test]
    fn unsupported_and_incomplete_hardware_never_issue_a_receipt() {
        static NO_MPU: IsolationEpoch = IsolationEpoch::new();
        let plan = plan();
        assert_eq!(
            NO_MPU.admit(&plan, IsolationCapabilities::unavailable(1, 1)),
            Err(IsolationError::UnsupportedArchitecture)
        );
        let incomplete = IsolationCapabilities {
            attributable_faults: false,
            ..v7()
        };
        assert_eq!(
            NO_MPU.admit(&plan, incomplete),
            Err(IsolationError::HardwareLifecycleUnavailable)
        );
    }

    #[test]
    fn v7_and_v8_alignment_and_region_contracts_fail_closed() {
        static BAD: IsolationEpoch = IsolationEpoch::new();
        let mut v8_plan = IsolationPlan::<4>::new(1, 1);
        v8_plan.add(IsolationRegion::code(0, 96)).unwrap();
        v8_plan.add(IsolationRegion::data(0x2000_0060, 96)).unwrap();
        v8_plan
            .add(IsolationRegion::stack(0x2000_00C0, 96))
            .unwrap();
        let v8 = IsolationCapabilities::pmsa(8, 1, IsolationArchitecture::PmsaV8M, 8);
        assert!(BAD.admit(&v8_plan, v8).is_ok());

        static BAD_V7: IsolationEpoch = IsolationEpoch::new();
        assert_eq!(
            BAD_V7.admit(&v8_plan, v7()),
            Err(IsolationError::InvalidRange { index: 0 })
        );

        static BAD_ROLE: IsolationEpoch = IsolationEpoch::new();
        let mut malformed = plan();
        malformed.regions[0] = Some(IsolationRegion {
            executable: false,
            ..IsolationRegion::code(0, 1024 * 1024)
        });
        assert_eq!(
            BAD_ROLE.admit(&malformed, v7()),
            Err(IsolationError::InvalidRegionContract { index: 0 })
        );
    }

    #[test]
    fn overlap_provider_change_stop_and_generation_exhaustion_are_rejected() {
        static OVERLAP: IsolationEpoch = IsolationEpoch::new();
        let mut overlapping = plan();
        overlapping.regions[2] = Some(IsolationRegion::stack(0x2000_0080, 128));
        assert_eq!(
            OVERLAP.admit(&overlapping, v7()),
            Err(IsolationError::OverlappingRegions {
                first: 1,
                second: 2
            })
        );

        static EPOCH: IsolationEpoch = IsolationEpoch::new();
        let receipt = EPOCH.admit(&plan(), v7()).unwrap();
        assert_eq!(
            receipt.revalidate(IsolationCapabilities {
                generation: 4,
                ..v7()
            }),
            Err(IsolationError::ProviderChanged)
        );
        EPOCH.stop(receipt).unwrap();
        assert_eq!(receipt.ensure_current(), Err(IsolationError::StaleReceipt));
    }
}
