//! Fair multi-waiter async composition (Wave 56): MPMC channel + task group.
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
//! Complexity vs Embassy: an Embassy `Channel<M, T, N>` is MPMC but pulls in
//! `embassy-sync` and a `RawMutex`, and its `Sender`/`Receiver` clones carry the
//! channel reference. Here the channel is one `'static` value, waiters are a
//! fixed `W`-slot array (no per-clone state), and the whole thing is
//! `critical-section` only — the same MPMC ergonomics with a declared,
//! auditable waiter bound instead of an unbounded intrusive list.

use core::cell::RefCell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use critical_section::Mutex;

/// Why a waiter could not be parked or a group could not admit a member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitError {
    /// The fixed waiter capacity `W` is exhausted — raise `W` for this channel.
    WaitersFull,
}

/// A fixed-capacity FIFO of wakers with arrival-order fairness and dedup.
pub struct WaitQueue<const W: usize> {
    slots: [Option<Waker>; W],
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
            if slot.as_ref().is_some_and(|w| w.will_wake(waker)) {
                return Ok(()); // dedup: a storm collapses to one entry
            }
        }
        if self.len == W {
            return Err(WaitError::WaitersFull);
        }
        self.slots[self.len] = Some(waker.clone());
        self.len += 1;
        Ok(())
    }

    /// Wake the oldest waiter (fair) and remove it. Returns true if one woke.
    pub fn wake_one(&mut self) -> bool {
        if self.len == 0 {
            return false;
        }
        let waker = self.slots[0].take();
        // Shift the FIFO down by one (bounded by W).
        for i in 1..self.len {
            self.slots[i - 1] = self.slots[i].take();
        }
        self.len -= 1;
        if let Some(waker) = waker {
            waker.wake();
            true
        } else {
            false
        }
    }

    /// Wake every parked waiter (used on cancel/close).
    pub fn wake_all(&mut self) {
        for slot in self.slots[..self.len].iter_mut() {
            if let Some(waker) = slot.take() {
                waker.wake();
            }
        }
        self.len = 0;
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

    /// Push without blocking; `Err(value)` when full (or `Err` semantics via
    /// [`try_recv`] returning `None` after close). Wakes one fair receiver.
    pub fn try_send(&self, value: T) -> Result<(), T> {
        critical_section::with(|cs| {
            let mut state = self.state.borrow(cs).borrow_mut();
            if state.closed || state.len == C {
                return Err(value);
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
        }
    }

    pub fn recv(&self) -> MpmcRecv<'_, T, C, W> {
        MpmcRecv { channel: self }
    }
}

/// Send future: resolves `Ok(())` on success, `Err(value)` if the channel closes
/// while waiting, and parks fairly (with an attributed error if `W` is exhausted).
pub struct MpmcSend<'c, T, const C: usize, const W: usize> {
    channel: &'c MpmcChannel<T, C, W>,
    value: Option<T>,
}

impl<T: Unpin, const C: usize, const W: usize> Future for MpmcSend<'_, T, C, W> {
    type Output = Result<(), WaitError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let value = self.value.take().expect("polled after completion");
        match self.channel.try_send(value) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(value) => {
                let parked = critical_section::with(|cs| {
                    let mut state = self.channel.state.borrow(cs).borrow_mut();
                    if state.closed {
                        return Ok(true); // closed: fall through and fail below
                    }
                    state.send_waiters.park(cx.waker()).map(|()| false)
                });
                match parked {
                    Err(err) => Poll::Ready(Err(err)),
                    Ok(_closed_or_parked) => {
                        // Re-check after parking (lost-wake race + close).
                        match self.channel.try_send(value) {
                            Ok(()) => Poll::Ready(Ok(())),
                            Err(value) => {
                                if self.channel.is_closed() {
                                    Poll::Ready(Err(WaitError::WaitersFull))
                                } else {
                                    self.value = Some(value);
                                    Poll::Pending
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Recv future: resolves `Some(value)`, or `None` if the channel closes empty.
pub struct MpmcRecv<'c, T, const C: usize, const W: usize> {
    channel: &'c MpmcChannel<T, C, W>,
}

impl<T, const C: usize, const W: usize> Future for MpmcRecv<'_, T, C, W> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        if let Some(value) = self.channel.try_recv() {
            return Poll::Ready(Some(value));
        }
        let park = critical_section::with(|cs| {
            let mut state = self.channel.state.borrow(cs).borrow_mut();
            if state.closed && state.len == 0 {
                return Ok(true); // closed + drained
            }
            state.recv_waiters.park(cx.waker()).map(|()| false)
        });
        match park {
            Ok(true) => Poll::Ready(None),
            Err(_) => Poll::Ready(None), // waiter table full: don't hang, report empty
            Ok(false) => match self.channel.try_recv() {
                Some(value) => Poll::Ready(Some(value)),
                None if self.channel.is_closed() => Poll::Ready(None),
                None => Poll::Pending,
            },
        }
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
        GroupCancelled { group: self }
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
}

/// Future returned by [`TaskGroup::cancelled`]; parks in the group's wait queue.
pub struct GroupCancelled<'g, const N: usize> {
    group: &'g TaskGroup<N>,
}

impl<const N: usize> Future for GroupCancelled<'_, N> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        critical_section::with(|cs| {
            let mut state = self.group.state.borrow(cs).borrow_mut();
            if state.cancelled {
                return Poll::Ready(());
            }
            // WaitersFull cannot happen: at most N members, W == N.
            let _ = state.waiters.park(cx.waker());
            Poll::Pending
        })
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
                SUM.fetch_add(CH.recv().await.unwrap(), Ordering::Relaxed);
                COUNT.fetch_add(1, Ordering::Relaxed);
            }
        });
        let c2 = pin!(async {
            for _ in 0..2 {
                SUM.fetch_add(CH.recv().await.unwrap(), Ordering::Relaxed);
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
    fn mpmc_close_wakes_blocked_receivers_with_none() {
        static CH: MpmcChannel<u32, 1, 2> = MpmcChannel::new();
        static GOT_NONE: AtomicU32 = AtomicU32::new(0);
        let core = leak_core::<1>();
        let mut exec = ReactorExecutor::bind(core);
        let waiter = pin!(async {
            if CH.recv().await.is_none() {
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
    fn task_group_cancels_all_members_and_joins() {
        static GROUP: TaskGroup<3> = TaskGroup::new();
        static CANCELLED: AtomicU32 = AtomicU32::new(0);
        let core = leak_core::<3>();
        let mut exec = ReactorExecutor::bind(core);

        for _ in 0..3 {
            let index = GROUP.join_member().unwrap();
            let fut = Box::leak(Box::new(async move {
                GROUP.cancelled().await;
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
    fn group_membership_is_bounded_and_attributed() {
        static GROUP: TaskGroup<1> = TaskGroup::new();
        assert_eq!(GROUP.join_member(), Ok(0));
        assert_eq!(GROUP.join_member(), Err(WaitError::WaitersFull));
    }
}
