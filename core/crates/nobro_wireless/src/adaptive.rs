//! Adaptive, bounded message scheduling layered above [`ManagedLink`](crate::ManagedLink).
//!
//! The default storage is a fixed array. Applications may instead lend a slot slice, or
//! explicitly enable the `alloc` feature and reserve a bounded heap queue once. No mode
//! allocates while enqueueing or servicing traffic. Deadlines describe the desired service
//! time; expiry is the hard usefulness limit. This distinction lets best-effort radio work
//! absorb variable link latency without weakening deterministic tasks.

use crate::{LinkError, ManagedLink, TxContract, WirelessBackend};

/// Retry behavior for failures that reached a backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total backend submissions, including the first attempt.
    pub max_attempts: u8,
    pub initial_backoff_us: u64,
    pub max_backoff_us: u64,
}

impl RetryPolicy {
    pub const fn none() -> Self {
        Self {
            max_attempts: 1,
            initial_backoff_us: 0,
            max_backoff_us: 0,
        }
    }

    pub const fn exponential(
        max_attempts: u8,
        initial_backoff_us: u64,
        max_backoff_us: u64,
    ) -> Self {
        Self {
            max_attempts,
            initial_backoff_us,
            max_backoff_us,
        }
    }

    pub const fn is_valid(self) -> bool {
        self.max_attempts != 0
            && self.initial_backoff_us <= self.max_backoff_us
            && (self.max_attempts == 1 || self.initial_backoff_us != 0)
    }

    pub fn delay_after(self, failed_attempts: u8) -> u64 {
        if failed_attempts == 0 || self.initial_backoff_us == 0 {
            return 0;
        }
        let shift = (failed_attempts - 1).min(63) as u32;
        self.initial_backoff_us
            .checked_shl(shift)
            .unwrap_or(u64::MAX)
            .min(self.max_backoff_us)
    }
}

/// Queue-wide policy. It is data, so configuration tools can explain its exact cost/behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdaptivePolicy {
    pub retry: RetryPolicy,
    /// Batchable messages may wait this long to reduce radio wakeups.
    pub batch_window_us: u64,
    /// Maximum messages serviced before the caller should yield to other work.
    pub max_batch_messages: u16,
}

impl AdaptivePolicy {
    pub const fn responsive() -> Self {
        Self {
            retry: RetryPolicy::exponential(3, 1_000, 100_000),
            batch_window_us: 0,
            max_batch_messages: 1,
        }
    }

    pub const fn low_energy(batch_window_us: u64) -> Self {
        Self {
            retry: RetryPolicy::exponential(4, 10_000, 1_000_000),
            batch_window_us,
            max_batch_messages: 8,
        }
    }

    pub const fn is_valid(self) -> bool {
        self.retry.is_valid() && self.max_batch_messages != 0
    }
}

/// Per-message timing and priority. Greater priority values run first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageContract {
    pub offered_at_us: u64,
    pub deadline_us: u64,
    pub expires_at_us: u64,
    pub priority: u8,
    pub batchable: bool,
}

impl MessageContract {
    pub const fn best_effort(offered_at_us: u64, expires_at_us: u64) -> Self {
        Self {
            offered_at_us,
            deadline_us: expires_at_us,
            expires_at_us,
            priority: 0,
            batchable: true,
        }
    }

    pub const fn urgent(offered_at_us: u64, deadline_us: u64) -> Self {
        Self {
            offered_at_us,
            deadline_us,
            expires_at_us: deadline_us,
            priority: u8::MAX,
            batchable: false,
        }
    }

    pub const fn deadline(mut self, deadline_us: u64) -> Self {
        self.deadline_us = deadline_us;
        self
    }

    pub const fn priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    pub const fn batchable(mut self, batchable: bool) -> Self {
        self.batchable = batchable;
        self
    }

