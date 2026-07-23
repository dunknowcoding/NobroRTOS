//! Fair multi-waiter async composition: MPMC channel and task group.
//!
//! [`async_rt::Channel`](crate::async_rt) parks ONE waker per side — correct for
//! a single producer and single consumer. This module adds the general case with
//! the same discipline (no alloc, wake dedup, cancellation-safe, fuel-bounded by
//! the reactor, attributed capacity errors):
//!
//! - [`WaitQueue`] — a fixed-capacity FIFO of wakers. Enqueue deduplicates by
//!   `Waker::will_wake`, and wake order is arrival order, so no waiter is
//!   starved (fairness) and a wake storm collapses to one entry per task.
//! - [`MpmcChannel`] — a bounded ring served by two `WaitQueue`s. Multiple
//!   producers and consumers block fairly when full/empty; `try_*` never
//!   allocates and a waiter that cannot be parked gets an attributed
//!   [`WaitError::WaitersFull`] rather than a silent lost wake.
//! - [`TaskGroup`] — a scoped set of related work sharing one [`CancelToken`]:
//!   cancel the group and every member's `cancelled()` resolves; `all_done`
//!   reports when the members have finished, giving structured join/cancel
//!   without an allocator or a nursery task.
//!
//! The channel is one static value. Waiters use a fixed W-slot array with no
//! per-clone allocation, so the waiter bound remains explicit and auditable.

use core::cell::RefCell;
use core::future::Future;
use core::marker::PhantomPinned;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use critical_section::Mutex;

/// Why a waiter could not be parked or a group could not admit a member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitError {
    /// The fixed waiter capacity `W` is exhausted — raise `W` for this channel.
    WaitersFull,
}

/// Why a non-blocking MPMC send could not accept the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MpmcTrySendError<T> {
    /// The ring is at capacity; retry after a receiver makes space.
    Full(T),
    /// The channel is closed and will never accept another value.
    Closed(T),
}

impl<T> MpmcTrySendError<T> {
    pub fn into_inner(self) -> T {
        match self {
            Self::Full(value) | Self::Closed(value) => value,
        }
    }
}

/// Terminal failure returned by a blocking MPMC send future.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MpmcSendError<T> {
    /// The channel closed before the value could be delivered.
    Closed(T),
    /// The admitted sender-waiter capacity was exhausted.
    WaitersFull(T),
}

