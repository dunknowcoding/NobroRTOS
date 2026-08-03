//! Fixed-capacity, generation-safe cross-core data plane.
//!
//! The queue is deliberately single-producer/single-consumer. That matches the
//! platform shape used by the promoted two-core ports: the bootstrap core owns
//! admission and production, while one admitted secondary-core executor owns
//! consumption. A recovered secondary core may take over consumption only after
//! the old incarnation has stopped. Release/acquire publication makes payload
//! writes visible before the consumer observes the producer index without a
//! heap, lock, or target-specific cache assumption.

use core::{cell::UnsafeCell, mem::MaybeUninit};

use portable_atomic::{AtomicU32, AtomicUsize, Ordering};

/// One unit of admitted cross-core work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrossCoreMessage<T: Copy> {
    pub generation: u32,
    pub sequence: u32,
    pub payload: T,
}

/// Producer-side rejection. The original message is returned for a bounded
/// single-core fallback or retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossCoreSendError<T: Copy> {
    Full(CrossCoreMessage<T>),
    InactiveGeneration(CrossCoreMessage<T>),
    NonMonotonicSequence(CrossCoreMessage<T>),
}

/// Consumer disposition after removing one queue entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossCoreReceive<T: Copy> {
    Work(CrossCoreMessage<T>),
    Cancelled(CrossCoreMessage<T>),
    Stale(CrossCoreMessage<T>),
}

/// Generation transition failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossCoreGenerationError {
    Zero,
    NotNewer { current: u32, requested: u32 },
}

struct Slot<T: Copy>(UnsafeCell<MaybeUninit<CrossCoreMessage<T>>>);

impl<T: Copy> Slot<T> {
    const fn new() -> Self {
        Self(UnsafeCell::new(MaybeUninit::uninit()))
    }
}

/// Allocation-free SPSC queue with one deliberately unused ring slot.
///
/// `N` is the storage slot count and the usable capacity is `N - 1`. Exactly
/// one producer and one consumer may operate concurrently. A stopped consumer
/// may be replaced by a recovered core; two consumers must never overlap.
pub struct CrossCoreDataPlane<T: Copy, const N: usize> {
    slots: [Slot<T>; N],
    producer: AtomicUsize,
    consumer: AtomicUsize,
    active_generation: AtomicU32,
    last_sequence: AtomicU32,
    cancelled_through: AtomicU32,
}

// Slot ownership is separated by the producer/consumer indices. Publication
// uses Release and observation uses Acquire. `T: Copy + Send` prevents a
// cross-core transfer of a non-Send value or a value with drop obligations.
unsafe impl<T: Copy + Send, const N: usize> Sync for CrossCoreDataPlane<T, N> {}

impl<T: Copy, const N: usize> Default for CrossCoreDataPlane<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy, const N: usize> CrossCoreDataPlane<T, N> {
    pub const fn new() -> Self {
        assert!(N >= 2, "a cross-core ring needs at least two slots");
        Self {
            slots: [const { Slot::new() }; N],
            producer: AtomicUsize::new(0),
            consumer: AtomicUsize::new(0),
            active_generation: AtomicU32::new(0),
            last_sequence: AtomicU32::new(0),
            cancelled_through: AtomicU32::new(0),
        }
    }

    pub const fn capacity(&self) -> usize {
        N - 1
    }

    pub fn active_generation(&self) -> u32 {
        self.active_generation.load(Ordering::Acquire)
    }

    /// Begin a strictly newer secondary-core incarnation.
    ///
    /// Entries already in the ring are intentionally retained. The replacement
    /// consumer drains and reports them as [`CrossCoreReceive::Stale`], proving
    /// that a restart cannot silently execute work from the failed incarnation.
    pub fn begin_generation(&self, generation: u32) -> Result<(), CrossCoreGenerationError> {
        if generation == 0 {
            return Err(CrossCoreGenerationError::Zero);
        }
        let current = self.active_generation.load(Ordering::Acquire);
        if generation <= current {
            return Err(CrossCoreGenerationError::NotNewer {
                current,
                requested: generation,
            });
        }
        self.cancelled_through.store(0, Ordering::Release);
        self.last_sequence.store(0, Ordering::Release);
        self.active_generation.store(generation, Ordering::Release);
        Ok(())
    }

    pub fn try_send(&self, message: CrossCoreMessage<T>) -> Result<(), CrossCoreSendError<T>> {
        if message.generation == 0 || message.generation != self.active_generation() {
            return Err(CrossCoreSendError::InactiveGeneration(message));
        }
        let last = self.last_sequence.load(Ordering::Acquire);
        if message.sequence == 0 || message.sequence <= last {
            return Err(CrossCoreSendError::NonMonotonicSequence(message));
        }

        let head = self.producer.load(Ordering::Relaxed);
        let next = Self::next(head);
        if next == self.consumer.load(Ordering::Acquire) {
            return Err(CrossCoreSendError::Full(message));
        }

        // SAFETY: this is the sole producer, and `next != consumer` proves the
        // producer-owned slot is not concurrently read by the sole consumer.
        unsafe { (*self.slots[head].0.get()).write(message) };
        self.last_sequence
            .store(message.sequence, Ordering::Release);
        self.producer.store(next, Ordering::Release);
        Ok(())
    }