    pub const fn is_valid(self) -> bool {
        self.offered_at_us <= self.deadline_us && self.deadline_us <= self.expires_at_us
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageId(u32);

impl MessageId {
    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueError {
    InvalidPolicy,
    InvalidContract,
    PayloadTooLarge,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceEvent {
    Empty,
    IdleUntil(u64),
    Delivered(MessageId),
    Expired(MessageId),
    RetryAt(MessageId, u64),
    RetryExhausted(MessageId),
    Rejected(MessageId, LinkError),
}

/// Scheduler-facing idle decision. This is a hint, not permission for deep system-off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadioPowerHint {
    QueueEmpty,
    StayAwake,
    IdleUntil(u64),
}

/// Monotonic counters separate offered load from useful delivered throughput.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdaptiveDiagnostics {
    pub offered_messages: u32,
    pub offered_bytes: u64,
    pub delivered_messages: u32,
    pub delivered_bytes: u64,
    pub deadline_misses: u32,
    pub expired_messages: u32,
    pub cancelled_messages: u32,
    pub backpressure_rejections: u32,
    pub retry_attempts: u32,
    pub retry_exhaustions: u32,
    pub link_down_deferrals: u32,
    pub window_deferrals: u32,
    pub backend_rejections: u32,
    pub completion_wakes: u32,
    pub radio_wake_batches: u32,
    pub latency_sum_us: u64,
    pub latency_max_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrafficSnapshot {
    pub interval_us: u64,
    pub offered_messages: u32,
    pub delivered_messages: u32,
    pub offered_per_second: u32,
    pub observed_per_second: u32,
}

impl AdaptiveDiagnostics {
    pub fn since(self, earlier: Self, interval_us: u64) -> TrafficSnapshot {
        let offered = self
            .offered_messages
            .saturating_sub(earlier.offered_messages);
        let delivered = self
            .delivered_messages
            .saturating_sub(earlier.delivered_messages);
        TrafficSnapshot {
            interval_us,
            offered_messages: offered,
            delivered_messages: delivered,
            offered_per_second: rate_per_second(offered, interval_us),
            observed_per_second: rate_per_second(delivered, interval_us),
        }
    }
}

fn rate_per_second(count: u32, interval_us: u64) -> u32 {
    if interval_us == 0 {
        return 0;
    }
    ((u64::from(count).saturating_mul(1_000_000) / interval_us).min(u64::from(u32::MAX))) as u32
}

/// Hook called from an interrupt, DMA completion, or vendor callback bridge.
/// Implementations should only make the owning task runnable; heavy work stays in task context.
pub trait CompletionWake {
    fn wake(&self);
}

/// One reusable caller-visible slot. Fields are private so queue invariants cannot be bypassed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MessageSlot<const BYTES: usize> {
    occupied: bool,
    id: MessageId,
    bytes: [u8; BYTES],
    len: u16,
    contract: MessageContract,
    attempts: u8,
    next_attempt_us: u64,
    deadline_recorded: bool,
}

impl<const BYTES: usize> MessageSlot<BYTES> {
    pub const fn empty() -> Self {
        Self {
            occupied: false,
            id: MessageId(0),
            bytes: [0; BYTES],
            len: 0,
            contract: MessageContract::best_effort(0, 0),
            attempts: 0,
            next_attempt_us: 0,
            deadline_recorded: false,
        }
    }

    pub const fn is_occupied(&self) -> bool {
        self.occupied
    }

    fn payload(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

impl<const BYTES: usize> Default for MessageSlot<BYTES> {
    fn default() -> Self {
        Self::empty()
    }
}

/// Storage contract. Custom pools can implement this without changing queue policy.
pub trait AdaptiveStorage<const BYTES: usize> {
    fn slots(&self) -> &[MessageSlot<BYTES>];
    fn slots_mut(&mut self) -> &mut [MessageSlot<BYTES>];
}

pub struct FixedStorage<const SLOTS: usize, const BYTES: usize> {
    slots: [MessageSlot<BYTES>; SLOTS],
}

impl<const SLOTS: usize, const BYTES: usize> FixedStorage<SLOTS, BYTES> {
    pub const fn new() -> Self {
        Self {
            slots: [MessageSlot::empty(); SLOTS],
        }
    }
}

impl<const SLOTS: usize, const BYTES: usize> Default for FixedStorage<SLOTS, BYTES> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const SLOTS: usize, const BYTES: usize> AdaptiveStorage<BYTES> for FixedStorage<SLOTS, BYTES> {
    fn slots(&self) -> &[MessageSlot<BYTES>] {
        &self.slots
    }

    fn slots_mut(&mut self) -> &mut [MessageSlot<BYTES>] {
        &mut self.slots
    }
}

pub struct BorrowedStorage<'a, const BYTES: usize> {
    slots: &'a mut [MessageSlot<BYTES>],
}

impl<'a, const BYTES: usize> BorrowedStorage<'a, BYTES> {
    pub fn new(slots: &'a mut [MessageSlot<BYTES>]) -> Self {
        Self { slots }
    }
}

impl<const BYTES: usize> AdaptiveStorage<BYTES> for BorrowedStorage<'_, BYTES> {
    fn slots(&self) -> &[MessageSlot<BYTES>] {
        self.slots
    }

    fn slots_mut(&mut self) -> &mut [MessageSlot<BYTES>] {
        self.slots
    }
}

#[cfg(feature = "alloc")]
pub struct HeapStorage<const BYTES: usize> {
    slots: alloc::vec::Vec<MessageSlot<BYTES>>,
}

#[cfg(feature = "alloc")]
impl<const BYTES: usize> HeapStorage<BYTES> {
    /// Reserve exactly `slot_count` slots once. Enqueue/service never allocate.
    pub fn with_slots(slot_count: usize) -> Self {
        Self {
            slots: alloc::vec![MessageSlot::empty(); slot_count],
        }
    }

    pub fn reserved_bytes(&self) -> usize {
        self.slots.capacity() * core::mem::size_of::<MessageSlot<BYTES>>()
    }
}

#[cfg(feature = "alloc")]
impl<const BYTES: usize> AdaptiveStorage<BYTES> for HeapStorage<BYTES> {
    fn slots(&self) -> &[MessageSlot<BYTES>] {
        &self.slots
    }

    fn slots_mut(&mut self) -> &mut [MessageSlot<BYTES>] {
        &mut self.slots
    }
}

/// Adaptive queue over a selected storage policy.
pub struct AdaptiveQueue<S, const BYTES: usize> {
    storage: S,
    policy: AdaptivePolicy,
    next_id: u32,
    len: usize,
    diagnostics: AdaptiveDiagnostics,
}

pub type FixedAdaptiveQueue<const SLOTS: usize, const BYTES: usize> =
    AdaptiveQueue<FixedStorage<SLOTS, BYTES>, BYTES>;
pub type BorrowedAdaptiveQueue<'a, const BYTES: usize> =
    AdaptiveQueue<BorrowedStorage<'a, BYTES>, BYTES>;
#[cfg(feature = "alloc")]
pub type HeapAdaptiveQueue<const BYTES: usize> = AdaptiveQueue<HeapStorage<BYTES>, BYTES>;

impl<const SLOTS: usize, const BYTES: usize> AdaptiveQueue<FixedStorage<SLOTS, BYTES>, BYTES> {
    pub fn fixed(policy: AdaptivePolicy) -> Result<Self, QueueError> {
        Self::with_storage(FixedStorage::new(), policy)
    }
}

impl<'a, const BYTES: usize> AdaptiveQueue<BorrowedStorage<'a, BYTES>, BYTES> {
    pub fn borrowed(
        slots: &'a mut [MessageSlot<BYTES>],
        policy: AdaptivePolicy,
    ) -> Result<Self, QueueError> {
        Self::with_storage(BorrowedStorage::new(slots), policy)
    }
}

#[cfg(feature = "alloc")]
impl<const BYTES: usize> AdaptiveQueue<HeapStorage<BYTES>, BYTES> {
    pub fn heap(slot_count: usize, policy: AdaptivePolicy) -> Result<Self, QueueError> {
        Self::with_storage(HeapStorage::with_slots(slot_count), policy)
    }

    pub fn reserved_heap_bytes(&self) -> usize {
        self.storage.reserved_bytes()
    }
}

impl<S: AdaptiveStorage<BYTES>, const BYTES: usize> AdaptiveQueue<S, BYTES> {
    pub fn with_storage(storage: S, policy: AdaptivePolicy) -> Result<Self, QueueError> {
        if !policy.is_valid()
            || storage.slots().is_empty()
            || BYTES == 0
            || BYTES > usize::from(u16::MAX)
        {
            return Err(QueueError::InvalidPolicy);
        }
        Ok(Self {
            storage,
            policy,
            next_id: 1,
            len: 0,
            diagnostics: AdaptiveDiagnostics::default(),
        })
    }

    pub fn enqueue(
        &mut self,
        payload: &[u8],
        contract: MessageContract,
    ) -> Result<MessageId, QueueError> {
        if !contract.is_valid() {
            return Err(QueueError::InvalidContract);
        }
        if payload.len() > BYTES || payload.len() > usize::from(u16::MAX) {
            return Err(QueueError::PayloadTooLarge);
        }
        self.diagnostics.offered_messages = self.diagnostics.offered_messages.saturating_add(1);
        self.diagnostics.offered_bytes = self
            .diagnostics
            .offered_bytes
            .saturating_add(payload.len() as u64);
        let Some(index) = self.storage.slots().iter().position(|slot| !slot.occupied) else {
            self.diagnostics.backpressure_rejections =
                self.diagnostics.backpressure_rejections.saturating_add(1);
            return Err(QueueError::Full);
        };
        let id = self.allocate_id();
        let slot = &mut self.storage.slots_mut()[index];
        slot.bytes[..payload.len()].copy_from_slice(payload);
        slot.len = payload.len() as u16;
        slot.id = id;
        slot.contract = contract;
        slot.attempts = 0;
        slot.next_attempt_us = contract.offered_at_us;
        slot.deadline_recorded = false;
        slot.occupied = true;
        self.len += 1;
        Ok(id)
    }

    /// Queue best-effort data for a relative usefulness window.
    pub fn enqueue_best_effort_for(
        &mut self,
        payload: &[u8],
        now_us: u64,
        useful_for_us: u64,
    ) -> Result<MessageId, QueueError> {
        self.enqueue(
            payload,
            MessageContract::best_effort(now_us, now_us.saturating_add(useful_for_us)),
        )
    }

    /// Queue urgent data with a relative deadline and no batching delay.
    pub fn enqueue_urgent_within(
        &mut self,
        payload: &[u8],
        now_us: u64,
        within_us: u64,
    ) -> Result<MessageId, QueueError> {
        self.enqueue(
            payload,
            MessageContract::urgent(now_us, now_us.saturating_add(within_us)),
        )
    }

    pub fn cancel(&mut self, id: MessageId) -> bool {
        let Some(index) = self
            .storage
            .slots()
            .iter()
            .position(|slot| slot.occupied && slot.id == id)
        else {
            return false;
        };
        self.release(index);
        self.diagnostics.cancelled_messages = self.diagnostics.cancelled_messages.saturating_add(1);
        true
    }

    pub fn service_one<B: WirelessBackend>(
        &mut self,
        now_us: u64,
        link: &mut ManagedLink<B>,
    ) -> ServiceEvent {
        if let Some(index) = self.oldest_expired(now_us) {
            let id = self.storage.slots()[index].id;
            self.record_deadline_miss(index, now_us);
            self.release(index);
            self.diagnostics.expired_messages = self.diagnostics.expired_messages.saturating_add(1);
            return ServiceEvent::Expired(id);
        }
        let Some(index) = self.best_ready(now_us) else {
            return self.next_wake(now_us);
        };
        self.record_deadline_miss(index, now_us);
        let id = self.storage.slots()[index].id;
        let expires_at = self.storage.slots()[index].contract.expires_at_us;
        self.storage.slots_mut()[index].attempts =
            self.storage.slots()[index].attempts.saturating_add(1);
        let result = {
            let slot = &self.storage.slots()[index];
            link.send_at(now_us, TxContract::by(expires_at), slot.payload())
        };
        match result {
            Ok(()) => {
                let slot = &self.storage.slots()[index];
                let bytes = u64::from(slot.len);
                let latency = now_us.saturating_sub(slot.contract.offered_at_us);
                self.diagnostics.delivered_messages =
                    self.diagnostics.delivered_messages.saturating_add(1);
                self.diagnostics.delivered_bytes =
                    self.diagnostics.delivered_bytes.saturating_add(bytes);
                self.diagnostics.latency_sum_us =
                    self.diagnostics.latency_sum_us.saturating_add(latency);
                self.diagnostics.latency_max_us = self.diagnostics.latency_max_us.max(latency);
                self.release(index);
                ServiceEvent::Delivered(id)
            }
            Err(error @ (LinkError::PayloadTooLarge | LinkError::DeadlineElapsed)) => {
                self.release(index);
                ServiceEvent::Rejected(id, error)
            }
            Err(LinkError::WindowExhausted) => {
                self.storage.slots_mut()[index].attempts =
                    self.storage.slots()[index].attempts.saturating_sub(1);
                self.diagnostics.window_deferrals =
                    self.diagnostics.window_deferrals.saturating_add(1);
                let retry_at = self.defer_without_attempt(index, now_us);
                ServiceEvent::RetryAt(id, retry_at)
            }
            Err(LinkError::LinkDown) => {
                self.storage.slots_mut()[index].attempts =
                    self.storage.slots()[index].attempts.saturating_sub(1);
                self.diagnostics.link_down_deferrals =
                    self.diagnostics.link_down_deferrals.saturating_add(1);
                let retry_at = self.defer_without_attempt(index, now_us);
                ServiceEvent::RetryAt(id, retry_at)
            }
            Err(LinkError::BackendRejected) => {
                self.diagnostics.backend_rejections =
                    self.diagnostics.backend_rejections.saturating_add(1);
                self.diagnostics.retry_attempts = self.diagnostics.retry_attempts.saturating_add(1);
                let attempts = self.storage.slots()[index].attempts;
                if attempts >= self.policy.retry.max_attempts {
                    self.release(index);
                    self.diagnostics.retry_exhaustions =
                        self.diagnostics.retry_exhaustions.saturating_add(1);
                    ServiceEvent::RetryExhausted(id)
                } else {
                    let retry_at = now_us.saturating_add(self.policy.retry.delay_after(attempts));
                    self.storage.slots_mut()[index].next_attempt_us = retry_at.min(expires_at);
                    ServiceEvent::RetryAt(id, retry_at.min(expires_at))
                }
            }
        }
    }

    pub fn service_batch<B: WirelessBackend>(
        &mut self,
        now_us: u64,
        link: &mut ManagedLink<B>,
    ) -> u16 {
        let mut delivered = 0;
        let mut serviced = 0;
        while serviced < self.policy.max_batch_messages {
            match self.service_one(now_us, link) {
                ServiceEvent::Delivered(_) => {
                    delivered += 1;
                    serviced += 1;
                }
                ServiceEvent::Expired(_)
                | ServiceEvent::RetryExhausted(_)
                | ServiceEvent::Rejected(_, _) => serviced += 1,
                _ => break,
            }
        }
        if serviced != 0 {
            self.diagnostics.radio_wake_batches =
                self.diagnostics.radio_wake_batches.saturating_add(1);
        }
        delivered
    }

    pub fn notify_completion<W: CompletionWake>(&mut self, wake: &W) {
        self.diagnostics.completion_wakes = self.diagnostics.completion_wakes.saturating_add(1);
        wake.wake();
    }

    pub fn power_hint(&self, now_us: u64) -> RadioPowerHint {
        if self.len == 0 {
            return RadioPowerHint::QueueEmpty;
        }
        match self.next_ready_time() {
            Some(next) if next > now_us => RadioPowerHint::IdleUntil(next),
            _ => RadioPowerHint::StayAwake,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn capacity(&self) -> usize {
        self.storage.slots().len()
    }

    /// Bytes reserved for message slots by the selected storage policy.
    pub fn reserved_storage_bytes(&self) -> usize {
        self.storage
            .slots()
            .len()
            .saturating_mul(core::mem::size_of::<MessageSlot<BYTES>>())
    }

    /// Complete queue value size. Heap storage is reported separately by
    /// `HeapAdaptiveQueue::reserved_heap_bytes` when that feature is enabled.
    pub const fn queue_state_bytes() -> usize {
        core::mem::size_of::<Self>()
    }

    pub const fn diagnostics(&self) -> AdaptiveDiagnostics {
        self.diagnostics
    }

    pub const fn policy(&self) -> AdaptivePolicy {
        self.policy
    }

    pub fn into_storage(self) -> S {
        self.storage
    }

    fn allocate_id(&mut self) -> MessageId {
        loop {
            let id = MessageId(self.next_id.max(1));
            self.next_id = self.next_id.wrapping_add(1).max(1);
            if !self
                .storage
                .slots()
                .iter()
                .any(|slot| slot.occupied && slot.id == id)
            {
                return id;
            }
        }
    }

    fn release(&mut self, index: usize) {
        self.storage.slots_mut()[index] = MessageSlot::empty();
        self.len = self.len.saturating_sub(1);
    }

    fn record_deadline_miss(&mut self, index: usize, now_us: u64) {
        let slot = &mut self.storage.slots_mut()[index];
        if now_us > slot.contract.deadline_us && !slot.deadline_recorded {
            slot.deadline_recorded = true;
            self.diagnostics.deadline_misses = self.diagnostics.deadline_misses.saturating_add(1);
        }
    }

    fn oldest_expired(&self, now_us: u64) -> Option<usize> {
        self.storage
            .slots()
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.occupied && now_us > slot.contract.expires_at_us)
            .min_by_key(|(_, slot)| (slot.contract.expires_at_us, slot.id.0))
            .map(|(index, _)| index)
    }

    fn ready_at(&self, slot: &MessageSlot<BYTES>) -> u64 {
        let batch_at = if slot.contract.batchable {
            slot.contract
                .offered_at_us
                .saturating_add(self.policy.batch_window_us)
                .min(slot.contract.deadline_us)
                .min(slot.contract.expires_at_us)
        } else {
            slot.contract.offered_at_us
        };
        slot.next_attempt_us.max(batch_at)
    }

    fn best_ready(&self, now_us: u64) -> Option<usize> {
        self.storage
            .slots()
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.occupied && self.ready_at(slot) <= now_us)
            .max_by_key(|(_, slot)| {
                (
                    slot.contract.priority,
                    core::cmp::Reverse(slot.contract.deadline_us),
                    core::cmp::Reverse(slot.contract.offered_at_us),
                    core::cmp::Reverse(slot.id.0),
                )
            })
            .map(|(index, _)| index)
    }

    fn next_ready_time(&self) -> Option<u64> {
        self.storage
            .slots()
            .iter()
            .filter(|slot| slot.occupied)
            .map(|slot| self.ready_at(slot).min(slot.contract.expires_at_us))
            .min()
    }

    fn next_wake(&self, now_us: u64) -> ServiceEvent {
        match self.next_ready_time() {
            Some(next) if next > now_us => ServiceEvent::IdleUntil(next),
            Some(_) => ServiceEvent::IdleUntil(now_us),
            None => ServiceEvent::Empty,
        }
    }

    fn defer_without_attempt(&mut self, index: usize, now_us: u64) -> u64 {
        let expires = self.storage.slots()[index].contract.expires_at_us;
        let delay = self.policy.retry.initial_backoff_us.max(1);
        let retry_at = now_us.saturating_add(delay).min(expires);
        self.storage.slots_mut()[index].next_attempt_us = retry_at;
        retry_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{link_catalog, LinkBudget, LinkDescriptor, LinkState, WirelessBackend};
    use core::cell::Cell;

    struct ScriptedRadio {
        up: bool,
        rejects: u8,
        sent: [u8; 32],
        len: usize,
    }

    impl WirelessBackend for ScriptedRadio {
        fn descriptor(&self) -> LinkDescriptor {
            link_catalog::NRF_PROPRIETARY
        }

        fn link_state(&mut self) -> LinkState {
            if self.up {
                LinkState::Up
            } else {
                LinkState::Down
            }
        }

        fn send(&mut self, payload: &[u8]) -> bool {
            if self.rejects != 0 {
                self.rejects -= 1;
                return false;
            }
            self.sent[..payload.len()].copy_from_slice(payload);
            self.len = payload.len();
            true
        }

        fn recv(&mut self, _buf: &mut [u8]) -> usize {
            0
        }

        fn recover(&mut self) -> bool {
            self.up = true;
            true
        }
    }

    fn link(up: bool, rejects: u8) -> ManagedLink<ScriptedRadio> {
        ManagedLink::new(
            ScriptedRadio {
                up,
                rejects,
                sent: [0; 32],
                len: 0,
            },
            LinkBudget::new(32, 16, 256),
        )
    }

    #[test]
    fn burst_backpressure_cancel_and_priority_are_explicit() {
        let mut queue = FixedAdaptiveQueue::<2, 16>::fixed(AdaptivePolicy::responsive()).unwrap();
        let low = queue
            .enqueue(
                b"low",
                MessageContract::best_effort(0, 100)
                    .priority(1)
                    .batchable(false),
            )
            .unwrap();
        let high = queue
            .enqueue(
                b"high",
                MessageContract::best_effort(0, 100)
                    .priority(9)
                    .batchable(false),
            )
            .unwrap();
        assert_eq!(
            queue.enqueue(b"overflow", MessageContract::best_effort(0, 100)),
            Err(QueueError::Full)
        );
        let mut radio = link(true, 0);
        assert_eq!(
            queue.service_one(0, &mut radio),
            ServiceEvent::Delivered(high)
        );
        assert!(queue.cancel(low));
        assert!(!queue.cancel(low));
        assert_eq!(queue.diagnostics().offered_messages, 3);
        assert_eq!(queue.diagnostics().backpressure_rejections, 1);
        assert_eq!(queue.diagnostics().cancelled_messages, 1);
    }

    #[test]
    fn relative_helpers_keep_the_beginner_path_short_and_overflow_safe() {
        let mut queue = FixedAdaptiveQueue::<2, 8>::fixed(AdaptivePolicy::responsive()).unwrap();
        queue.enqueue_best_effort_for(b"data", 10, 90).unwrap();
        queue
            .enqueue_urgent_within(b"alarm", u64::MAX - 5, 20)
            .unwrap();
        assert_eq!(queue.storage.slots()[0].contract.expires_at_us, 100);
        assert_eq!(queue.storage.slots()[1].contract.expires_at_us, u64::MAX);
        assert!(!queue.storage.slots()[1].contract.batchable);
    }

    #[test]
    fn zero_payload_capacity_is_rejected() {
        assert!(matches!(
            FixedAdaptiveQueue::<1, 0>::fixed(AdaptivePolicy::responsive()),
            Err(QueueError::InvalidPolicy)
        ));
    }

    #[test]
    fn batching_exposes_idle_wake_without_deep_sleep_permission() {
        let mut queue = FixedAdaptiveQueue::<2, 16>::fixed(AdaptivePolicy::low_energy(50)).unwrap();
        queue
            .enqueue(b"batched", MessageContract::best_effort(10, 200))
            .unwrap();
        assert_eq!(queue.power_hint(10), RadioPowerHint::IdleUntil(60));
        let mut radio = link(true, 0);
        assert_eq!(
            queue.service_one(10, &mut radio),
            ServiceEvent::IdleUntil(60)
        );
        assert!(matches!(
            queue.service_one(60, &mut radio),
            ServiceEvent::Delivered(_)
        ));
        assert_eq!(queue.power_hint(60), RadioPowerHint::QueueEmpty);
    }

    #[test]
    fn link_delay_does_not_consume_retry_budget_and_expiry_is_hard() {
        let mut queue = FixedAdaptiveQueue::<2, 16>::fixed(AdaptivePolicy::responsive()).unwrap();
        let id = queue
            .enqueue(
                b"late-ok",
                MessageContract::best_effort(0, 20)
                    .deadline(5)
                    .batchable(false),
            )
            .unwrap();
        let mut radio = link(false, 0);
        assert_eq!(
            queue.service_one(0, &mut radio),
            ServiceEvent::RetryAt(id, 20)
        );
        radio.backend_mut().up = true;
        assert_eq!(
            queue.service_one(20, &mut radio),
            ServiceEvent::Delivered(id)
        );
        assert_eq!(queue.diagnostics().deadline_misses, 1);

        let expired = queue
            .enqueue(
                b"expired",
                MessageContract::best_effort(21, 22).batchable(false),
            )
            .unwrap();
        assert_eq!(
            queue.service_one(23, &mut radio),
            ServiceEvent::Expired(expired)
        );
    }

    #[test]
    fn explicit_link_recovery_preserves_the_queued_message() {
        let mut queue = FixedAdaptiveQueue::<1, 16>::fixed(AdaptivePolicy::responsive()).unwrap();
        let id = queue
            .enqueue(
                b"recover",
                MessageContract::best_effort(0, 5_000).batchable(false),
            )
            .unwrap();
        let mut radio = link(false, 0);
        assert_eq!(
            queue.service_one(0, &mut radio),
            ServiceEvent::RetryAt(id, 1_000)
        );
        assert!(radio.recover());
        assert_eq!(
            queue.service_one(1_000, &mut radio),
            ServiceEvent::Delivered(id)
        );
        assert_eq!(radio.diagnostics().recoveries, 1);
        assert_eq!(queue.diagnostics().link_down_deferrals, 1);
    }

    #[test]
    fn delayed_high_priority_message_reorders_ready_work_only() {
        let mut queue = FixedAdaptiveQueue::<3, 16>::fixed(AdaptivePolicy::responsive()).unwrap();
        let ready = queue
            .enqueue(
                b"ready",
                MessageContract::best_effort(0, 5_000)
                    .priority(1)
                    .batchable(false),
            )
            .unwrap();
        let delayed = queue
            .enqueue(
                b"delayed",
                MessageContract::best_effort(500, 5_000)
                    .priority(255)
                    .batchable(false),
            )
            .unwrap();
        let mut radio = link(true, 0);
        assert_eq!(
            queue.service_one(0, &mut radio),
            ServiceEvent::Delivered(ready)
        );
        assert_eq!(
            queue.service_one(0, &mut radio),
            ServiceEvent::IdleUntil(500)
        );
        assert_eq!(
            queue.service_one(500, &mut radio),
            ServiceEvent::Delivered(delayed)
        );
    }

    #[test]
    fn equal_priority_uses_earliest_deadline_before_offer_order() {
        let mut queue = FixedAdaptiveQueue::<2, 16>::fixed(AdaptivePolicy::responsive()).unwrap();
        let later_deadline = queue
            .enqueue(
                b"old",
                MessageContract::best_effort(0, 100)
                    .deadline(90)
                    .batchable(false),
            )
            .unwrap();
        let earlier_deadline = queue
            .enqueue(
                b"new",
                MessageContract::best_effort(10, 100)
                    .deadline(20)
                    .batchable(false),
            )
            .unwrap();
        let mut radio = link(true, 0);
        assert_eq!(
            queue.service_one(10, &mut radio),
            ServiceEvent::Delivered(earlier_deadline)
        );
        assert_eq!(
            queue.service_one(10, &mut radio),
            ServiceEvent::Delivered(later_deadline)
        );
    }

    #[test]
    fn backend_retry_exhaustion_is_bounded() {
        let policy = AdaptivePolicy {
            retry: RetryPolicy::exponential(2, 5, 5),
            batch_window_us: 0,
            max_batch_messages: 1,
        };
        let mut queue = FixedAdaptiveQueue::<1, 8>::fixed(policy).unwrap();
        let id = queue
            .enqueue(b"x", MessageContract::best_effort(0, 100).batchable(false))
            .unwrap();
        let mut radio = link(true, 2);
        assert_eq!(
            queue.service_one(0, &mut radio),
            ServiceEvent::RetryAt(id, 5)
        );
        assert_eq!(
            queue.service_one(5, &mut radio),
            ServiceEvent::RetryExhausted(id)
        );
        assert_eq!(queue.diagnostics().retry_attempts, 2);
        assert_eq!(queue.diagnostics().retry_exhaustions, 1);
    }

    #[test]
    fn exhausted_message_does_not_block_the_rest_of_a_batch() {
        let policy = AdaptivePolicy {
            retry: RetryPolicy::none(),
            batch_window_us: 0,
            max_batch_messages: 2,
        };
        let mut queue = FixedAdaptiveQueue::<2, 8>::fixed(policy).unwrap();
        let exhausted = queue
            .enqueue(
                b"drop",
                MessageContract::best_effort(0, 100)
                    .priority(2)
                    .batchable(false),
            )
            .unwrap();
        queue
            .enqueue(
                b"send",
                MessageContract::best_effort(0, 100)
                    .priority(1)
                    .batchable(false),
            )
            .unwrap();
        let mut radio = link(true, 1);
        assert_eq!(queue.service_batch(0, &mut radio), 1);
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.diagnostics().retry_exhaustions, 1);
        assert!(!queue.cancel(exhausted));
    }

    #[test]
    fn borrowed_pool_and_rate_snapshot_keep_storage_and_rates_visible() {
        let mut slots = [MessageSlot::<8>::empty(); 3];
        let mut queue =
            BorrowedAdaptiveQueue::borrowed(&mut slots, AdaptivePolicy::responsive()).unwrap();
        let before = queue.diagnostics();
        queue.enqueue(b"a", MessageContract::urgent(0, 10)).unwrap();
        queue.enqueue(b"b", MessageContract::urgent(0, 10)).unwrap();
        let mut radio = link(true, 0);
        assert_eq!(queue.service_batch(0, &mut radio), 1);
        radio.reset_window();
        assert_eq!(queue.service_batch(0, &mut radio), 1);
        let snapshot = queue.diagnostics().since(before, 500_000);
        assert_eq!(snapshot.offered_per_second, 4);
        assert_eq!(snapshot.observed_per_second, 4);
        assert_eq!(queue.capacity(), 3);
        assert_eq!(
            queue.reserved_storage_bytes(),
            3 * core::mem::size_of::<MessageSlot<8>>()
        );
    }

    struct CounterWake(Cell<u32>);
    impl CompletionWake for CounterWake {
        fn wake(&self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn callback_completion_only_wakes_owner() {
        let mut queue = FixedAdaptiveQueue::<1, 8>::fixed(AdaptivePolicy::responsive()).unwrap();
        let wake = CounterWake(Cell::new(0));
        queue.notify_completion(&wake);
        assert_eq!(wake.0.get(), 1);
        assert_eq!(queue.diagnostics().completion_wakes, 1);
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn heap_mode_reserves_once_and_remains_bounded() {
        let mut queue = HeapAdaptiveQueue::<16>::heap(2, AdaptivePolicy::responsive()).unwrap();
        let reserved = queue.reserved_heap_bytes();
        assert!(reserved >= 2 * core::mem::size_of::<MessageSlot<16>>());
        queue
            .enqueue(b"one", MessageContract::urgent(0, 10))
            .unwrap();
        queue
            .enqueue(b"two", MessageContract::urgent(0, 10))
            .unwrap();
        assert_eq!(queue.reserved_heap_bytes(), reserved);
        assert_eq!(
            queue.enqueue(b"three", MessageContract::urgent(0, 10)),
            Err(QueueError::Full)
        );
    }
}