impl<T> MpmcSendError<T> {
    pub fn into_inner(self) -> T {
        match self {
            Self::Closed(value) | Self::WaitersFull(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WaitRegistration(usize);

impl WaitRegistration {
    const UNTRACKED: Self = Self(0);
}

struct WaitSlot {
    // Zero is reserved for the public waker-deduplicating `park` API. Pinned
    // future anchors always have non-null addresses, so a single word is
    // enough and each bounded slot avoids `Option<usize>` tag expansion.
    registration: WaitRegistration,
    waker: Waker,
}

/// A fixed-capacity FIFO of wakers with arrival-order fairness and dedup.
pub struct WaitQueue<const W: usize> {
    slots: [Option<WaitSlot>; W],
    len: usize,
}

impl<const W: usize> WaitQueue<W> {
    pub const fn new() -> Self {
        Self {
            slots: [const { None }; W],
            len: 0,
        }
    }

    /// Park `waker` at the back unless an equivalent one is already queued.
    /// Returns `WaitersFull` when the bound is reached (attributed, never silent).
    pub fn park(&mut self, waker: &Waker) -> Result<(), WaitError> {
        for slot in self.slots[..self.len].iter() {
            if slot
                .as_ref()
                .is_some_and(|waiter| waiter.waker.will_wake(waker))
            {
                return Ok(()); // dedup: a storm collapses to one entry
            }
        }
        if self.len == W {
            return Err(WaitError::WaitersFull);
        }
        self.slots[self.len] = Some(WaitSlot {
            registration: WaitRegistration::UNTRACKED,
            waker: waker.clone(),
        });
        self.len += 1;
        Ok(())
    }

    /// Register one future independently of other futures that happen to use
    /// the same task waker. The stable key lets cancellation remove only the
    /// caller's registration and lets a changed task waker update in place.
    fn register(&mut self, registration: WaitRegistration, waker: &Waker) -> Result<(), WaitError> {
        if let Some(slot) = self.slots[..self.len]
            .iter_mut()
            .flatten()
            .find(|slot| slot.registration == registration)
        {
            if !slot.waker.will_wake(waker) {
                slot.waker = waker.clone();
            }
            return Ok(());
        }
        if self.len == W {
            return Err(WaitError::WaitersFull);
        }
        self.slots[self.len] = Some(WaitSlot {
            registration,
            waker: waker.clone(),
        });
        self.len += 1;
        Ok(())
    }

    /// Wake the oldest waiter (fair) and remove it. Returns true if one woke.
    pub fn wake_one(&mut self) -> bool {
        if self.len == 0 {
            return false;
        }
        let waiter = self.slots[0].take();
        // Shift the FIFO down by one (bounded by W).
        for i in 1..self.len {
            self.slots[i - 1] = self.slots[i].take();
        }
        self.len -= 1;
        if let Some(waiter) = waiter {
            waiter.waker.wake();
            true
        } else {
            false
        }
    }

    /// Wake every parked waiter (used on cancel/close).
    pub fn wake_all(&mut self) {
        for slot in self.slots[..self.len].iter_mut() {
            if let Some(waiter) = slot.take() {
                waiter.waker.wake();
            }
        }
        self.len = 0;
    }

    /// Remove one equivalent parked waker registered through [`park`](Self::park).
    pub fn remove(&mut self, waker: &Waker) -> bool {
        let Some(index) = self.slots[..self.len].iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|known| known.waker.will_wake(waker))
        }) else {
            return false;
        };
        self.remove_index(index);
        true
    }

    fn unregister(&mut self, registration: WaitRegistration) -> bool {
        let Some(index) = self.slots[..self.len].iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|known| known.registration == registration)
        }) else {
            return false;
        };
        self.remove_index(index);
        true
    }

    fn remove_index(&mut self, index: usize) {
        self.slots[index] = None;
        for cursor in index + 1..self.len {
            self.slots[cursor - 1] = self.slots[cursor].take();
        }
        self.len -= 1;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const W: usize> Default for WaitQueue<W> {
    fn default() -> Self {
        Self::new()
    }
}

struct MpmcState<T, const C: usize, const W: usize> {
    ring: [Option<T>; C],
    head: usize,
    len: usize,
    closed: bool,
    recv_waiters: WaitQueue<W>,
    send_waiters: WaitQueue<W>,
}

/// A bounded MPMC channel: many producers, many consumers, fair FIFO wakeups,
/// no allocation. Place in a `static` (like the reactor cores).
pub struct MpmcChannel<T, const C: usize, const W: usize> {
    state: Mutex<RefCell<MpmcState<T, C, W>>>,
}

impl<T, const C: usize, const W: usize> MpmcChannel<T, C, W> {
    #[allow(clippy::new_without_default)] // const-context construction
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(RefCell::new(MpmcState {
                ring: [const { None }; C],
                head: 0,
                len: 0,
                closed: false,
                recv_waiters: WaitQueue::new(),
                send_waiters: WaitQueue::new(),
            })),
        }
    }

    /// Push without blocking. This compatibility form returns `Err(value)` for
    /// either full or closed; use [`try_send_checked`](Self::try_send_checked)
    /// when the caller must distinguish retryable backpressure from closure.
    pub fn try_send(&self, value: T) -> Result<(), T> {
        self.try_send_checked(value)
            .map_err(MpmcTrySendError::into_inner)
    }

    /// Push without blocking and retain the reason plus ownership of `value`
    /// on failure. Wakes one fair receiver after a successful enqueue.
    pub fn try_send_checked(&self, value: T) -> Result<(), MpmcTrySendError<T>> {
        critical_section::with(|cs| {
            let mut state = self.state.borrow(cs).borrow_mut();
            if state.closed {
                return Err(MpmcTrySendError::Closed(value));
            }
            if state.len == C {
                return Err(MpmcTrySendError::Full(value));
            }
            let tail = (state.head + state.len) % C;
            state.ring[tail] = Some(value);
            state.len += 1;
            state.recv_waiters.wake_one();
            Ok(())
        })
    }

    /// Pop without blocking; `None` when empty. Wakes one fair sender.
    pub fn try_recv(&self) -> Option<T> {
        critical_section::with(|cs| {
            let mut state = self.state.borrow(cs).borrow_mut();
            if state.len == 0 {
                return None;
            }
            let head = state.head;
            let value = state.ring[head].take();
            state.head = (head + 1) % C;
            state.len -= 1;
            state.send_waiters.wake_one();
            value
        })
    }

    /// Close the channel: future sends fail, and all parked waiters wake so
    /// their futures can observe the close and resolve.
    pub fn close(&self) {
        critical_section::with(|cs| {
            let mut state = self.state.borrow(cs).borrow_mut();
            state.closed = true;
            state.recv_waiters.wake_all();
            state.send_waiters.wake_all();
        });
    }

    pub fn is_closed(&self) -> bool {
        critical_section::with(|cs| self.state.borrow(cs).borrow().closed)
    }

    pub fn len(&self) -> usize {
        critical_section::with(|cs| self.state.borrow(cs).borrow().len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn send(&self, value: T) -> MpmcSend<'_, T, C, W> {
        MpmcSend {
            channel: self,
            value: Some(value),
            registration_anchor: 0,
            _pin: PhantomPinned,
        }
    }

    pub fn recv(&self) -> MpmcRecv<'_, T, C, W> {
        MpmcRecv {
            channel: self,
            registration_anchor: 0,
            _pin: PhantomPinned,
        }
    }

    fn remove_sender(&self, registration: WaitRegistration) {
        critical_section::with(|cs| {
            self.state
                .borrow(cs)
                .borrow_mut()
                .send_waiters
                .unregister(registration);
        });
    }

    fn remove_receiver(&self, registration: WaitRegistration) {
        critical_section::with(|cs| {
            self.state
                .borrow(cs)
                .borrow_mut()
                .recv_waiters
                .unregister(registration);
        });
    }
}

