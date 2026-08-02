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
pub struct MessageId(u64);

impl MessageId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueError {
    InvalidPolicy,
    InvalidContract,
    PayloadTooLarge,
    Full,
    IdExhausted,
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
    next_id: u64,
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
        let id = self.allocate_id().ok_or(QueueError::IdExhausted)?;
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

    fn allocate_id(&mut self) -> Option<MessageId> {
        if self.next_id == 0 {
            return None;
        }
        let id = MessageId(self.next_id);
        self.next_id = self.next_id.checked_add(1).unwrap_or(0);
        Some(id)
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

/// Bytes prepended to a sequenced data-plane frame: session id then sequence number,
/// both little-endian. A session id changes after a sender restart, so delayed packets
/// from an earlier application instance cannot be mistaken for current work.
pub const SEQUENCED_HEADER_BYTES: usize = 8;

/// Encode one session-scoped packet without allocation.
pub fn encode_sequenced(
    session: u32,
    sequence: u32,
    payload: &[u8],
    destination: &mut [u8],
) -> Option<usize> {
    let len = SEQUENCED_HEADER_BYTES.checked_add(payload.len())?;
    if destination.len() < len {
        return None;
    }
    destination[..4].copy_from_slice(&session.to_le_bytes());
    destination[4..8].copy_from_slice(&sequence.to_le_bytes());
    destination[8..len].copy_from_slice(payload);
    Some(len)
}

/// Decode one frame produced by [`encode_sequenced`].
pub fn decode_sequenced(frame: &[u8]) -> Option<(u32, u32, &[u8])> {
    let session = u32::from_le_bytes(frame.get(..4)?.try_into().ok()?);
    let sequence = u32::from_le_bytes(frame.get(4..8)?.try_into().ok()?);
    Some((session, sequence, frame.get(8..)?))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngressError {
    InvalidCapacity,
    PayloadTooLarge,
    DestinationTooSmall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngressEvent {
    Empty,
    Delivered {
        sequence: u32,
        bytes: u16,
        reordered: bool,
        lost_before: u32,
    },
    Buffered {
        sequence: u32,
    },
    Backpressure {
        sequence: u32,
    },
    Duplicate {
        sequence: u32,
    },
    Expired {
        sequence: u32,
        lost_before: u32,
    },
    WrongSession {
        expected: u32,
        observed: u32,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IngressDiagnostics {
    pub observed_packets: u32,
    pub delivered_packets: u32,
    pub delivered_bytes: u64,
    pub buffered_packets: u32,
    pub reordered_packets: u32,
    pub duplicate_packets: u32,
    pub backpressure_rejections: u32,
    pub inferred_lost_packets: u32,
    pub window_evictions: u32,
    pub expired_packets: u32,
    pub session_rejections: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IngressSlot<const BYTES: usize> {
    occupied: bool,
    sequence: u32,
    expires_at_us: u64,
    len: u16,
    bytes: [u8; BYTES],
}

impl<const BYTES: usize> IngressSlot<BYTES> {
    const fn empty() -> Self {
        Self {
            occupied: false,
            sequence: 0,
            expires_at_us: 0,
            len: 0,
            bytes: [0; BYTES],
        }
    }
}

/// Fixed-storage receive sequencer for variable-delay packet links.
///
/// In-order packets are copied directly to caller storage. Future packets inside the
/// bounded window are retained until the gap arrives. A packet beyond the window makes
/// the missing range explicit and advances the receiver; stale/session-mismatched packets
/// fail closed. Sequence ordering is wrap-aware for distances below half the `u32` space.
pub struct SequencedReceiver<const SLOTS: usize, const BYTES: usize> {
    session: u32,
    next_sequence: u32,
    slots: [IngressSlot<BYTES>; SLOTS],
    diagnostics: IngressDiagnostics,
}

impl<const SLOTS: usize, const BYTES: usize> SequencedReceiver<SLOTS, BYTES> {
    pub fn new(session: u32, initial_sequence: u32) -> Result<Self, IngressError> {
        if SLOTS == 0 || BYTES == 0 || BYTES > usize::from(u16::MAX) {
            return Err(IngressError::InvalidCapacity);
        }
        Ok(Self {
            session,
            next_sequence: initial_sequence,
            slots: [IngressSlot::empty(); SLOTS],
            diagnostics: IngressDiagnostics::default(),
        })
    }

    pub const fn session(&self) -> u32 {
        self.session
    }

    pub const fn next_sequence(&self) -> u32 {
        self.next_sequence
    }

    pub const fn diagnostics(&self) -> IngressDiagnostics {
        self.diagnostics
    }

    /// Start an explicitly admitted sender session and discard only the old session's
    /// buffered packets. Applications decide when a peer restart is trustworthy.
    pub fn reset_session(&mut self, session: u32, initial_sequence: u32) {
        self.session = session;
        self.next_sequence = initial_sequence;
        self.clear_slots();
    }

    pub fn ingest(
        &mut self,
        now_us: u64,
        session: u32,
        sequence: u32,
        expires_at_us: u64,
        payload: &[u8],
        destination: &mut [u8],
    ) -> Result<IngressEvent, IngressError> {
        if payload.len() > BYTES || payload.len() > usize::from(u16::MAX) {
            return Err(IngressError::PayloadTooLarge);
        }
        self.diagnostics.observed_packets = self.diagnostics.observed_packets.saturating_add(1);
        if session != self.session {
            self.diagnostics.session_rejections =
                self.diagnostics.session_rejections.saturating_add(1);
            return Ok(IngressEvent::WrongSession {
                expected: self.session,
                observed: session,
            });
        }
        if now_us > expires_at_us {
            self.diagnostics.expired_packets = self.diagnostics.expired_packets.saturating_add(1);
            if sequence == self.next_sequence {
                self.next_sequence = self.next_sequence.wrapping_add(1);
            }
            return Ok(IngressEvent::Expired {
                sequence,
                lost_before: 0,
            });
        }

        let distance = sequence.wrapping_sub(self.next_sequence);
        if distance == 0 {
            if destination.len() < payload.len() {
                return Err(IngressError::DestinationTooSmall);
            }
            destination[..payload.len()].copy_from_slice(payload);
            self.next_sequence = self.next_sequence.wrapping_add(1);
            return Ok(self.delivered(sequence, payload.len(), false, 0));
        }
        if distance >= (1u32 << 31) {
            self.diagnostics.duplicate_packets =
                self.diagnostics.duplicate_packets.saturating_add(1);
            return Ok(IngressEvent::Duplicate { sequence });
        }
        if distance > SLOTS as u32 {
            if destination.len() < payload.len() {
                return Err(IngressError::DestinationTooSmall);
            }
            let buffered_before = self
                .slots
                .iter()
                .filter(|slot| {
                    if !slot.occupied {
                        return false;
                    }
                    let buffered_distance = slot.sequence.wrapping_sub(self.next_sequence);
                    buffered_distance < distance
                })
                .count() as u32;
            let lost = distance.saturating_sub(buffered_before);
            self.diagnostics.inferred_lost_packets =
                self.diagnostics.inferred_lost_packets.saturating_add(lost);
            self.diagnostics.window_evictions = self
                .diagnostics
                .window_evictions
                .saturating_add(buffered_before);
            self.clear_slots();
            self.next_sequence = sequence.wrapping_add(1);
            destination[..payload.len()].copy_from_slice(payload);
            return Ok(self.delivered(sequence, payload.len(), false, lost));
        }
        if self
            .slots
            .iter()
            .any(|slot| slot.occupied && slot.sequence == sequence)
        {
            self.diagnostics.duplicate_packets =
                self.diagnostics.duplicate_packets.saturating_add(1);
            return Ok(IngressEvent::Duplicate { sequence });
        }
        let Some(slot) = self.slots.iter_mut().find(|slot| !slot.occupied) else {
            self.diagnostics.backpressure_rejections =
                self.diagnostics.backpressure_rejections.saturating_add(1);
            return Ok(IngressEvent::Backpressure { sequence });
        };
        slot.bytes[..payload.len()].copy_from_slice(payload);
        slot.len = payload.len() as u16;
        slot.sequence = sequence;
        slot.expires_at_us = expires_at_us;
        slot.occupied = true;
        self.diagnostics.buffered_packets = self.diagnostics.buffered_packets.saturating_add(1);
        Ok(IngressEvent::Buffered { sequence })
    }

    /// Deliver one now-contiguous buffered packet. If the nearest buffered packet has
    /// expired while a gap remains, the missing range is accounted as loss and that
    /// expired packet is retired, allowing later work to progress on the next call.
    pub fn drain(
        &mut self,
        now_us: u64,
        destination: &mut [u8],
    ) -> Result<IngressEvent, IngressError> {
        let mut lost_before = 0;
        let mut index = self.slot_for(self.next_sequence);
        if index.is_none() {
            if let Some((candidate, distance)) = self.nearest_expired(now_us) {
                lost_before = distance;
                self.diagnostics.inferred_lost_packets = self
                    .diagnostics
                    .inferred_lost_packets
                    .saturating_add(distance);
                self.next_sequence = self.slots[candidate].sequence;
                index = Some(candidate);
            }
        }
        let Some(index) = index else {
            return Ok(IngressEvent::Empty);
        };
        let slot = self.slots[index];
        if now_us > slot.expires_at_us {
            self.slots[index] = IngressSlot::empty();
            self.next_sequence = self.next_sequence.wrapping_add(1);
            self.diagnostics.expired_packets = self.diagnostics.expired_packets.saturating_add(1);
            return Ok(IngressEvent::Expired {
                sequence: slot.sequence,
                lost_before,
            });
        }
        let len = usize::from(slot.len);
        if destination.len() < len {
            return Err(IngressError::DestinationTooSmall);
        }
        destination[..len].copy_from_slice(&slot.bytes[..len]);
        self.slots[index] = IngressSlot::empty();
        self.next_sequence = self.next_sequence.wrapping_add(1);
        Ok(self.delivered(slot.sequence, len, true, lost_before))
    }

    fn delivered(
        &mut self,
        sequence: u32,
        bytes: usize,
        reordered: bool,
        lost_before: u32,
    ) -> IngressEvent {
        self.diagnostics.delivered_packets = self.diagnostics.delivered_packets.saturating_add(1);
        self.diagnostics.delivered_bytes = self
            .diagnostics
            .delivered_bytes
            .saturating_add(bytes as u64);
        if reordered {
            self.diagnostics.reordered_packets =
                self.diagnostics.reordered_packets.saturating_add(1);
        }
        IngressEvent::Delivered {
            sequence,
            bytes: bytes as u16,
            reordered,
            lost_before,
        }
    }

    fn slot_for(&self, sequence: u32) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.occupied && slot.sequence == sequence)
    }

    fn nearest_expired(&self, now_us: u64) -> Option<(usize, u32)> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.occupied && now_us > slot.expires_at_us)
            .filter_map(|(index, slot)| {
                let distance = slot.sequence.wrapping_sub(self.next_sequence);
                (distance < (1u32 << 31)).then_some((index, distance))
            })
            .min_by_key(|(_, distance)| *distance)
    }

    fn clear_slots(&mut self) {
        self.slots.fill(IngressSlot::empty());
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
    fn message_id_exhaustion_fails_closed_without_reusing_an_old_id() {
        let mut queue = FixedAdaptiveQueue::<1, 8>::fixed(AdaptivePolicy::responsive()).unwrap();
        queue.next_id = u64::MAX;
        let last = queue
            .enqueue(b"last", MessageContract::urgent(0, 10))
            .unwrap();
        assert_eq!(last.get(), u64::MAX);
        assert!(queue.cancel(last));
        assert_eq!(
            queue.enqueue(b"reuse", MessageContract::urgent(0, 10)),
            Err(QueueError::IdExhausted)
        );
        assert!(queue.is_empty());
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

    #[cfg(feature = "alloc")]
    #[test]
    fn heap_mode_stays_bounded_over_heavy_churn() {
        // The opt-in heap path is the flexible counterpart to the fixed queue: it
        // reserves once, so 10k enqueue/cancel cycles must not allocate again or
        // grow the reserved footprint. Enabling flexibility never becomes
        // unbounded runtime growth beside the static tasks.
        let mut queue = HeapAdaptiveQueue::<16>::heap(4, AdaptivePolicy::responsive()).unwrap();
        let reserved = queue.reserved_heap_bytes();

        const N: u32 = 10_000;
        for _ in 0..N {
            let id = queue
                .enqueue(b"tick", MessageContract::urgent(0, u64::MAX))
                .unwrap();
            assert_eq!(queue.reserved_heap_bytes(), reserved);
            assert!(queue.cancel(id));
        }
        assert_eq!(queue.diagnostics().offered_messages, N);
        assert_eq!(queue.diagnostics().cancelled_messages, N);

        for _ in 0..4 {
            queue
                .enqueue(b"x", MessageContract::urgent(0, u64::MAX))
                .unwrap();
        }
        assert_eq!(
            queue.enqueue(b"x", MessageContract::urgent(0, u64::MAX)),
            Err(QueueError::Full)
        );
        assert_eq!(queue.reserved_heap_bytes(), reserved);
    }

    #[test]
    fn fixed_storage_stays_bounded_over_heavy_churn() {
        // The no-heap fixed queue is the certainty path for budget-critical work
        // (e.g. motor control): thousands of enqueue/cancel cycles must never grow
        // its footprint and must reuse slots exactly, so a flexible workload
        // elsewhere can never leak a slot or perturb the static path. Delivery is
        // covered by the scripted-radio tests; this stresses slot lifetime.
        let mut queue = FixedAdaptiveQueue::<4, 16>::fixed(AdaptivePolicy::responsive()).unwrap();
        let reserved = queue.reserved_storage_bytes();

        const N: u32 = 10_000;
        for _ in 0..N {
            let id = queue
                .enqueue(b"tick", MessageContract::urgent(0, u64::MAX))
                .unwrap();
            // Fixed, allocation-free storage: the footprint never grows.
            assert_eq!(queue.reserved_storage_bytes(), reserved);
            // Cancel frees the slot; the next enqueue must reuse it (never Full).
            assert!(queue.cancel(id));
        }
        assert_eq!(queue.diagnostics().offered_messages, N);
        assert_eq!(queue.diagnostics().cancelled_messages, N);

        // No slot leaked over the churn: the queue still fills to exactly its
        // capacity and rejects the next message, with the same fixed footprint.
        for _ in 0..4 {
            queue
                .enqueue(b"x", MessageContract::urgent(0, u64::MAX))
                .unwrap();
        }
        assert_eq!(
            queue.enqueue(b"x", MessageContract::urgent(0, u64::MAX)),
            Err(QueueError::Full)
        );
        assert_eq!(queue.reserved_storage_bytes(), reserved);
    }

    #[test]
    fn sequenced_wire_format_is_bounded_and_round_trips() {
        let mut frame = [0u8; 16];
        let len = encode_sequenced(0x1122_3344, 7, b"radio", &mut frame).unwrap();
        assert_eq!(len, 13);
        assert_eq!(
            decode_sequenced(&frame[..len]),
            Some((0x1122_3344, 7, b"radio".as_slice()))
        );
        assert!(encode_sequenced(1, 2, b"123456789", &mut frame).is_none());
        assert!(decode_sequenced(&frame[..7]).is_none());
    }

    #[test]
    fn sequenced_receiver_orders_rejects_duplicates_and_sessions() {
        let mut receiver = SequencedReceiver::<3, 8>::new(11, 0).unwrap();
        let mut out = [0u8; 8];

        assert_eq!(
            receiver.ingest(0, 11, 0, 100, b"zero", &mut out),
            Ok(IngressEvent::Delivered {
                sequence: 0,
                bytes: 4,
                reordered: false,
                lost_before: 0,
            })
        );
        assert_eq!(&out[..4], b"zero");
        assert_eq!(
            receiver.ingest(1, 11, 2, 100, b"two", &mut out),
            Ok(IngressEvent::Buffered { sequence: 2 })
        );
        assert_eq!(
            receiver.ingest(2, 11, 2, 100, b"two", &mut out),
            Ok(IngressEvent::Duplicate { sequence: 2 })
        );
        assert_eq!(
            receiver.ingest(3, 10, 1, 100, b"old", &mut out),
            Ok(IngressEvent::WrongSession {
                expected: 11,
                observed: 10,
            })
        );
        assert_eq!(
            receiver.ingest(4, 11, 1, 100, b"one", &mut out),
            Ok(IngressEvent::Delivered {
                sequence: 1,
                bytes: 3,
                reordered: false,
                lost_before: 0,
            })
        );
        assert_eq!(
            receiver.drain(4, &mut out),
            Ok(IngressEvent::Delivered {
                sequence: 2,
                bytes: 3,
                reordered: true,
                lost_before: 0,
            })
        );
        assert_eq!(&out[..3], b"two");
        let diagnostics = receiver.diagnostics();
        assert_eq!(diagnostics.observed_packets, 5);
        assert_eq!(diagnostics.delivered_packets, 3);
        assert_eq!(diagnostics.buffered_packets, 1);
        assert_eq!(diagnostics.reordered_packets, 1);
        assert_eq!(diagnostics.duplicate_packets, 1);
        assert_eq!(diagnostics.session_rejections, 1);
    }

    #[test]
    fn future_packet_admission_does_not_require_a_delivery_buffer() {
        let mut receiver = SequencedReceiver::<2, 8>::new(7, 0).unwrap();
        assert_eq!(
            receiver.ingest(0, 7, 1, 100, b"future", &mut []),
            Ok(IngressEvent::Buffered { sequence: 1 })
        );
        assert_eq!(
            receiver.ingest(0, 7, 0, 100, b"now", &mut []),
            Err(IngressError::DestinationTooSmall)
        );
        let mut output = [0; 8];
        assert!(matches!(
            receiver.ingest(0, 7, 0, 100, b"now", &mut output),
            Ok(IngressEvent::Delivered { sequence: 0, .. })
        ));
        assert!(matches!(
            receiver.drain(0, &mut output),
            Ok(IngressEvent::Delivered {
                sequence: 1,
                reordered: true,
                ..
            })
        ));
    }

    #[test]
    fn sequenced_receiver_accounts_window_loss_and_expired_gaps() {
        let mut receiver = SequencedReceiver::<2, 8>::new(9, 5).unwrap();
        let mut out = [0u8; 8];

        assert_eq!(
            receiver.ingest(0, 9, 8, 100, b"far", &mut out),
            Ok(IngressEvent::Delivered {
                sequence: 8,
                bytes: 3,
                reordered: false,
                lost_before: 3,
            })
        );
        assert_eq!(receiver.next_sequence(), 9);

        receiver.reset_session(10, 20);
        assert_eq!(
            receiver.ingest(0, 10, 21, 10, b"late", &mut out),
            Ok(IngressEvent::Buffered { sequence: 21 })
        );
        assert_eq!(
            receiver.drain(11, &mut out),
            Ok(IngressEvent::Expired {
                sequence: 21,
                lost_before: 1,
            })
        );
        let diagnostics = receiver.diagnostics();
        assert_eq!(diagnostics.inferred_lost_packets, 4);
        assert_eq!(diagnostics.expired_packets, 1);
    }
}