    /// Cancel all current-generation work through `sequence`.
    pub fn cancel_through(&self, generation: u32, sequence: u32) -> bool {
        if generation == 0 || generation != self.active_generation() {
            return false;
        }
        let mut current = self.cancelled_through.load(Ordering::Acquire);
        while sequence > current {
            match self.cancelled_through.compare_exchange_weak(
                current,
                sequence,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
        true
    }

    pub fn try_receive(&self) -> Option<CrossCoreReceive<T>> {
        let tail = self.consumer.load(Ordering::Relaxed);
        if tail == self.producer.load(Ordering::Acquire) {
            return None;
        }

        // SAFETY: the Acquire load observed publication of this initialized
        // slot. This is the sole consumer and advances the consumer index only
        // after copying the message out.
        let message = unsafe { (*self.slots[tail].0.get()).assume_init_read() };
        self.consumer.store(Self::next(tail), Ordering::Release);

        if message.generation != self.active_generation() {
            Some(CrossCoreReceive::Stale(message))
        } else if message.sequence <= self.cancelled_through.load(Ordering::Acquire) {
            Some(CrossCoreReceive::Cancelled(message))
        } else {
            Some(CrossCoreReceive::Work(message))
        }
    }

    pub fn is_empty(&self) -> bool {
        self.consumer.load(Ordering::Acquire) == self.producer.load(Ordering::Acquire)
    }

    const fn next(index: usize) -> usize {
        if index + 1 == N { 0 } else { index + 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(generation: u32, sequence: u32, payload: u32) -> CrossCoreMessage<u32> {
        CrossCoreMessage {
            generation,
            sequence,
            payload,
        }
    }

    #[test]
    fn bounded_fifo_saturates_and_returns_original_work() {
        let plane = CrossCoreDataPlane::<u32, 3>::new();
        plane.begin_generation(1).unwrap();
        plane.try_send(msg(1, 1, 11)).unwrap();
        plane.try_send(msg(1, 2, 22)).unwrap();
        assert_eq!(
            plane.try_send(msg(1, 3, 33)),
            Err(CrossCoreSendError::Full(msg(1, 3, 33)))
        );
        assert_eq!(
            plane.try_receive(),
            Some(CrossCoreReceive::Work(msg(1, 1, 11)))
        );
        assert_eq!(
            plane.try_receive(),
            Some(CrossCoreReceive::Work(msg(1, 2, 22)))
        );
        assert!(plane.try_receive().is_none());
    }

    #[test]
    fn cancellation_is_generation_scoped() {
        let plane = CrossCoreDataPlane::<u32, 4>::new();
        plane.begin_generation(7).unwrap();
        plane.try_send(msg(7, 1, 10)).unwrap();
        plane.try_send(msg(7, 2, 20)).unwrap();
        assert!(plane.cancel_through(7, 1));
        assert!(!plane.cancel_through(6, u32::MAX));
        assert_eq!(
            plane.try_receive(),
            Some(CrossCoreReceive::Cancelled(msg(7, 1, 10)))
        );
        assert_eq!(
            plane.try_receive(),
            Some(CrossCoreReceive::Work(msg(7, 2, 20)))
        );
    }

    #[test]
    fn restart_retains_but_rejects_stale_entries() {
        let plane = CrossCoreDataPlane::<u32, 4>::new();
        plane.begin_generation(3).unwrap();
        plane.try_send(msg(3, 1, 99)).unwrap();
        plane.begin_generation(4).unwrap();
        plane.try_send(msg(4, 1, 44)).unwrap();
        assert_eq!(
            plane.try_receive(),
            Some(CrossCoreReceive::Stale(msg(3, 1, 99)))
        );
        assert_eq!(
            plane.try_receive(),
            Some(CrossCoreReceive::Work(msg(4, 1, 44)))
        );
    }

    #[test]
    fn generations_and_sequences_fail_closed() {
        let plane = CrossCoreDataPlane::<u32, 3>::new();
        assert_eq!(
            plane.begin_generation(0),
            Err(CrossCoreGenerationError::Zero)
        );
        plane.begin_generation(2).unwrap();
        assert_eq!(
            plane.begin_generation(2),
            Err(CrossCoreGenerationError::NotNewer {
                current: 2,
                requested: 2,
            })
        );
        assert_eq!(
            plane.try_send(msg(1, 1, 1)),
            Err(CrossCoreSendError::InactiveGeneration(msg(1, 1, 1)))
        );
        assert_eq!(
            plane.try_send(msg(2, 0, 1)),
            Err(CrossCoreSendError::NonMonotonicSequence(msg(2, 0, 1)))
        );
        plane.try_send(msg(2, 1, 1)).unwrap();
        assert_eq!(
            plane.try_send(msg(2, 1, 2)),
            Err(CrossCoreSendError::NonMonotonicSequence(msg(2, 1, 2)))
        );
    }
}
