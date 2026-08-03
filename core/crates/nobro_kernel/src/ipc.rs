//! Typed, allocation-free IPC for payloads larger than the nano two-word message.
//!
//! Each payload lives in one generation-tagged fixed-pool slot. Destination
//! queues store only typed handles, so multicast shares one payload instead of
//! copying it. Receivers borrow through a callback and the router releases the
//! reference deterministically when the callback returns. The compact
//! [`crate::Message`] mailbox remains the nano control path.

use core::{marker::PhantomData, mem::MaybeUninit};

use crate::ModuleId;

#[derive(Debug, PartialEq, Eq)]
pub struct PayloadHandle<T> {
    slot: u16,
    epoch: u16,
    generation: u32,
    marker: PhantomData<fn() -> T>,
}

impl<T> Copy for PayloadHandle<T> {}

impl<T> Clone for PayloadHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> PayloadHandle<T> {
    pub const fn slot(self) -> u16 {
        self.slot
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }

    pub const fn epoch(self) -> u16 {
        self.epoch
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadPoolError {
    Full,
    InvalidHandle,
    WrongOwner,
    SharedPayload,
    RefcountOverflow,
    IdentityExhausted,
    Expired,
    InvalidExpiry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PayloadOwnership {
    Vacant,
    Exclusive(ModuleId),
    Shared,
}

struct PayloadSlot<T> {
    value: MaybeUninit<T>,
    epoch: u16,
    generation: u32,
    ownership: PayloadOwnership,
    references: u16,
    source: ModuleId,
    expires_at_us: u64,
}

impl<T> PayloadSlot<T> {
    const fn empty() -> Self {
        Self {
            value: MaybeUninit::uninit(),
            epoch: 0,
            generation: 0,
            ownership: PayloadOwnership::Vacant,
            references: 0,
            source: ModuleId::Kernel,
            expires_at_us: 0,
        }
    }

    const fn occupied(&self) -> bool {
        !matches!(self.ownership, PayloadOwnership::Vacant)
    }

    const fn reusable(&self) -> bool {
        self.generation < u32::MAX || self.epoch < u16::MAX
    }

    const fn next_identity(&self) -> Option<(u16, u32)> {
        if self.generation < u32::MAX {
            Some((self.epoch, self.generation + 1))
        } else if self.epoch < u16::MAX {
            Some((self.epoch + 1, 1))
        } else {
            None
        }
    }
}

/// Typed fixed-capacity payload storage. Handles never expose a raw pointer.
pub struct PayloadPool<T, const N: usize> {
    slots: [PayloadSlot<T>; N],
    len: usize,
}

impl<T, const N: usize> PayloadPool<T, N> {
    pub const fn new() -> Self {
        Self {
            slots: [const { PayloadSlot::empty() }; N],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub fn allocate(
        &mut self,
        owner: ModuleId,
        value: T,
        expires_at_us: u64,
        now_us: u64,
    ) -> Result<PayloadHandle<T>, PayloadPoolError> {
        if expires_at_us <= now_us {
            return Err(PayloadPoolError::InvalidExpiry);
        }
        let Some((index, slot)) = self
            .slots
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| !slot.occupied() && slot.reusable())
        else {
            return Err(if self.slots.iter().any(|slot| !slot.occupied()) {
                PayloadPoolError::IdentityExhausted
            } else {
                PayloadPoolError::Full
            });
        };
        let slot_index = u16::try_from(index).map_err(|_| PayloadPoolError::RefcountOverflow)?;
        let (epoch, generation) = slot
            .next_identity()
            .ok_or(PayloadPoolError::IdentityExhausted)?;
        slot.value.write(value);
        slot.epoch = epoch;
        slot.generation = generation;
        slot.ownership = PayloadOwnership::Exclusive(owner);
        slot.references = 1;
        slot.source = owner;
        slot.expires_at_us = expires_at_us;
        self.len += 1;
        Ok(PayloadHandle {
            slot: slot_index,
            epoch,
            generation,
            marker: PhantomData,
        })
    }

    pub fn transfer(
        &mut self,
        handle: PayloadHandle<T>,
        from: ModuleId,
        to: ModuleId,
        now_us: u64,
    ) -> Result<(), PayloadPoolError> {
        let slot = self.live_slot_mut(handle, now_us)?;
        match slot.ownership {
            PayloadOwnership::Exclusive(owner) if owner == from => {
                slot.ownership = PayloadOwnership::Exclusive(to);
                Ok(())
            }
            PayloadOwnership::Exclusive(_) => Err(PayloadPoolError::WrongOwner),
            PayloadOwnership::Shared => Err(PayloadPoolError::SharedPayload),
            PayloadOwnership::Vacant => Err(PayloadPoolError::InvalidHandle),
        }
    }

    pub fn borrow(
        &self,
        handle: PayloadHandle<T>,
        owner: ModuleId,
        now_us: u64,
    ) -> Result<&T, PayloadPoolError> {
        let slot = self.live_slot(handle, now_us)?;
        match slot.ownership {
            PayloadOwnership::Exclusive(actual) if actual == owner => {
                // SAFETY: occupied slots are initialized exactly once and are
                // dropped only while holding an exclusive pool borrow.
                Ok(unsafe { slot.value.assume_init_ref() })
            }
            PayloadOwnership::Exclusive(_) => Err(PayloadPoolError::WrongOwner),
            PayloadOwnership::Shared => Err(PayloadPoolError::SharedPayload),
            PayloadOwnership::Vacant => Err(PayloadPoolError::InvalidHandle),
        }
    }

    pub fn release(
        &mut self,
        handle: PayloadHandle<T>,
        owner: ModuleId,
    ) -> Result<(), PayloadPoolError> {
        let slot = self.slot_mut(handle)?;
        match slot.ownership {
            PayloadOwnership::Exclusive(actual) if actual == owner => self.retire(handle),
            PayloadOwnership::Exclusive(_) => Err(PayloadPoolError::WrongOwner),
            PayloadOwnership::Shared => Err(PayloadPoolError::SharedPayload),
            PayloadOwnership::Vacant => Err(PayloadPoolError::InvalidHandle),
        }
    }

    fn share(
        &mut self,
        handle: PayloadHandle<T>,
        owner: ModuleId,
        references: usize,
        now_us: u64,
    ) -> Result<(), PayloadPoolError> {
        let references =
            u16::try_from(references).map_err(|_| PayloadPoolError::RefcountOverflow)?;
        if references == 0 {
            return Err(PayloadPoolError::RefcountOverflow);
        }
        let slot = self.live_slot_mut(handle, now_us)?;
        match slot.ownership {
            PayloadOwnership::Exclusive(actual) if actual == owner => {
                slot.ownership = PayloadOwnership::Shared;
                slot.references = references;
                Ok(())
            }
            PayloadOwnership::Exclusive(_) => Err(PayloadPoolError::WrongOwner),
            PayloadOwnership::Shared => Err(PayloadPoolError::SharedPayload),
            PayloadOwnership::Vacant => Err(PayloadPoolError::InvalidHandle),
        }
    }

    fn borrow_shared(&self, handle: PayloadHandle<T>, now_us: u64) -> Result<&T, PayloadPoolError> {
        let slot = self.live_slot(handle, now_us)?;
        if slot.ownership != PayloadOwnership::Shared || slot.references == 0 {
            return Err(PayloadPoolError::InvalidHandle);
        }
        // SAFETY: same initialized-slot invariant as `borrow`.
        Ok(unsafe { slot.value.assume_init_ref() })
    }

    fn release_shared(&mut self, handle: PayloadHandle<T>) -> Result<(), PayloadPoolError> {
        let slot = self.slot_mut(handle)?;
        if slot.ownership != PayloadOwnership::Shared || slot.references == 0 {
            return Err(PayloadPoolError::InvalidHandle);
        }
        slot.references -= 1;
        if slot.references == 0 {
            self.retire(handle)?;
        }
        Ok(())
    }

    fn live_slot(
        &self,
        handle: PayloadHandle<T>,
        now_us: u64,
    ) -> Result<&PayloadSlot<T>, PayloadPoolError> {
        let slot = self.slot(handle)?;
        if now_us >= slot.expires_at_us {
            return Err(PayloadPoolError::Expired);
        }
        Ok(slot)
    }

    fn live_slot_mut(
        &mut self,
        handle: PayloadHandle<T>,
        now_us: u64,
    ) -> Result<&mut PayloadSlot<T>, PayloadPoolError> {
        let slot = self.slot_mut(handle)?;
        if now_us >= slot.expires_at_us {
            return Err(PayloadPoolError::Expired);
        }
        Ok(slot)
    }

    fn slot(&self, handle: PayloadHandle<T>) -> Result<&PayloadSlot<T>, PayloadPoolError> {
        let slot = self
            .slots
            .get(usize::from(handle.slot))
            .ok_or(PayloadPoolError::InvalidHandle)?;
        if !slot.occupied() || slot.epoch != handle.epoch || slot.generation != handle.generation {
            return Err(PayloadPoolError::InvalidHandle);
        }
        Ok(slot)
    }

    fn slot_mut(
        &mut self,
        handle: PayloadHandle<T>,
    ) -> Result<&mut PayloadSlot<T>, PayloadPoolError> {
        let slot = self
            .slots
            .get_mut(usize::from(handle.slot))
            .ok_or(PayloadPoolError::InvalidHandle)?;
        if !slot.occupied() || slot.epoch != handle.epoch || slot.generation != handle.generation {
            return Err(PayloadPoolError::InvalidHandle);
        }
        Ok(slot)
    }

    fn retire(&mut self, handle: PayloadHandle<T>) -> Result<(), PayloadPoolError> {
        let slot = self.slot_mut(handle)?;
        // SAFETY: the slot is occupied and initialized; ownership is cleared
        // immediately after this one drop.
        unsafe { slot.value.assume_init_drop() };
        slot.ownership = PayloadOwnership::Vacant;
        slot.references = 0;
        slot.expires_at_us = 0;
        self.len -= 1;
        Ok(())
    }

    fn expires_at(&self, handle: PayloadHandle<T>) -> Result<u64, PayloadPoolError> {
        Ok(self.slot(handle)?.expires_at_us)
    }
}

impl<T, const N: usize> Default for PayloadPool<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> Drop for PayloadPool<T, N> {
    fn drop(&mut self) {
        for slot in &mut self.slots {
            if slot.occupied() {
                // SAFETY: every occupied slot owns one initialized value.
                unsafe { slot.value.assume_init_drop() };
                slot.ownership = PayloadOwnership::Vacant;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum IpcPriority {
    #[default]
    Normal = 0,
    Urgent = 1,
    Control = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcPushError {
    NoDestination,
    DestinationRegistryFull,
    DuplicateDestination(ModuleId),
    DestinationUnknown(ModuleId),
    DestinationQuota(ModuleId),
    ControlReserve,
    GlobalCapacity,
    PayloadPoolFull,
    PayloadIdentityExhausted,
    InvalidExpiry,
    FanoutTooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcReceiveError {
    DestinationUnknown(ModuleId),
    Payload(PayloadPoolError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpcPublishReceipt<T> {
    pub handle: PayloadHandle<T>,
    pub destinations: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IpcSnapshot {
    pub payloads: usize,
    pub queued: usize,
    pub global_capacity: usize,
    pub control_reserve: usize,
    pub rejected_global: u32,
    pub rejected_reserve: u32,
    pub rejected_destination: u32,
    pub expired: u32,
    pub cancelled: u32,
}

#[derive(Clone, Copy)]
struct IpcEnvelope<T> {
    source: ModuleId,
    destination: ModuleId,
    priority: IpcPriority,
    handle: PayloadHandle<T>,
}

struct DestinationQueue<T, const Q: usize> {
    destination: Option<ModuleId>,
    quota: usize,
    entries: [Option<IpcEnvelope<T>>; Q],
    len: usize,
}

impl<T, const Q: usize> DestinationQueue<T, Q> {
    const fn empty() -> Self {
        Self {
            destination: None,
            quota: 0,
            entries: [const { None }; Q],
            len: 0,
        }
    }

    fn push(&mut self, envelope: IpcEnvelope<T>) {
        self.entries[self.len] = Some(envelope);
        self.len += 1;
    }

    fn remove_at(&mut self, index: usize) -> IpcEnvelope<T> {
        let removed = self.entries[index].take().unwrap();
        for cursor in index..self.len - 1 {
            self.entries[cursor] = self.entries[cursor + 1].take();
        }
        self.entries[self.len - 1] = None;
        self.len -= 1;
        removed
    }

    fn pop_best(&mut self) -> Option<IpcEnvelope<T>> {
        let mut best = 0usize;
        for index in 1..self.len {
            let candidate = self.entries[index].as_ref().unwrap();
            let selected = self.entries[best].as_ref().unwrap();
            if candidate.priority > selected.priority {
                best = index;
            }
        }
        (self.len != 0).then(|| self.remove_at(best))
    }
}

/// Fixed-pool payloads plus independent bounded destination queues.
pub struct IpcRouter<
    T,
    const PAYLOADS: usize,
    const DESTINATIONS: usize,
    const QUEUE: usize,
    const GLOBAL: usize,
> {
    pool: PayloadPool<T, PAYLOADS>,
    queues: [DestinationQueue<T, QUEUE>; DESTINATIONS],
    queued: usize,
    control_reserve: usize,
    rejected_global: u32,
    rejected_reserve: u32,
    rejected_destination: u32,
    expired: u32,
    cancelled: u32,
}

impl<
        T,
        const PAYLOADS: usize,
        const DESTINATIONS: usize,
        const QUEUE: usize,
        const GLOBAL: usize,
    > IpcRouter<T, PAYLOADS, DESTINATIONS, QUEUE, GLOBAL>
{
    pub const fn new(control_reserve: usize) -> Self {
        Self {
            pool: PayloadPool::new(),
            queues: [const { DestinationQueue::empty() }; DESTINATIONS],
            queued: 0,
            control_reserve: if control_reserve > GLOBAL {
                GLOBAL
            } else {
                control_reserve
            },
            rejected_global: 0,
            rejected_reserve: 0,
            rejected_destination: 0,
            expired: 0,
            cancelled: 0,
        }
    }

    pub fn register_destination(
        &mut self,
        destination: ModuleId,
        quota: usize,
    ) -> Result<(), IpcPushError> {
        if quota == 0 || quota > QUEUE {
            return Err(IpcPushError::DestinationQuota(destination));
        }
        if self.queue_index(destination).is_some() {
            return Err(IpcPushError::DuplicateDestination(destination));
        }
        let Some(queue) = self
            .queues
            .iter_mut()
            .find(|queue| queue.destination.is_none())
        else {
            return Err(IpcPushError::DestinationRegistryFull);
        };
        queue.destination = Some(destination);
        queue.quota = quota;
        Ok(())
    }

    pub fn publish(
        &mut self,
        source: ModuleId,
        destinations: &[ModuleId],
        priority: IpcPriority,
        expires_at_us: u64,
        now_us: u64,
        value: T,
    ) -> Result<IpcPublishReceipt<T>, IpcPushError> {
        if destinations.is_empty() {
            return Err(IpcPushError::NoDestination);
        }
        if destinations.len() > usize::from(u16::MAX) {
            return Err(IpcPushError::FanoutTooLarge);
        }
        if expires_at_us <= now_us {
            return Err(IpcPushError::InvalidExpiry);
        }
        for (index, destination) in destinations.iter().enumerate() {
            if destinations[..index].contains(destination) {
                return Err(IpcPushError::DuplicateDestination(*destination));
            }
            let Some(queue_index) = self.queue_index(*destination) else {
                return Err(IpcPushError::DestinationUnknown(*destination));
            };
            let queue = &self.queues[queue_index];
            if queue.len >= queue.quota {
                self.rejected_destination = self.rejected_destination.saturating_add(1);
                return Err(IpcPushError::DestinationQuota(*destination));
            }
        }
        let Some(next_queued) = self.queued.checked_add(destinations.len()) else {
            self.rejected_global = self.rejected_global.saturating_add(1);
            return Err(IpcPushError::GlobalCapacity);
        };
        if next_queued > GLOBAL {
            self.rejected_global = self.rejected_global.saturating_add(1);
            return Err(IpcPushError::GlobalCapacity);
        }
        if priority != IpcPriority::Control
            && next_queued > GLOBAL.saturating_sub(self.control_reserve)
        {
            self.rejected_reserve = self.rejected_reserve.saturating_add(1);
            return Err(IpcPushError::ControlReserve);
        }
        let handle = self
            .pool
            .allocate(source, value, expires_at_us, now_us)
            .map_err(|error| match error {
                PayloadPoolError::Full => IpcPushError::PayloadPoolFull,
                PayloadPoolError::IdentityExhausted => IpcPushError::PayloadIdentityExhausted,
                _ => IpcPushError::InvalidExpiry,
            })?;
        if self
            .pool
            .share(handle, source, destinations.len(), now_us)
            .is_err()
        {
            let _ = self.pool.release(handle, source);
            return Err(IpcPushError::FanoutTooLarge);
        }
        for destination in destinations {
            let queue_index = self.queue_index(*destination).unwrap();
            self.queues[queue_index].push(IpcEnvelope {
                source,
                destination: *destination,
                priority,
                handle,
            });
        }
        self.queued = next_queued;
        Ok(IpcPublishReceipt {
            handle,
            destinations: destinations.len() as u16,
        })
    }

    /// Borrow one highest-priority payload for `destination`. The reference is
    /// valid only during `visit`; its shared pool reference is released after
    /// the callback returns.
    pub fn receive_with<R>(
        &mut self,
        destination: ModuleId,
        now_us: u64,
        visit: impl FnOnce(ModuleId, IpcPriority, &T) -> R,
    ) -> Result<Option<R>, IpcReceiveError> {
        let Some(queue_index) = self.queue_index(destination) else {
            return Err(IpcReceiveError::DestinationUnknown(destination));
        };
        self.expire_queue(queue_index, now_us);
        let Some(envelope) = self.queues[queue_index].pop_best() else {
            return Ok(None);
        };
        self.queued -= 1;
        debug_assert_eq!(envelope.destination, destination);
        let result = {
            let payload = match self.pool.borrow_shared(envelope.handle, now_us) {
                Ok(payload) => payload,
                Err(error) => {
                    // The envelope has already left its queue. Release its
                    // reference even if a corrupted/stale handle reaches this
                    // boundary, so one bad delivery cannot leak pool capacity.
                    let _ = self.pool.release_shared(envelope.handle);
                    return Err(IpcReceiveError::Payload(error));
                }
            };
            visit(envelope.source, envelope.priority, payload)
        };
        self.pool
            .release_shared(envelope.handle)
            .map_err(IpcReceiveError::Payload)?;
        Ok(Some(result))
    }

    pub fn expire(&mut self, now_us: u64) -> usize {
        let before = self.queued;
        for index in 0..DESTINATIONS {
            self.expire_queue(index, now_us);
        }
        before - self.queued
    }

    pub fn cancel_for(&mut self, module: ModuleId) -> usize {
        let mut removed = 0usize;
        for queue in &mut self.queues {
            let mut index = 0;
            while index < queue.len {
                let envelope = queue.entries[index].as_ref().unwrap();
                if envelope.source == module || envelope.destination == module {
                    let envelope = queue.remove_at(index);
                    let _ = self.pool.release_shared(envelope.handle);
                    self.queued -= 1;
                    removed += 1;
                } else {
                    index += 1;
                }
            }
        }
        self.cancelled = self
            .cancelled
            .saturating_add(u32::try_from(removed).unwrap_or(u32::MAX));
        removed
    }

    pub const fn snapshot(&self) -> IpcSnapshot {
        IpcSnapshot {
            payloads: self.pool.len(),
            queued: self.queued,
            global_capacity: GLOBAL,
            control_reserve: self.control_reserve,
            rejected_global: self.rejected_global,
            rejected_reserve: self.rejected_reserve,
            rejected_destination: self.rejected_destination,
            expired: self.expired,
            cancelled: self.cancelled,
        }
    }

    fn queue_index(&self, destination: ModuleId) -> Option<usize> {
        self.queues
            .iter()
            .position(|queue| queue.destination == Some(destination))
    }

    fn expire_queue(&mut self, queue_index: usize, now_us: u64) {
        let queue = &mut self.queues[queue_index];
        let mut index = 0;
        while index < queue.len {
            let handle = queue.entries[index].as_ref().unwrap().handle;
            let expired = match self.pool.expires_at(handle) {
                Ok(deadline) => now_us >= deadline,
                Err(_) => true,
            };
            if expired {
                let envelope = queue.remove_at(index);
                let _ = self.pool.release_shared(envelope.handle);
                self.queued -= 1;
                self.expired = self.expired.saturating_add(1);
            } else {
                index += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn exclusive_payload_transfer_rejects_stale_and_wrong_owner_handles() {
        let mut pool = PayloadPool::<u32, 1>::new();
        let first = pool.allocate(ModuleId::Sensor, 7, 100, 0).unwrap();
        assert_eq!(*pool.borrow(first, ModuleId::Sensor, 0).unwrap(), 7);
        pool.transfer(first, ModuleId::Sensor, ModuleId::Radio, 0)
            .unwrap();
        assert_eq!(
            pool.borrow(first, ModuleId::Sensor, 0),
            Err(PayloadPoolError::WrongOwner)
        );
        pool.release(first, ModuleId::Radio).unwrap();
        let second = pool.allocate(ModuleId::Sensor, 9, 100, 0).unwrap();
        assert_eq!(first.slot(), second.slot());
        assert_ne!(first.generation(), second.generation());
        assert_eq!(
            pool.borrow(first, ModuleId::Sensor, 0),
            Err(PayloadPoolError::InvalidHandle)
        );
    }

    #[test]
    fn payload_identity_rollover_advances_epoch_then_fails_closed() {
        let mut pool = PayloadPool::<u32, 1>::new();
        let stale = pool.allocate(ModuleId::Sensor, 1, 100, 0).unwrap();
        pool.release(stale, ModuleId::Sensor).unwrap();
        pool.slots[0].generation = u32::MAX;
        let fresh = pool.allocate(ModuleId::Sensor, 2, 100, 0).unwrap();
        assert_eq!(fresh.epoch(), stale.epoch() + 1);
        assert_eq!(fresh.generation(), 1);
        assert_eq!(
            pool.borrow(stale, ModuleId::Sensor, 0),
            Err(PayloadPoolError::InvalidHandle)
        );
        pool.release(fresh, ModuleId::Sensor).unwrap();
        pool.slots[0].epoch = u16::MAX;
        pool.slots[0].generation = u32::MAX;
        assert_eq!(
            pool.allocate(ModuleId::Sensor, 3, 100, 0),
            Err(PayloadPoolError::IdentityExhausted)
        );
    }

    #[test]
    fn destination_queues_preserve_fair_capacity_priority_and_control_reserve() {
        let mut router = IpcRouter::<u32, 4, 2, 3, 4>::new(1);
        router.register_destination(ModuleId::Sensor, 2).unwrap();
        router.register_destination(ModuleId::Radio, 2).unwrap();
        router
            .publish(
                ModuleId::Kernel,
                &[ModuleId::Sensor],
                IpcPriority::Normal,
                100,
                0,
                1,
            )
            .unwrap();
        router
            .publish(
                ModuleId::Kernel,
                &[ModuleId::Sensor],
                IpcPriority::Urgent,
                100,
                0,
                2,
            )
            .unwrap();
        assert_eq!(
            router.publish(
                ModuleId::Kernel,
                &[ModuleId::Sensor],
                IpcPriority::Normal,
                100,
                0,
                3,
            ),
            Err(IpcPushError::DestinationQuota(ModuleId::Sensor))
        );
        router
            .publish(
                ModuleId::Kernel,
                &[ModuleId::Radio],
                IpcPriority::Normal,
                100,
                0,
                4,
            )
            .unwrap();
        assert_eq!(
            router.publish(
                ModuleId::Kernel,
                &[ModuleId::Radio],
                IpcPriority::Normal,
                100,
                0,
                5,
            ),
            Err(IpcPushError::ControlReserve)
        );
        router
            .publish(
                ModuleId::Kernel,
                &[ModuleId::Radio],
                IpcPriority::Control,
                100,
                0,
                6,
            )
            .unwrap();
        let first = router
            .receive_with(ModuleId::Sensor, 0, |_, _, value| *value)
            .unwrap();
        assert_eq!(first, Some(2));
        let second = router
            .receive_with(ModuleId::Sensor, 0, |_, _, value| *value)
            .unwrap();
        assert_eq!(second, Some(1));
        let radio = router
            .receive_with(ModuleId::Radio, 0, |_, priority, value| (priority, *value))
            .unwrap();
        assert_eq!(radio, Some((IpcPriority::Control, 6)));
    }

    #[test]
    fn multicast_uses_one_payload_and_expiry_or_cancellation_releases_every_ref() {
        static DROPS: AtomicUsize = AtomicUsize::new(0);
        struct CountDrop;
        impl Drop for CountDrop {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::AcqRel);
            }
        }

        DROPS.store(0, Ordering::Release);
        let mut router = IpcRouter::<CountDrop, 1, 2, 2, 4>::new(0);
        router.register_destination(ModuleId::Sensor, 2).unwrap();
        router.register_destination(ModuleId::Radio, 2).unwrap();
        router
            .publish(
                ModuleId::Kernel,
                &[ModuleId::Sensor, ModuleId::Radio],
                IpcPriority::Normal,
                10,
                0,
                CountDrop,
            )
            .unwrap();
        assert_eq!(router.snapshot().payloads, 1);
        assert_eq!(router.snapshot().queued, 2);
        assert_eq!(router.expire(10), 2);
        assert_eq!(router.snapshot().payloads, 0);
        assert_eq!(DROPS.load(Ordering::Acquire), 1);

        router
            .publish(
                ModuleId::Sensor,
                &[ModuleId::Sensor, ModuleId::Radio],
                IpcPriority::Normal,
                20,
                10,
                CountDrop,
            )
            .unwrap();
        assert_eq!(router.cancel_for(ModuleId::Sensor), 2);
        assert_eq!(router.snapshot().payloads, 0);
        assert_eq!(DROPS.load(Ordering::Acquire), 2);
    }

    #[test]
    fn typed_admission_errors_leave_router_state_unchanged() {
        let mut router = IpcRouter::<u32, 4, 1, 4, 1>::new(0);
        router.register_destination(ModuleId::Sensor, 4).unwrap();
        assert_eq!(
            router.register_destination(ModuleId::Radio, 1),
            Err(IpcPushError::DestinationRegistryFull)
        );
        assert_eq!(
            router.publish(
                ModuleId::Kernel,
                &[ModuleId::Sensor],
                IpcPriority::Normal,
                0,
                0,
                1,
            ),
            Err(IpcPushError::InvalidExpiry)
        );
        router
            .publish(
                ModuleId::Kernel,
                &[ModuleId::Sensor],
                IpcPriority::Normal,
                10,
                0,
                2,
            )
            .unwrap();
        assert_eq!(
            router.publish(
                ModuleId::Kernel,
                &[ModuleId::Sensor],
                IpcPriority::Control,
                10,
                0,
                3,
            ),
            Err(IpcPushError::GlobalCapacity)
        );
        assert_eq!(router.snapshot().queued, 1);
        assert_eq!(router.snapshot().payloads, 1);
        assert_eq!(router.snapshot().rejected_global, 1);
    }

    #[test]
    fn cancelling_one_multicast_destination_retains_the_other_reference() {
        let mut router = IpcRouter::<u32, 1, 2, 2, 2>::new(0);
        router.register_destination(ModuleId::Sensor, 2).unwrap();
        router.register_destination(ModuleId::Radio, 2).unwrap();
        router
            .publish(
                ModuleId::Kernel,
                &[ModuleId::Sensor, ModuleId::Radio],
                IpcPriority::Normal,
                10,
                0,
                77,
            )
            .unwrap();
        assert_eq!(router.cancel_for(ModuleId::Sensor), 1);
        assert_eq!(router.snapshot().payloads, 1);
        assert_eq!(router.snapshot().queued, 1);
        assert_eq!(
            router
                .receive_with(ModuleId::Radio, 0, |_, _, value| *value)
                .unwrap(),
            Some(77)
        );
        assert_eq!(router.snapshot().payloads, 0);
        assert_eq!(router.snapshot().queued, 0);
    }
}