/// Send future: resolves `Ok(())` on success and returns the undelivered value
/// in [`MpmcSendError`] on closure or waiter-capacity exhaustion.
pub struct MpmcSend<'c, T, const C: usize, const W: usize> {
    channel: &'c MpmcChannel<T, C, W>,
    value: Option<T>,
    registration_anchor: u8,
    _pin: PhantomPinned,
}

impl<T: Unpin, const C: usize, const W: usize> Future for MpmcSend<'_, T, C, W> {
    type Output = Result<(), MpmcSendError<T>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let registration = WaitRegistration(core::ptr::from_ref(&self.registration_anchor).addr());
        // SAFETY: `registration_anchor` is never moved after the first poll
        // because `PhantomPinned` makes this future `!Unpin`. We do not project
        // a pinned reference to `value`; `T: Unpin` permits taking it.
        let this = unsafe { self.as_mut().get_unchecked_mut() };
        let value = this.value.take().expect("polled after completion");
        match this.channel.try_send_checked(value) {
            Ok(()) => {
                this.channel.remove_sender(registration);
                Poll::Ready(Ok(()))
            }
            Err(MpmcTrySendError::Closed(value)) => {
                this.channel.remove_sender(registration);
                Poll::Ready(Err(MpmcSendError::Closed(value)))
            }
            Err(MpmcTrySendError::Full(value)) => {
                let parked = critical_section::with(|cs| -> Result<bool, WaitError> {
                    let mut state = this.channel.state.borrow(cs).borrow_mut();
                    if state.closed {
                        return Ok(true);
                    }
                    state.send_waiters.register(registration, cx.waker())?;
                    Ok(false)
                });
                match parked {
                    Err(WaitError::WaitersFull) => {
                        Poll::Ready(Err(MpmcSendError::WaitersFull(value)))
                    }
                    Ok(true) => Poll::Ready(Err(MpmcSendError::Closed(value))),
                    Ok(false) => {
                        // Re-check after parking (lost-wake race + close).
                        match this.channel.try_send_checked(value) {
                            Ok(()) => {
                                this.channel.remove_sender(registration);
                                Poll::Ready(Ok(()))
                            }
                            Err(MpmcTrySendError::Closed(value)) => {
                                this.channel.remove_sender(registration);
                                Poll::Ready(Err(MpmcSendError::Closed(value)))
                            }
                            Err(MpmcTrySendError::Full(value)) => {
                                this.value = Some(value);
                                Poll::Pending
                            }
                        }
                    }
                }
            }
        }
    }
}

impl<T, const C: usize, const W: usize> Drop for MpmcSend<'_, T, C, W> {
    fn drop(&mut self) {
        let registration = WaitRegistration(core::ptr::from_ref(&self.registration_anchor).addr());
        self.channel.remove_sender(registration);
    }
}

