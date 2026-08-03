//! Generation-safe DMA buffer ownership and coherency boundary.
//!
//! MPU isolation does not constrain DMA masters. This registry therefore
//! admits each static buffer separately, rejects overlap, binds the lease to
//! one owner/peripheral/channel, and routes preparation, completion, cancel,
//! and reset through an architecture backend. No handle contains an address.

use crate::isolation::IsolationReceipt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaOwnerId(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaDirection {
    MemoryToPeripheral,
    PeripheralToMemory,
    Bidirectional,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaCoherency {
    /// The architecture has no data cache covering this region.
    Uncached,
    /// The backend must perform explicit clean/invalidate operations.
    SoftwareManaged,
    /// Hardware keeps the DMA master coherent with the CPU.
    HardwareCoherent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaLeaseRequest {
    pub alignment: usize,
    pub direction: DmaDirection,
    pub coherency: DmaCoherency,
    pub peripheral: u16,
    pub channel: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaBufferDescriptor {
    pub address: usize,
    pub len: usize,
    pub direction: DmaDirection,
    pub coherency: DmaCoherency,
    pub peripheral: u16,
    pub channel: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaLease {
    slot: u16,
    generation: u32,
    owner: DmaOwnerId,
}

impl DmaLease {
    pub const fn generation(self) -> u32 {
        self.generation
    }

    pub const fn owner(self) -> DmaOwnerId {
        self.owner
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaLeaseError<E> {
    Full,
    EmptyBuffer,
    InvalidAlignment,
    AddressOverflow,
    Overlap,
    GenerationExhausted,
    InvalidHandle,
    WrongOwner,
    AlreadyActive,
    NotActive,
    TransferTooLong,
    IsolationDenied,
    IsolationStale,
    Backend(E),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaRecoveryReason {
    Timeout,
    PeripheralFault,
    OwnerShutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaCompletionReceipt {
    pub owner: DmaOwnerId,
    pub transferred: usize,
    pub requested: usize,
    pub partial: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaRecoveryReceipt {
    pub owner: DmaOwnerId,
    pub reason: DmaRecoveryReason,
    pub cancel_confirmed: bool,
    pub peripheral_reset: bool,
}

/// Architecture-specific cache and peripheral lifecycle operations.
pub trait DmaLeaseBackend {
    type Error;

    fn prepare(&mut self, descriptor: DmaBufferDescriptor) -> Result<(), Self::Error>;
    fn complete(
        &mut self,
        descriptor: DmaBufferDescriptor,
        transferred: usize,
    ) -> Result<(), Self::Error>;
    fn cancel(&mut self, descriptor: DmaBufferDescriptor) -> Result<(), Self::Error>;
    fn reset(&mut self, peripheral: u16, channel: u16) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DmaLeaseState {
    Reserved,
    Active,
}

#[derive(Clone, Copy, Debug)]
struct DmaLeaseEntry {
    generation: u32,
    owner: DmaOwnerId,
    descriptor: DmaBufferDescriptor,
    state: DmaLeaseState,
    #[cfg(any(feature = "pmsa-v7", feature = "pmsa-v8", test))]
    isolation: Option<IsolationReceipt>,
}

/// Fixed-capacity DMA ownership registry. The static borrow consumed by
/// [`acquire_static`](Self::acquire_static) prevents safe CPU reuse while the
/// registry owns the region; the region becomes reusable only after the lease
/// is completed, cancelled, or recovered and the caller's higher-level static
/// allocator explicitly lends it again.
pub struct DmaLeaseRegistry<const N: usize> {
    entries: [Option<DmaLeaseEntry>; N],
    generations: [u32; N],
}

impl<const N: usize> DmaLeaseRegistry<N> {
    pub const fn new() -> Self {
        Self {
            entries: [None; N],
            generations: [0; N],
        }
    }

    pub fn acquire_static(
        &mut self,
        owner: DmaOwnerId,
        buffer: &'static mut [u8],
        request: DmaLeaseRequest,
    ) -> Result<DmaLease, DmaLeaseError<core::convert::Infallible>> {
        let address = buffer.as_mut_ptr() as usize;
        let len = buffer.len();
        // SAFETY: the consumed static mutable borrow supplies the required
        // lifetime and exclusive ownership.
        unsafe { self.acquire_region_with_isolation(owner, address, len, request, None) }
    }

    /// Acquire a DMA buffer for one still-live hardware-isolated module. The
    /// MPU does not constrain the bus master; this explicit binding does.
    pub fn acquire_isolated_static(
        &mut self,
        isolation: IsolationReceipt,
        buffer: &'static mut [u8],
        request: DmaLeaseRequest,
    ) -> Result<DmaLease, DmaLeaseError<core::convert::Infallible>> {
        isolation
            .ensure_usable()
            .map_err(|_| DmaLeaseError::IsolationStale)?;
        let peripheral =
            u8::try_from(request.peripheral).map_err(|_| DmaLeaseError::IsolationDenied)?;
        isolation
            .permits_peripheral(peripheral)
            .map_err(|error| match error {
                crate::IsolationError::PeripheralDenied(_) => DmaLeaseError::IsolationDenied,
                _ => DmaLeaseError::IsolationStale,
            })?;
        let address = buffer.as_mut_ptr() as usize;
        let len = buffer.len();
        // SAFETY: the consumed static mutable borrow supplies the required
        // lifetime and exclusive ownership.
        unsafe {
            self.acquire_region_with_isolation(
                DmaOwnerId(isolation.lease_owner()),
                address,
                len,
                request,
                Some(isolation),
            )
        }
    }

    /// Admit a statically valid DMA region that is already owned by a driver.
    ///
    /// # Safety
    /// The address range must remain valid and exclusively owned until this
    /// lease is completed, cancelled, or recovered. The caller must also keep
    /// CPU access synchronized with the selected coherency policy.
    pub unsafe fn acquire_region(
        &mut self,
        owner: DmaOwnerId,
        address: usize,
        len: usize,
        request: DmaLeaseRequest,
    ) -> Result<DmaLease, DmaLeaseError<core::convert::Infallible>> {
        self.acquire_region_with_isolation(owner, address, len, request, None)
    }

    unsafe fn acquire_region_with_isolation(
        &mut self,
        owner: DmaOwnerId,
        address: usize,
        len: usize,
        request: DmaLeaseRequest,
        isolation: Option<IsolationReceipt>,
    ) -> Result<DmaLease, DmaLeaseError<core::convert::Infallible>> {
        #[cfg(not(any(feature = "pmsa-v7", feature = "pmsa-v8", test)))]
        if isolation.is_some() {
            return Err(DmaLeaseError::IsolationDenied);
        }
        if let Some(receipt) = isolation {
            if receipt.lease_owner() != owner.0 {
                return Err(DmaLeaseError::IsolationDenied);
            }
            receipt
                .ensure_usable()
                .map_err(|_| DmaLeaseError::IsolationStale)?;
        }
        if len == 0 {
            return Err(DmaLeaseError::EmptyBuffer);
        }
        if request.alignment == 0
            || !request.alignment.is_power_of_two()
            || address & (request.alignment - 1) != 0
        {
            return Err(DmaLeaseError::InvalidAlignment);
        }
        let end = address
            .checked_add(len)
            .ok_or(DmaLeaseError::AddressOverflow)?;
        if self.entries.iter().flatten().any(|entry| {
            let other_end = entry
                .descriptor
                .address
                .saturating_add(entry.descriptor.len);
            address < other_end && entry.descriptor.address < end
        }) {
            return Err(DmaLeaseError::Overlap);
        }
        let Some(slot) = self
            .entries
            .iter()
            .enumerate()
            .position(|(slot, entry)| entry.is_none() && self.generations[slot] < u32::MAX)
        else {
            return Err(if self.entries.iter().any(Option::is_none) {
                DmaLeaseError::GenerationExhausted
            } else {
                DmaLeaseError::Full
            });
        };
        let lease_slot = u16::try_from(slot).map_err(|_| DmaLeaseError::Full)?;
        let generation = self.generations[slot] + 1;
        self.generations[slot] = generation;
        self.entries[slot] = Some(DmaLeaseEntry {
            generation,
            owner,
            descriptor: DmaBufferDescriptor {
                address,
                len,
                direction: request.direction,
                coherency: request.coherency,
                peripheral: request.peripheral,
                channel: request.channel,
            },
            state: DmaLeaseState::Reserved,
            #[cfg(any(feature = "pmsa-v7", feature = "pmsa-v8", test))]
            isolation,
        });
        Ok(DmaLease {
            slot: lease_slot,
            generation,
            owner,
        })
    }

    pub fn descriptor<E>(
        &self,
        lease: DmaLease,
        owner: DmaOwnerId,
    ) -> Result<DmaBufferDescriptor, DmaLeaseError<E>> {
        Ok(self.entry(lease, owner)?.descriptor)
    }

    pub fn begin<B: DmaLeaseBackend>(
        &mut self,
        lease: DmaLease,
        owner: DmaOwnerId,
        backend: &mut B,
    ) -> Result<DmaBufferDescriptor, DmaLeaseError<B::Error>> {
        let entry = self.entry_mut(lease, owner)?;
        if entry.state == DmaLeaseState::Active {
            return Err(DmaLeaseError::AlreadyActive);
        }
        backend
            .prepare(entry.descriptor)
            .map_err(DmaLeaseError::Backend)?;
        entry.state = DmaLeaseState::Active;
        Ok(entry.descriptor)
    }

    pub fn complete<B: DmaLeaseBackend>(
        &mut self,
        lease: DmaLease,
        owner: DmaOwnerId,
        transferred: usize,
        backend: &mut B,
    ) -> Result<DmaCompletionReceipt, DmaLeaseError<B::Error>> {
        let entry = *self.entry(lease, owner)?;
        if entry.state != DmaLeaseState::Active {
            return Err(DmaLeaseError::NotActive);
        }
        if transferred > entry.descriptor.len {
            return Err(DmaLeaseError::TransferTooLong);
        }
        backend
            .complete(entry.descriptor, transferred)
            .map_err(DmaLeaseError::Backend)?;
        self.retire(lease, owner)?;
        Ok(DmaCompletionReceipt {
            owner,
            transferred,
            requested: entry.descriptor.len,
            partial: transferred != entry.descriptor.len,
        })
    }

    pub fn cancel<B: DmaLeaseBackend>(
        &mut self,
        lease: DmaLease,
        owner: DmaOwnerId,
        backend: &mut B,
    ) -> Result<(), DmaLeaseError<B::Error>> {
        let entry = *self.entry_base(lease, owner)?;
        if entry.state == DmaLeaseState::Active {
            backend
                .cancel(entry.descriptor)
                .map_err(DmaLeaseError::Backend)?;
        }
        self.retire(lease, owner)
    }

    pub fn recover<B: DmaLeaseBackend>(
        &mut self,
        lease: DmaLease,
        owner: DmaOwnerId,
        reason: DmaRecoveryReason,
        backend: &mut B,
    ) -> Result<DmaRecoveryReceipt, DmaLeaseError<B::Error>> {
        let entry = *self.entry_base(lease, owner)?;
        let cancel_confirmed =
            entry.state != DmaLeaseState::Active || backend.cancel(entry.descriptor).is_ok();
        backend
            .reset(entry.descriptor.peripheral, entry.descriptor.channel)
            .map_err(DmaLeaseError::Backend)?;
        self.retire(lease, owner)?;
        Ok(DmaRecoveryReceipt {
            owner,
            reason,
            cancel_confirmed,
            peripheral_reset: true,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn entry<E>(
        &self,
        lease: DmaLease,
        owner: DmaOwnerId,
    ) -> Result<&DmaLeaseEntry, DmaLeaseError<E>> {
        let entry = self.entry_base(lease, owner)?;
        #[cfg(any(feature = "pmsa-v7", feature = "pmsa-v8", test))]
        {
            if entry
                .isolation
                .is_some_and(|receipt| receipt.ensure_usable().is_err())
            {
                return Err(DmaLeaseError::IsolationStale);
            }
        }
        Ok(entry)
    }

    fn entry_base<E>(
        &self,
        lease: DmaLease,
        owner: DmaOwnerId,
    ) -> Result<&DmaLeaseEntry, DmaLeaseError<E>> {
        let entry = self
            .entries
            .get(usize::from(lease.slot))
            .and_then(Option::as_ref)
            .ok_or(DmaLeaseError::InvalidHandle)?;
        if entry.generation != lease.generation {
            return Err(DmaLeaseError::InvalidHandle);
        }
        if entry.owner != owner || lease.owner != owner {
            return Err(DmaLeaseError::WrongOwner);
        }
        Ok(entry)
    }

    fn entry_mut<E>(
        &mut self,
        lease: DmaLease,
        owner: DmaOwnerId,
    ) -> Result<&mut DmaLeaseEntry, DmaLeaseError<E>> {
        self.entry(lease, owner)?;
        let entry = self
            .entries
            .get_mut(usize::from(lease.slot))
            .and_then(Option::as_mut)
            .ok_or(DmaLeaseError::InvalidHandle)?;
        if entry.generation != lease.generation {
            return Err(DmaLeaseError::InvalidHandle);
        }
        if entry.owner != owner || lease.owner != owner {
            return Err(DmaLeaseError::WrongOwner);
        }
        Ok(entry)
    }

    fn retire<E>(&mut self, lease: DmaLease, owner: DmaOwnerId) -> Result<(), DmaLeaseError<E>> {
        self.entry_base(lease, owner)?;
        self.entries[usize::from(lease.slot)] = None;
        Ok(())
    }
}

impl<const N: usize> Default for DmaLeaseRegistry<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Backend {
        prepared: u8,
        completed: u8,
        cancelled: u8,
        reset: u8,
        fail_cancel: bool,
        fail_reset: bool,
    }

    impl DmaLeaseBackend for Backend {
        type Error = ();

        fn prepare(&mut self, _: DmaBufferDescriptor) -> Result<(), Self::Error> {
            self.prepared += 1;
            Ok(())
        }
        fn complete(&mut self, _: DmaBufferDescriptor, _: usize) -> Result<(), Self::Error> {
            self.completed += 1;
            Ok(())
        }
        fn cancel(&mut self, _: DmaBufferDescriptor) -> Result<(), Self::Error> {
            self.cancelled += 1;
            if self.fail_cancel {
                Err(())
            } else {
                Ok(())
            }
        }
        fn reset(&mut self, _: u16, _: u16) -> Result<(), Self::Error> {
            self.reset += 1;
            if self.fail_reset {
                Err(())
            } else {
                Ok(())
            }
        }
    }

    fn request() -> DmaLeaseRequest {
        DmaLeaseRequest {
            alignment: 1,
            direction: DmaDirection::PeripheralToMemory,
            coherency: DmaCoherency::SoftwareManaged,
            peripheral: 2,
            channel: 3,
        }
    }

    fn acquire<const N: usize>(
        registry: &mut DmaLeaseRegistry<N>,
        owner: DmaOwnerId,
        buffer: &mut [u8],
    ) -> DmaLease {
        // SAFETY: each test keeps the backing array alive and exclusively owned
        // until the lease is completed or cancelled.
        unsafe {
            registry
                .acquire_region(owner, buffer.as_mut_ptr() as usize, buffer.len(), request())
                .unwrap()
        }
    }

    #[test]
    fn generation_and_owner_checks_reject_stale_or_foreign_access() {
        let owner = DmaOwnerId(7);
        let mut registry = DmaLeaseRegistry::<1>::new();
        let mut first_buffer = [0u8; 16];
        let first = acquire(&mut registry, owner, &mut first_buffer);
        assert_eq!(
            registry.descriptor::<()>(first, DmaOwnerId(8)),
            Err(DmaLeaseError::WrongOwner)
        );
        let mut backend = Backend::default();
        registry.cancel(first, owner, &mut backend).unwrap();
        let mut second_buffer = [0u8; 16];
        let second = acquire(&mut registry, owner, &mut second_buffer);
        assert!(second.generation() > first.generation());
        assert_eq!(
            registry.descriptor::<()>(first, owner),
            Err(DmaLeaseError::InvalidHandle)
        );
    }

    #[test]
    fn partial_completion_runs_prepare_and_completion_coherency_hooks() {
        let owner = DmaOwnerId(1);
        let mut registry = DmaLeaseRegistry::<1>::new();
        let mut buffer = [0u8; 32];
        let lease = acquire(&mut registry, owner, &mut buffer);
        let mut backend = Backend::default();
        registry.begin(lease, owner, &mut backend).unwrap();
        let receipt = registry.complete(lease, owner, 12, &mut backend).unwrap();
        assert_eq!(backend.prepared, 1);
        assert_eq!(backend.completed, 1);
        assert!(receipt.partial);
        assert_eq!(receipt.transferred, 12);
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn timeout_cancels_resets_and_reclaims_capacity() {
        let owner = DmaOwnerId(2);
        let mut registry = DmaLeaseRegistry::<1>::new();
        let mut buffer = [0u8; 8];
        let lease = acquire(&mut registry, owner, &mut buffer);
        let mut backend = Backend::default();
        registry.begin(lease, owner, &mut backend).unwrap();
        let receipt = registry
            .recover(lease, owner, DmaRecoveryReason::Timeout, &mut backend)
            .unwrap();
        assert!(receipt.peripheral_reset);
        assert!(receipt.cancel_confirmed);
        assert_eq!(backend.cancelled, 1);
        assert_eq!(backend.reset, 1);
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn failed_cancel_is_attributed_and_successful_reset_still_reclaims() {
        let owner = DmaOwnerId(4);
        let mut registry = DmaLeaseRegistry::<1>::new();
        let mut buffer = [0u8; 8];
        let lease = acquire(&mut registry, owner, &mut buffer);
        let mut backend = Backend {
            fail_cancel: true,
            ..Backend::default()
        };
        registry.begin(lease, owner, &mut backend).unwrap();
        let receipt = registry
            .recover(
                lease,
                owner,
                DmaRecoveryReason::PeripheralFault,
                &mut backend,
            )
            .unwrap();
        assert!(!receipt.cancel_confirmed);
        assert!(receipt.peripheral_reset);
        assert_eq!((backend.cancelled, backend.reset), (1, 1));
        assert!(registry.is_empty());
    }

    #[test]
    fn failed_reset_keeps_the_lease_live_for_bounded_retry() {
        let owner = DmaOwnerId(5);
        let mut registry = DmaLeaseRegistry::<1>::new();
        let mut buffer = [0u8; 8];
        let lease = acquire(&mut registry, owner, &mut buffer);
        let mut backend = Backend {
            fail_reset: true,
            ..Backend::default()
        };
        registry.begin(lease, owner, &mut backend).unwrap();
        assert_eq!(
            registry.recover(lease, owner, DmaRecoveryReason::OwnerShutdown, &mut backend,),
            Err(DmaLeaseError::Backend(()))
        );
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.descriptor::<()>(lease, owner).unwrap().len, 8);

        backend.fail_reset = false;
        registry
            .recover(lease, owner, DmaRecoveryReason::OwnerShutdown, &mut backend)
            .unwrap();
        assert!(registry.is_empty());
    }

    #[test]
    fn isolated_dma_rejects_post_fault_completion_but_allows_recovery() {
        use crate::{
            IsolationArchitecture, IsolationCapabilities, IsolationEpoch, IsolationPlan,
            IsolationRegion,
        };
        static EPOCH: IsolationEpoch = IsolationEpoch::new();
        let mut plan = IsolationPlan::<4>::new(41, 41);
        plan.add(IsolationRegion::code(0, 1024 * 1024)).unwrap();
        plan.add(IsolationRegion::data(0x2000_0000, 256)).unwrap();
        plan.add(IsolationRegion::stack(0x2000_0100, 256)).unwrap();
        plan.add(IsolationRegion::peripheral(0x4000_0000, 4096, 2))
            .unwrap();
        let capabilities = IsolationCapabilities::pmsa(1, 1, IsolationArchitecture::PmsaV7M, 8);
        let receipt = EPOCH.admit(&plan, capabilities).unwrap();
        let mut buffer = [0u8; 32];
        let mut registry = DmaLeaseRegistry::<1>::new();
        let owner = DmaOwnerId(receipt.lease_owner());
        // SAFETY: the stack buffer outlives the lease and recovery retires the
        // registry entry before the buffer leaves this test scope.
        let lease = unsafe {
            registry.acquire_region_with_isolation(
                owner,
                buffer.as_mut_ptr() as usize,
                buffer.len(),
                request(),
                Some(receipt),
            )
        }
        .unwrap();
        let mut backend = Backend::default();
        EPOCH.activate(receipt).unwrap();
        registry.begin(lease, owner, &mut backend).unwrap();
        EPOCH.fault(receipt).unwrap();
        assert_eq!(
            registry.complete(lease, owner, 32, &mut backend),
            Err(DmaLeaseError::IsolationStale)
        );
        let recovery = registry
            .recover(lease, owner, DmaRecoveryReason::OwnerShutdown, &mut backend)
            .unwrap();
        assert!(recovery.cancel_confirmed);
        assert!(registry.is_empty());
    }

    #[test]
    fn invalid_alignment_and_overlap_fail_before_capacity_changes() {
        let mut registry = DmaLeaseRegistry::<2>::new();
        let owner = DmaOwnerId(3);
        assert_eq!(
            unsafe { registry.acquire_region(owner, 3, 8, request()) },
            Ok(DmaLease {
                slot: 0,
                generation: 1,
                owner,
            })
        );
        assert_eq!(
            unsafe { registry.acquire_region(owner, 4, 2, request()) },
            Err(DmaLeaseError::Overlap)
        );
        let mut aligned = request();
        aligned.alignment = 8;
        assert_eq!(
            unsafe { registry.acquire_region(owner, 18, 8, aligned) },
            Err(DmaLeaseError::InvalidAlignment)
        );
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn exhausted_generation_fails_closed_without_reissuing_a_stale_lease() {
        let mut registry = DmaLeaseRegistry::<1>::new();
        registry.generations[0] = u32::MAX;
        let mut buffer = [0u8; 8];
        assert_eq!(
            unsafe {
                registry.acquire_region(
                    DmaOwnerId(1),
                    buffer.as_mut_ptr() as usize,
                    buffer.len(),
                    request(),
                )
            },
            Err(DmaLeaseError::GenerationExhausted)
        );
        assert!(registry.is_empty());
    }
}