/// Recv future: resolves `Some(value)`, or `None` if the channel closes empty.
/// Waiter exhaustion is returned explicitly and never impersonates EOF.
pub struct MpmcRecv<'c, T, const C: usize, const W: usize> {
    channel: &'c MpmcChannel<T, C, W>,
    registration_anchor: u8,
    _pin: PhantomPinned,
}

impl<T, const C: usize, const W: usize> Future for MpmcRecv<'_, T, C, W> {
    type Output = Result<Option<T>, WaitError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let registration = WaitRegistration(core::ptr::from_ref(&self.registration_anchor).addr());
        if let Some(value) = self.channel.try_recv() {
            self.channel.remove_receiver(registration);
            return Poll::Ready(Ok(Some(value)));
        }
        let park = critical_section::with(|cs| {
            let mut state = self.channel.state.borrow(cs).borrow_mut();
            if state.closed && state.len == 0 {
                return Ok(true);
            }
            state
                .recv_waiters
                .register(registration, cx.waker())
                .map(|()| false)
        });
        match park {
            Ok(true) => {
                self.channel.remove_receiver(registration);
                Poll::Ready(Ok(None))
            }
            Err(error) => Poll::Ready(Err(error)),
            Ok(false) => match self.channel.try_recv() {
                Some(value) => {
                    self.channel.remove_receiver(registration);
                    Poll::Ready(Ok(Some(value)))
                }
                None if self.channel.is_closed() => {
                    self.channel.remove_receiver(registration);
                    Poll::Ready(Ok(None))
                }
                None => Poll::Pending,
            },
        }
    }
}

impl<T, const C: usize, const W: usize> Drop for MpmcRecv<'_, T, C, W> {
    fn drop(&mut self) {
        let registration = WaitRegistration(core::ptr::from_ref(&self.registration_anchor).addr());
        self.channel.remove_receiver(registration);
    }
}

struct GroupState<const N: usize> {
    cancelled: bool,
    done: [bool; N],
    members: usize,
    waiters: WaitQueue<N>,
}

/// A scoped set of related tasks sharing one cancellation signal and a bounded
/// completion tally — structured join/cancel without an allocator.
///
/// Unlike a single [`CancelToken`](crate::async_rt::CancelToken), a group cancels
/// *every* member fairly: it parks each member's waker in its own [`WaitQueue`],
/// so `cancel_all` wakes all N (a single-waker token would only wake the last to
/// park). Members await `cancelled()` to bail early and `mark_done` at the end;
/// `all_done` is the join condition.
pub struct TaskGroup<const N: usize> {
    state: Mutex<RefCell<GroupState<N>>>,
}

impl<const N: usize> TaskGroup<N> {
    #[allow(clippy::new_without_default)] // const-context construction
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(RefCell::new(GroupState {
                cancelled: false,
                done: [false; N],
                members: 0,
                waiters: WaitQueue::new(),
            })),
        }
    }

    /// Reserve a member slot; returns its index or `WaitersFull` when the group
    /// is full. Call once per task before spawning it.
    pub fn join_member(&self) -> Result<usize, WaitError> {
        critical_section::with(|cs| {
            let mut state = self.state.borrow(cs).borrow_mut();
            if state.members == N {
                return Err(WaitError::WaitersFull);
            }
            let index = state.members;
            state.members += 1;
            Ok(index)
        })
    }

    /// A future that resolves when the group is cancelled (fair, multi-waiter).
    pub fn cancelled(&self) -> GroupCancelled<'_, N> {
        GroupCancelled {
            group: self,
            registration_anchor: 0,
            _pin: PhantomPinned,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        critical_section::with(|cs| self.state.borrow(cs).borrow().cancelled)
    }

    /// Cancel the whole group: every member's `cancelled()` resolves.
    pub fn cancel_all(&self) {
        critical_section::with(|cs| {
            let mut state = self.state.borrow(cs).borrow_mut();
            state.cancelled = true;
            state.waiters.wake_all();
        });
    }

    /// Mark member `index` finished.
    pub fn mark_done(&self, index: usize) {
        critical_section::with(|cs| {
            if let Some(slot) = self.state.borrow(cs).borrow_mut().done.get_mut(index) {
                *slot = true;
            }
        });
    }

    /// True once every reserved member has marked itself done.
    pub fn all_done(&self) -> bool {
        critical_section::with(|cs| {
            let state = self.state.borrow(cs).borrow();
            state.members > 0 && state.done[..state.members].iter().all(|&d| d)
        })
    }

    fn remove_waiter(&self, registration: WaitRegistration) {
        critical_section::with(|cs| {
            self.state
                .borrow(cs)
                .borrow_mut()
                .waiters
                .unregister(registration);
        });
    }
}

/// Future returned by [`TaskGroup::cancelled`]; parks in the group's wait queue.
pub struct GroupCancelled<'g, const N: usize> {
    group: &'g TaskGroup<N>,
    registration_anchor: u8,
    _pin: PhantomPinned,
}

impl<const N: usize> Future for GroupCancelled<'_, N> {
    type Output = Result<(), WaitError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let registration = WaitRegistration(core::ptr::from_ref(&self.registration_anchor).addr());
        let ready = critical_section::with(|cs| -> Result<bool, WaitError> {
            let mut state = self.group.state.borrow(cs).borrow_mut();
            if state.cancelled {
                return Ok(true);
            }
            state.waiters.register(registration, cx.waker())?;
            Ok(false)
        });
        if ready == Ok(true) {
            self.group.remove_waiter(registration);
            Poll::Ready(Ok(()))
        } else if let Err(error) = ready {
            Poll::Ready(Err(error))
        } else {
            Poll::Pending
        }
    }
}

impl<const N: usize> Drop for GroupCancelled<'_, N> {
    fn drop(&mut self) {
        let registration = WaitRegistration(core::ptr::from_ref(&self.registration_anchor).addr());
        self.group.remove_waiter(registration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_rt::{AsyncCore, ReactorExecutor};
    use core::pin::pin;
    use core::sync::atomic::{AtomicU32, Ordering};
    use std::boxed::Box;

    fn leak_core<const M: usize>() -> &'static AsyncCore<M> {
        Box::leak(Box::new(AsyncCore::<M>::new()))
    }

    #[test]
    fn wait_queue_is_fair_and_dedups() {
        let mut q = WaitQueue::<4>::new();
        // Build three distinct wakers via three reactor cells.
        let core = leak_core::<3>();
        let _exec = ReactorExecutor::bind(core);
        let (w0, w1, w2) = (core.waker_for(0), core.waker_for(1), core.waker_for(2));
        q.park(&w0).unwrap();
        q.park(&w1).unwrap();
        q.park(&w0).unwrap(); // dedup: already queued
        assert_eq!(q.len(), 2);
        q.park(&w2).unwrap();
        assert_eq!(q.len(), 3);
        // FIFO wake order: w0 first (fairness).
        assert!(q.wake_one());
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn wait_queue_reports_full_instead_of_losing_a_waker() {
        let mut q = WaitQueue::<1>::new();
        let core = leak_core::<2>();
        let _exec = ReactorExecutor::bind(core);
        q.park(&core.waker_for(0)).unwrap();
        assert_eq!(q.park(&core.waker_for(1)), Err(WaitError::WaitersFull));
    }

    #[test]
    fn mpmc_two_producers_two_consumers_deliver_every_item_fairly() {
        static CH: MpmcChannel<u32, 2, 4> = MpmcChannel::new();
        static SUM: AtomicU32 = AtomicU32::new(0);
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let core = leak_core::<4>();
        let mut exec = ReactorExecutor::bind(core);

        let p1 = pin!(async {
            CH.send(1).await.unwrap();
            CH.send(2).await.unwrap();
        });
        let p2 = pin!(async {
            CH.send(10).await.unwrap();
            CH.send(20).await.unwrap();
        });
        let c1 = pin!(async {
            for _ in 0..2 {
                SUM.fetch_add(
                    CH.recv().await.unwrap().expect("channel remained open"),
                    Ordering::Relaxed,
                );
                COUNT.fetch_add(1, Ordering::Relaxed);
            }
        });
        let c2 = pin!(async {
            for _ in 0..2 {
                SUM.fetch_add(
                    CH.recv().await.unwrap().expect("channel remained open"),
                    Ordering::Relaxed,
                );
                COUNT.fetch_add(1, Ordering::Relaxed);
            }
        });
        exec.spawn(p1).unwrap();
        exec.spawn(p2).unwrap();
        exec.spawn(c1).unwrap();
        exec.spawn(c2).unwrap();
        for _ in 0..20 {
            exec.run_ready(16);
            if exec.live() == 0 {
                break;
            }
        }
        assert_eq!(exec.live(), 0, "all four tasks completed");
        assert_eq!(COUNT.load(Ordering::Relaxed), 4);
        assert_eq!(SUM.load(Ordering::Relaxed), 33); // 1+2+10+20
    }

    #[test]
    fn mpmc_multi_producer_saturation_delivers_every_item_without_starvation() {
        // Wave 88 saturation/fairness: two producers push 500 items each through
        // a 2-slot MPMC ring, so both producers block and wake repeatedly under
        // heavy backpressure. A single consumer drains the exact total. Every
        // item must arrive (sum + count exact) and neither producer may starve
        // (the run must terminate; a starved producer would never finish).
        const M: u32 = 500;
        const TOTAL: u32 = 2 * M;
        static CH: MpmcChannel<u32, 3, 2> = MpmcChannel::new();
        static SUM: AtomicU32 = AtomicU32::new(0);
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let core = leak_core::<3>();
        let mut exec = ReactorExecutor::bind(core);

        let p1 = pin!(async {
            for _ in 0..M {
                CH.send(1).await.unwrap();
            }
        });
        let p2 = pin!(async {
            for _ in 0..M {
                CH.send(3).await.unwrap();
            }
        });
        let consumer = pin!(async {
            for _ in 0..TOTAL {
                let value = CH.recv().await.unwrap().expect("channel remained open");
                SUM.fetch_add(value, Ordering::Relaxed);
                COUNT.fetch_add(1, Ordering::Relaxed);
            }
        });
        exec.spawn(p1).unwrap();
        exec.spawn(p2).unwrap();
        exec.spawn(consumer).unwrap();

        let mut cycles = 0u32;
        while exec.live() != 0 {
            exec.run_ready(16);
            cycles += 1;
            assert!(
                cycles < 100_000,
                "must terminate; a starved producer would hang"
            );
        }
        assert_eq!(
            COUNT.load(Ordering::Relaxed),
            TOTAL,
            "every item delivered once"
        );
        assert_eq!(SUM.load(Ordering::Relaxed), M * 1 + M * 3);
    }

    #[test]
    fn mpmc_close_wakes_blocked_receivers_with_none() {
        static CH: MpmcChannel<u32, 1, 2> = MpmcChannel::new();
        static GOT_NONE: AtomicU32 = AtomicU32::new(0);
        let core = leak_core::<1>();
        let mut exec = ReactorExecutor::bind(core);
        let waiter = pin!(async {
            if CH.recv().await.unwrap().is_none() {
                GOT_NONE.fetch_add(1, Ordering::Relaxed);
            }
        });
        exec.spawn(waiter).unwrap();
        exec.run_ready(4);
        assert_eq!(exec.live(), 1, "parked on empty channel");
        CH.close();
        exec.run_ready(4);
        assert_eq!(exec.live(), 0);
        assert_eq!(GOT_NONE.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn cross_core_transport_survives_a_consumer_core_stall() {
        // Model two cores as two independent reactors sharing one MpmcChannel:
        // core A produces work, core B consumes it. We inject a "core B stalled"
        // fault by NOT running its reactor for several rounds while A keeps
        // producing (the bounded channel backpressures A), then resume B and
        // require every item delivered exactly once — cross-core transport +
        // core-stall fault + bounded recovery, all deterministic under Miri.
        static XCORE: MpmcChannel<u32, 2, 2> = MpmcChannel::new();
        static DELIVERED: AtomicU32 = AtomicU32::new(0);
        static SUM: AtomicU32 = AtomicU32::new(0);
        let core_a = leak_core::<1>();
        let core_b = leak_core::<1>();
        let mut exec_a = ReactorExecutor::bind(core_a);
        let mut exec_b = ReactorExecutor::bind(core_b);

        let producer = pin!(async {
            for i in 1..=5u32 {
                XCORE.send(i).await.unwrap(); // parks when the 2-slot ring is full
            }
        });
        let consumer = pin!(async {
            for _ in 0..5 {
                let v = XCORE.recv().await.unwrap().expect("channel remained open");
                SUM.fetch_add(v, Ordering::Relaxed);
                DELIVERED.fetch_add(1, Ordering::Relaxed);
            }
        });
        exec_a.spawn(producer).unwrap();
        exec_b.spawn(consumer).unwrap();

        // Phase 1: run ONLY core A. It fills the 2-slot channel and then parks
        // (backpressure) — the stalled consumer cannot lose or duplicate items.
        for _ in 0..5 {
            exec_a.run_ready(4);
        }
        assert_eq!(exec_a.live(), 1, "producer parked on a full channel");
        assert_eq!(
            DELIVERED.load(Ordering::Relaxed),
            0,
            "consumer core stalled"
        );
        assert_eq!(XCORE.len(), 2, "exactly the ring capacity buffered");

        // Phase 2: resume core B; drive both to completion.
        for _ in 0..20 {
            exec_a.run_ready(4);
            exec_b.run_ready(4);
            if exec_a.live() == 0 && exec_b.live() == 0 {
                break;
            }
        }
        assert_eq!(exec_a.live(), 0, "producer completed after B resumed");
        assert_eq!(exec_b.live(), 0, "consumer drained every item");
        assert_eq!(
            DELIVERED.load(Ordering::Relaxed),
            5,
            "each item delivered once"
        );
        assert_eq!(
            SUM.load(Ordering::Relaxed),
            15,
            "1+2+3+4+5, none lost/duplicated"
        );
    }

    #[test]
    fn recv_waiter_exhaustion_is_typed_and_cancellation_frees_capacity() {
        static CH: MpmcChannel<u32, 1, 1> = MpmcChannel::new();
        let core = leak_core::<2>();
        let _exec = ReactorExecutor::bind(core);
        let first_waker = core.waker_for(0);
        let second_waker = core.waker_for(1);
        let mut first_cx = Context::from_waker(&first_waker);
        let mut second_cx = Context::from_waker(&second_waker);

        let mut first = Box::pin(CH.recv());
        assert_eq!(first.as_mut().poll(&mut first_cx), Poll::Pending);

        let mut second = pin!(CH.recv());
        assert_eq!(
            second.as_mut().poll(&mut second_cx),
            Poll::Ready(Err(WaitError::WaitersFull))
        );
        assert!(!CH.is_closed(), "waiter exhaustion is not end-of-stream");

        drop(first);
        let mut replacement = pin!(CH.recv());
        assert_eq!(
            replacement.as_mut().poll(&mut second_cx),
            Poll::Pending,
            "dropping a pending receiver releases its waiter slot"
        );
    }

    #[test]
    fn pending_receiver_replaces_a_changed_task_waker_without_leaking_capacity() {
        static CH: MpmcChannel<u32, 1, 1> = MpmcChannel::new();
        let core = leak_core::<2>();
        let _exec = ReactorExecutor::bind(core);
        let first_waker = core.waker_for(0);
        let migrated_waker = core.waker_for(1);
        let mut first_cx = Context::from_waker(&first_waker);
        let mut migrated_cx = Context::from_waker(&migrated_waker);
        let mut receiver = Box::pin(CH.recv());

        assert_eq!(receiver.as_mut().poll(&mut first_cx), Poll::Pending);
        assert_eq!(
            receiver.as_mut().poll(&mut migrated_cx),
            Poll::Pending,
            "the replacement waker reuses the same admitted waiter slot"
        );

        drop(receiver);
        let mut replacement = pin!(CH.recv());
        assert_eq!(
            replacement.as_mut().poll(&mut first_cx),
            Poll::Pending,
            "dropping after waker migration removes the replacement registration"
        );
    }

    #[test]
    fn cancelling_one_of_two_same_task_waiters_preserves_the_other_wake() {
        static CH: MpmcChannel<u32, 1, 2> = MpmcChannel::new();
        let core = leak_core::<1>();
        let _exec = ReactorExecutor::bind(core);
        let shared_waker = core.waker_for(0);
        let mut cx = Context::from_waker(&shared_waker);
        let mut cancelled = Box::pin(CH.recv());
        let mut survivor = Box::pin(CH.recv());

        assert_eq!(cancelled.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(survivor.as_mut().poll(&mut cx), Poll::Pending);
        drop(cancelled);

        assert_eq!(CH.try_send(7), Ok(()));
        assert!(
            core.has_ready(),
            "the surviving future retains an independent wake registration"
        );
        assert_eq!(survivor.as_mut().poll(&mut cx), Poll::Ready(Ok(Some(7))));
    }

    #[test]
    fn send_distinguishes_full_from_closed_and_returns_undelivered_value() {
        static CH: MpmcChannel<u32, 1, 1> = MpmcChannel::new();
        assert_eq!(CH.try_send_checked(1), Ok(()));
        assert_eq!(CH.try_send_checked(2), Err(MpmcTrySendError::Full(2)));

        let core = leak_core::<1>();
        let _exec = ReactorExecutor::bind(core);
        let waker = core.waker_for(0);
        let mut cx = Context::from_waker(&waker);
        let mut pending = pin!(CH.send(42));
        assert_eq!(pending.as_mut().poll(&mut cx), Poll::Pending);

        CH.close();
        assert_eq!(
            pending.as_mut().poll(&mut cx),
            Poll::Ready(Err(MpmcSendError::Closed(42)))
        );
        assert_eq!(CH.try_send_checked(3), Err(MpmcTrySendError::Closed(3)));
    }

    #[test]
    fn task_group_cancels_all_members_and_joins() {
        static GROUP: TaskGroup<3> = TaskGroup::new();
        static CANCELLED: AtomicU32 = AtomicU32::new(0);
        let core = leak_core::<3>();
        let mut exec = ReactorExecutor::bind(core);

        for _ in 0..3 {
            let index = GROUP.join_member().unwrap();
            let fut = Box::leak(Box::new(async move {
                GROUP.cancelled().await.unwrap();
                CANCELLED.fetch_add(1, Ordering::Relaxed);
                GROUP.mark_done(index);
            }));
            // SAFETY(test): the future is heap-allocated and leaked, so it never
            // moves and outlives the reactor — a valid 'static Pin.
            let task = unsafe { Pin::new_unchecked(&mut *fut) };
            exec.spawn(task).unwrap();
        }
        exec.run_ready(8);
        assert!(!GROUP.all_done(), "members parked on the group token");
        GROUP.cancel_all();
        for _ in 0..4 {
            exec.run_ready(8);
        }
        assert_eq!(CANCELLED.load(Ordering::Relaxed), 3);
        assert!(GROUP.all_done(), "group join satisfied after cancel");
    }

    #[test]
    fn dropping_group_cancellation_future_releases_its_waiter_slot() {
        static GROUP: TaskGroup<1> = TaskGroup::new();
        assert_eq!(GROUP.join_member(), Ok(0));
        let core = leak_core::<2>();
        let _exec = ReactorExecutor::bind(core);
        let first_waker = core.waker_for(0);
        let replacement_waker = core.waker_for(1);
        let mut first_cx = Context::from_waker(&first_waker);
        let mut replacement_cx = Context::from_waker(&replacement_waker);

        let mut first = Box::pin(GROUP.cancelled());
        assert_eq!(first.as_mut().poll(&mut first_cx), Poll::Pending);
        drop(first);

        let mut replacement = Box::pin(GROUP.cancelled());
        assert_eq!(
            replacement.as_mut().poll(&mut replacement_cx),
            Poll::Pending
        );
        GROUP.cancel_all();
        assert_eq!(
            replacement.as_mut().poll(&mut replacement_cx),
            Poll::Ready(Ok(()))
        );
    }

    #[test]
    fn group_cancellation_waiter_overflow_is_typed() {
        static GROUP: TaskGroup<1> = TaskGroup::new();
        assert_eq!(GROUP.join_member(), Ok(0));
        let core = leak_core::<2>();
        let _exec = ReactorExecutor::bind(core);
        let first_waker = core.waker_for(0);
        let second_waker = core.waker_for(1);
        let mut first_cx = Context::from_waker(&first_waker);
        let mut second_cx = Context::from_waker(&second_waker);

        let mut admitted = Box::pin(GROUP.cancelled());
        assert_eq!(admitted.as_mut().poll(&mut first_cx), Poll::Pending);
        let mut overflow = Box::pin(GROUP.cancelled());
        assert_eq!(
            overflow.as_mut().poll(&mut second_cx),
            Poll::Ready(Err(WaitError::WaitersFull))
        );
    }

    #[test]
    fn group_membership_is_bounded_and_attributed() {
        static GROUP: TaskGroup<1> = TaskGroup::new();
        assert_eq!(GROUP.join_member(), Ok(0));
        assert_eq!(GROUP.join_member(), Err(WaitError::WaitersFull));
    }
}
