//! First-class bounded async (UX-02): a real reactor, still no-alloc.
//!
//! [`BoundedExecutor`](crate::BoundedExecutor) (Wave 3) re-polls every pending
//! task with a no-op waker — honest, but an escape hatch. This module is the
//! application model that replaces it:
//!
//! - **Real wakers, deduplicated**: each task owns a static [`WakeCell`]; a
//!   wake sets one bit in the core's ready mask (idempotent — a wake storm is
//!   one poll, proven by test) and is ISR-safe (atomics only).
//! - **Fuel-bounded execution**: [`ReactorExecutor::run_ready`] polls only
//!   ready tasks and stops at a caller-supplied fuel, so a self-waking future
//!   cannot monopolize a cycle. Never-ready futures simply stay parked.
//! - **Bounded services**: [`TimerQueue`] (fixed slots, drop releases),
//!   [`Channel`] (bounded ring with parked wakers both sides — full means
//!   `Pending`, never drop or alloc), [`Signal`] (sticky notify), and
//!   [`CancelToken`] (cooperative cancellation with a wake).
//! - **Composition**: [`join2`] and [`select2`] over any futures.
//! - **No hidden executor**: the intended integration is one module whose
//!   [`KernelExecutor`](crate::KernelExecutor) dispatch drives
//!   `advance(now)` + `run_ready(fuel)` — async work is admitted, budgeted,
//!   measured, energy-charged, and recovered exactly like sync work (the
//!   mixed-graph test below runs the reactor under the kernel executor).
//!
//! Capacity rule: `N <= 32` tasks per core (one ready-mask word). Cores are
//! `'static` (the usual static-cell pattern) because wakers may outlive any
//! stack frame.

use core::cell::RefCell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use critical_section::Mutex;
// Drop-in atomics: native CAS where the ISA has it, critical-section fallback
// on CAS-less cores (thumbv6m/AVR) — matches scheduler.rs.
use portable_atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU8, Ordering};

use crate::async_exec::SpawnedTask;

/// Per-task wake state; lives in a `'static` [`AsyncCore`].
pub struct WakeCell {
    bit: AtomicU8,
    ready: AtomicPtr<AtomicU32>,
}

impl WakeCell {
    const fn new() -> Self {
        Self {
            bit: AtomicU8::new(0),
            ready: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    fn wake_bit(&self) {
        let ready = self.ready.load(Ordering::Acquire);
        if !ready.is_null() {
            // SAFETY: `ready` points into the same 'static AsyncCore that owns
            // this cell; it is never deallocated.
            unsafe { &*ready }.fetch_or(1 << self.bit.load(Ordering::Relaxed), Ordering::AcqRel);
        }
    }
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(
    |data| RawWaker::new(data, &VTABLE),
    |data| unsafe { &*(data as *const WakeCell) }.wake_bit(),
    |data| unsafe { &*(data as *const WakeCell) }.wake_bit(),
    |_| {},
);

/// The shared wake state for up to `N <= 32` tasks. Place in a `static`.
pub struct AsyncCore<const N: usize> {
    ready: AtomicU32,
    cells: [WakeCell; N],
}

impl<const N: usize> AsyncCore<N> {
    pub const fn new() -> Self {
        assert!(N <= 32, "AsyncCore supports at most 32 tasks per core");
        Self {
            ready: AtomicU32::new(0),
            cells: [const { WakeCell::new() }; N],
        }
    }

    fn init(&'static self) {
        for (index, cell) in self.cells.iter().enumerate() {
            cell.bit.store(index as u8, Ordering::Relaxed);
            cell.ready
                .store(&self.ready as *const _ as *mut _, Ordering::Release);
        }
    }

    fn waker(&'static self, index: usize) -> Waker {
        let data = &self.cells[index] as *const WakeCell as *const ();
        // SAFETY: the vtable functions only dereference `data` as the 'static
        // WakeCell it is; clone/wake/drop never free anything.
        unsafe { Waker::from_raw(RawWaker::new(data, &VTABLE)) }
    }
}

impl<const N: usize> Default for AsyncCore<N> {
    fn default() -> Self {
        Self::new()
    }
}

struct Slot<'a> {
    future: SpawnedTask<'a>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReactorStats {
    pub polled: u32,
    pub completed: u32,
    /// Live tasks remaining after the pass.
    pub live: u32,
    /// The fuel bound stopped the pass with ready work left (wake storm).
    pub fuel_exhausted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReactorError {
    Full,
}

/// Fuel-bounded, wake-driven executor over caller-owned futures.
pub struct ReactorExecutor<'a, const N: usize> {
    core: &'static AsyncCore<N>,
    slots: [Option<Slot<'a>>; N],
    count: usize,
}

impl<'a, const N: usize> ReactorExecutor<'a, N> {
    pub fn bind(core: &'static AsyncCore<N>) -> Self {
        core.init();
        core.ready.store(0, Ordering::Release);
        Self {
            core,
            slots: [const { None }; N],
            count: 0,
        }
    }

    pub fn spawn(&mut self, future: SpawnedTask<'a>) -> Result<usize, ReactorError> {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(Slot { future });
                self.count += 1;
                // Initial poll happens on the next run_ready pass.
                self.core.ready.fetch_or(1 << index, Ordering::AcqRel);
                return Ok(index);
            }
        }
        Err(ReactorError::Full)
    }

    pub fn live(&self) -> usize {
        self.count
    }

    /// Poll ready tasks only, spending at most `fuel` polls. Wakes arriving
    /// during a task's own poll are honored on a later pass (dedup by bit), so
    /// a self-waking task cannot starve its peers or the cycle budget.
    pub fn run_ready(&mut self, fuel: u32) -> ReactorStats {
        let mut stats = ReactorStats::default();
        let mut budget = fuel;
        loop {
            let batch = self.core.ready.fetch_and(0, Ordering::AcqRel);
            if batch == 0 {
                break;
            }
            for index in 0..N {
                if batch & (1 << index) == 0 {
                    continue;
                }
                if budget == 0 {
                    // Preserve this bit and the batch's un-polled remainder.
                    let unpolled = batch & (!0u32 << index);
                    self.core.ready.fetch_or(unpolled, Ordering::AcqRel);
                    stats.fuel_exhausted = true;
                    stats.live = self.count as u32;
                    return stats;
                }
                let Some(slot) = self.slots[index].as_mut() else {
                    continue;
                };
                budget -= 1;
                stats.polled += 1;
                let waker = self.core.waker(index);
                let mut cx = Context::from_waker(&waker);
                if slot.future.as_mut().poll(&mut cx).is_ready() {
                    stats.completed += 1;
                    self.slots[index] = None;
                    self.count -= 1;
                }
            }
            if budget == 0 {
                if self.core.ready.load(Ordering::Acquire) != 0 {
                    stats.fuel_exhausted = true;
                }
                break;
            }
        }
        stats.live = self.count as u32;
        stats
    }

    /// True when a wake is pending (the kernel-executor adapter's "poll me
    /// again" signal).
    pub fn has_ready(&self) -> bool {
        self.core.ready.load(Ordering::Acquire) != 0
    }
}

// ---------------------------------------------------------------- timers

struct TimerEntry {
    deadline_us: u64,
    waker: Waker,
    fired: bool,
}

/// Fixed-capacity timer service. Futures register on first poll; `advance`
/// fires due entries and wakes their tasks; dropping a [`Sleep`] releases its
/// slot (cancellation-safe).
pub struct TimerQueue<const T: usize> {
    slots: Mutex<RefCell<[Option<TimerEntry>; T]>>,
}

impl<const T: usize> TimerQueue<T> {
    #[allow(clippy::new_without_default)] // const-context construction
    pub const fn new() -> Self {
        Self {
            slots: Mutex::new(RefCell::new([const { None }; T])),
        }
    }

    /// Fire every entry with `deadline <= now`; returns how many woke.
    pub fn advance(&self, now_us: u64) -> usize {
        critical_section::with(|cs| {
            let mut slots = self.slots.borrow(cs).borrow_mut();
            let mut fired = 0;
            for entry in slots.iter_mut().flatten() {
                if !entry.fired && entry.deadline_us <= now_us {
                    entry.fired = true;
                    entry.waker.wake_by_ref();
                    fired += 1;
                }
            }
            fired
        })
    }

    /// Earliest un-fired deadline (the idle/sleep input).
    pub fn next_deadline_us(&self) -> Option<u64> {
        critical_section::with(|cs| {
            self.slots
                .borrow(cs)
                .borrow()
                .iter()
                .flatten()
                .filter(|entry| !entry.fired)
                .map(|entry| entry.deadline_us)
                .min()
        })
    }

    pub fn sleep_until(&self, deadline_us: u64) -> Sleep<'_, T> {
        Sleep {
            queue: self,
            deadline_us,
            slot: None,
        }
    }
}

pub struct Sleep<'q, const T: usize> {
    queue: &'q TimerQueue<T>,
    deadline_us: u64,
    slot: Option<usize>,
}

impl<const T: usize> Future for Sleep<'_, T> {
    /// `false` = no timer slot was available (bounded and honest, not silent).
    type Output = bool;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<bool> {
        let deadline_us = self.deadline_us;
        let taken = self.slot;
        let queue = self.queue;
        let (result, claimed) = critical_section::with(|cs| {
            let mut slots = queue.slots.borrow(cs).borrow_mut();
            match taken {
                Some(index) => match slots[index].as_mut() {
                    Some(entry) if entry.fired => {
                        slots[index] = None;
                        (Poll::Ready(true), Some(index))
                    }
                    Some(entry) => {
                        // Refresh the waker (task may have been re-bound).
                        entry.waker = cx.waker().clone();
                        (Poll::Pending, Some(index))
                    }
                    None => (Poll::Ready(true), Some(index)),
                },
                None => {
                    let Some(index) = slots.iter().position(|slot| slot.is_none()) else {
                        return (Poll::Ready(false), None);
                    };
                    slots[index] = Some(TimerEntry {
                        deadline_us,
                        waker: cx.waker().clone(),
                        fired: false,
                    });
                    (Poll::Pending, Some(index))
                }
            }
        });
        self.slot = claimed;
        if matches!(result, Poll::Ready(_)) {
            self.slot = None;
        }
        result
    }
}

impl<const T: usize> Drop for Sleep<'_, T> {
    fn drop(&mut self) {
        if let Some(index) = self.slot {
            critical_section::with(|cs| {
                self.queue.slots.borrow(cs).borrow_mut()[index] = None;
            });
        }
    }
}

// ---------------------------------------------------------------- channel

struct ChannelState<T, const C: usize> {
    ring: [Option<T>; C],
    head: usize,
    len: usize,
    rx_waker: Option<Waker>,
    tx_waker: Option<Waker>,
}

/// Bounded async channel: `send` parks when full (backpressure, never drops),
/// `recv` parks when empty. One parked waker per side (last waiter wins —
/// intended use is one producer task and one consumer task).
pub struct Channel<T, const C: usize> {
    state: Mutex<RefCell<ChannelState<T, C>>>,
}

impl<T, const C: usize> Channel<T, C> {
    #[allow(clippy::new_without_default)] // const-context construction
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(RefCell::new(ChannelState {
                ring: [const { None }; C],
                head: 0,
                len: 0,
                rx_waker: None,
                tx_waker: None,
            })),
        }
    }

    pub fn try_send(&self, value: T) -> Result<(), T> {
        critical_section::with(|cs| {
            let mut state = self.state.borrow(cs).borrow_mut();
            if state.len == C {
                return Err(value);
            }
            let tail = (state.head + state.len) % C;
            state.ring[tail] = Some(value);
            state.len += 1;
            if let Some(waker) = state.rx_waker.take() {
                waker.wake();
            }
            Ok(())
        })
    }

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
            if let Some(waker) = state.tx_waker.take() {
                waker.wake();
            }
            value
        })
    }

    pub fn send(&self, value: T) -> SendFuture<'_, T, C> {
        SendFuture {
            channel: self,
            value: Some(value),
        }
    }

    pub fn recv(&self) -> RecvFuture<'_, T, C> {
        RecvFuture { channel: self }
    }

    pub fn len(&self) -> usize {
        critical_section::with(|cs| self.state.borrow(cs).borrow().len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct SendFuture<'c, T, const C: usize> {
    channel: &'c Channel<T, C>,
    value: Option<T>,
}

impl<T: Unpin, const C: usize> Future for SendFuture<'_, T, C> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let value = self.value.take().expect("polled after completion");
        match self.channel.try_send(value) {
            Ok(()) => Poll::Ready(()),
            Err(value) => {
                critical_section::with(|cs| {
                    self.channel.state.borrow(cs).borrow_mut().tx_waker = Some(cx.waker().clone());
                });
                // Re-check after parking: the consumer may have drained between
                // the failed try_send and the park (lost-wake race).
                match self.channel.try_send(value) {
                    Ok(()) => Poll::Ready(()),
                    Err(value) => {
                        self.value = Some(value);
                        Poll::Pending
                    }
                }
            }
        }
    }
}

pub struct RecvFuture<'c, T, const C: usize> {
    channel: &'c Channel<T, C>,
}

impl<T, const C: usize> Future for RecvFuture<'_, T, C> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        if let Some(value) = self.channel.try_recv() {
            return Poll::Ready(value);
        }
        critical_section::with(|cs| {
            self.channel.state.borrow(cs).borrow_mut().rx_waker = Some(cx.waker().clone());
        });
        match self.channel.try_recv() {
            Some(value) => Poll::Ready(value),
            None => Poll::Pending,
        }
    }
}

// ------------------------------------------------------- signal + cancel

/// Sticky one-shot notification: `notify` wakes the parked waiter (or is
/// remembered until someone waits).
pub struct Signal {
    set: AtomicBool,
    waker: Mutex<RefCell<Option<Waker>>>,
}

impl Signal {
    #[allow(clippy::new_without_default)] // const-context construction
    pub const fn new() -> Self {
        Self {
            set: AtomicBool::new(false),
            waker: Mutex::new(RefCell::new(None)),
        }
    }

    pub fn notify(&self) {
        self.set.store(true, Ordering::Release);
        critical_section::with(|cs| {
            if let Some(waker) = self.waker.borrow(cs).borrow_mut().take() {
                waker.wake();
            }
        });
    }

    /// Await the signal; consumes it (resets to unsignaled).
    pub fn wait(&self) -> SignalWait<'_> {
        SignalWait { signal: self }
    }
}

pub struct SignalWait<'s> {
    signal: &'s Signal,
}

impl Future for SignalWait<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.signal.set.swap(false, Ordering::AcqRel) {
            return Poll::Ready(());
        }
        critical_section::with(|cs| {
            *self.signal.waker.borrow(cs).borrow_mut() = Some(cx.waker().clone());
        });
        if self.signal.set.swap(false, Ordering::AcqRel) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// Cooperative cancellation: `cancel()` is sticky, idempotent, and wakes the
/// parked waiter; `cancelled()` resolves once cancelled (select it against
/// the work being cancelled).
pub struct CancelToken {
    signal: Signal,
    cancelled: AtomicBool,
}

impl CancelToken {
    #[allow(clippy::new_without_default)] // const-context construction
    pub const fn new() -> Self {
        Self {
            signal: Signal::new(),
            cancelled: AtomicBool::new(false),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.signal.notify();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn cancelled(&self) -> Cancelled<'_> {
        Cancelled { token: self }
    }
}

pub struct Cancelled<'t> {
    token: &'t CancelToken,
}

impl Future for Cancelled<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.token.is_cancelled() {
            return Poll::Ready(());
        }
        critical_section::with(|cs| {
            *self.token.signal.waker.borrow(cs).borrow_mut() = Some(cx.waker().clone());
        });
        if self.token.is_cancelled() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

// ---------------------------------------------------------- combinators

/// Await both futures; resolves with both outputs.
pub fn join2<A: Future, B: Future>(a: A, b: B) -> Join2<A, B> {
    Join2 {
        a,
        b,
        a_out: None,
        b_out: None,
    }
}

pub struct Join2<A: Future, B: Future> {
    a: A,
    b: B,
    a_out: Option<A::Output>,
    b_out: Option<B::Output>,
}

impl<A: Future + Unpin, B: Future + Unpin> Future for Join2<A, B>
where
    A::Output: Unpin,
    B::Output: Unpin,
{
    type Output = (A::Output, B::Output);

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        if this.a_out.is_none() {
            if let Poll::Ready(out) = Pin::new(&mut this.a).poll(cx) {
                this.a_out = Some(out);
            }
        }
        if this.b_out.is_none() {
            if let Poll::Ready(out) = Pin::new(&mut this.b).poll(cx) {
                this.b_out = Some(out);
            }
        }
        if this.a_out.is_some() && this.b_out.is_some() {
            Poll::Ready((this.a_out.take().unwrap(), this.b_out.take().unwrap()))
        } else {
            Poll::Pending
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Either<A, B> {
    First(A),
    Second(B),
}

/// Await whichever future resolves first (the loser is dropped — its
/// resources, e.g. a timer slot, are released by its Drop).
pub fn select2<A: Future, B: Future>(a: A, b: B) -> Select2<A, B> {
    Select2 { a, b }
}

pub struct Select2<A: Future, B: Future> {
    a: A,
    b: B,
}

impl<A: Future + Unpin, B: Future + Unpin> Future for Select2<A, B> {
    type Output = Either<A::Output, B::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        if let Poll::Ready(out) = Pin::new(&mut this.a).poll(cx) {
            return Poll::Ready(Either::First(out));
        }
        if let Poll::Ready(out) = Pin::new(&mut this.b).poll(cx) {
            return Poll::Ready(Either::Second(out));
        }
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::pin::pin;
    use std::boxed::Box;

    fn leak_core<const N: usize>() -> &'static AsyncCore<N> {
        Box::leak(Box::new(AsyncCore::<N>::new()))
    }

    #[test]
    fn wake_storm_is_one_bit_and_fuel_bounds_hold() {
        let core = leak_core::<2>();
        let mut exec = ReactorExecutor::bind(core);

        // A task that wakes itself 100 times per poll and never finishes.
        struct Storm;
        impl Future for Storm {
            type Output = ();
            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
                for _ in 0..100 {
                    cx.waker().wake_by_ref(); // dedup: still ONE ready bit
                }
                Poll::Pending
            }
        }
        let storm = pin!(Storm);
        exec.spawn(storm).unwrap();

        let stats = exec.run_ready(3);
        // Fuel 3, one task: each pass polls it once; its self-wake re-arms it.
        assert_eq!(stats.polled, 3);
        assert!(stats.fuel_exhausted);
        assert!(exec.has_ready()); // still parked-ready, never lost
        assert_eq!(stats.live, 1);
    }

    #[test]
    fn never_ready_future_parks_without_spinning() {
        let core = leak_core::<1>();
        let mut exec = ReactorExecutor::bind(core);
        struct Never;
        impl Future for Never {
            type Output = ();
            fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
                Poll::Pending // never wakes itself
            }
        }
        let never = pin!(Never);
        exec.spawn(never).unwrap();
        assert_eq!(exec.run_ready(10).polled, 1); // the initial poll
        let second = exec.run_ready(10);
        assert_eq!(second.polled, 0); // parked: no wake, no poll, no spin
        assert!(!second.fuel_exhausted);
        assert_eq!(second.live, 1);
    }

    #[test]
    fn timers_fire_in_order_and_release_their_slots() {
        static QUEUE: TimerQueue<2> = TimerQueue::new();
        let core = leak_core::<2>();
        let mut exec = ReactorExecutor::bind(core);

        let early = pin!(async {
            assert!(QUEUE.sleep_until(100).await);
        });
        let late = pin!(async {
            assert!(QUEUE.sleep_until(200).await);
        });
        exec.spawn(early).unwrap();
        exec.spawn(late).unwrap();
        exec.run_ready(8); // both register
        assert_eq!(QUEUE.next_deadline_us(), Some(100));

        QUEUE.advance(150);
        let stats = exec.run_ready(8);
        assert_eq!(stats.completed, 1); // early fired, late still parked
        assert_eq!(QUEUE.next_deadline_us(), Some(200));

        QUEUE.advance(u64::MAX); // far-future advance: no wrap, late fires
        assert_eq!(exec.run_ready(8).completed, 1);
        assert_eq!(QUEUE.next_deadline_us(), None); // all slots released
        assert_eq!(exec.live(), 0);
    }

    #[test]
    fn full_channel_backpressures_and_resumes() {
        static CH: Channel<u32, 1> = Channel::new();
        let core = leak_core::<2>();
        let mut exec = ReactorExecutor::bind(core);

        static SUM: AtomicU32 = AtomicU32::new(0);
        let producer = pin!(async {
            CH.send(1).await;
            CH.send(2).await; // must park: capacity 1
            CH.send(3).await;
        });
        let consumer = pin!(async {
            for _ in 0..3 {
                SUM.fetch_add(CH.recv().await, Ordering::Relaxed);
            }
        });
        exec.spawn(producer).unwrap();
        exec.spawn(consumer).unwrap();
        for _ in 0..10 {
            exec.run_ready(8);
            if exec.live() == 0 {
                break;
            }
        }
        assert_eq!(exec.live(), 0, "both sides completed");
        assert_eq!(SUM.load(Ordering::Relaxed), 6);
        assert!(CH.is_empty());
    }

    #[test]
    fn cancellation_races_are_safe_and_sticky() {
        static TOKEN: CancelToken = CancelToken::new();
        let core = leak_core::<1>();
        let mut exec = ReactorExecutor::bind(core);
        let outer = pin!(async {
            TOKEN.cancelled().await;
        });
        exec.spawn(outer).unwrap();
        exec.run_ready(4);
        assert_eq!(exec.live(), 1); // parked on the token

        TOKEN.cancel();
        TOKEN.cancel(); // idempotent
        assert_eq!(exec.run_ready(4).completed, 1);
        assert!(TOKEN.is_cancelled()); // sticky after the fact
    }

    #[test]
    fn join_and_select_compose_and_losers_release_resources() {
        static SIG: Signal = Signal::new();
        static QUEUE: TimerQueue<2> = TimerQueue::new();
        let core = leak_core::<1>();
        let mut exec = ReactorExecutor::bind(core);

        let task = pin!(async {
            // select: the signal beats the far-future timer.
            let sleep = QUEUE.sleep_until(u64::MAX - 1);
            let winner = select2(SIG.wait(), pin!(sleep)).await;
            assert!(matches!(winner, Either::First(())));
            // join: both complete.
            let pair = join2(core::future::ready(7u8), core::future::ready(9u8)).await;
            assert_eq!(pair, (7, 9));
        });
        exec.spawn(task).unwrap();
        exec.run_ready(8);
        assert_eq!(exec.live(), 1);
        SIG.notify();
        assert_eq!(exec.run_ready(8).completed, 1);
        // The losing Sleep was dropped by select2: its slot must be free.
        assert_eq!(QUEUE.next_deadline_us(), None);
    }

    #[test]
    fn mixed_sync_async_graph_runs_under_the_kernel_executor() {
        use crate::{
            AppGraph, ContainmentPolicy, FaultThresholds, KernelExecutor, Poll as TaskPoll,
            Runtime, SystemProfile, TaskDecl,
        };
        use nobro_power::{PowerHookError, PowerMode, PowerPlatform};

        struct AlwaysOn;
        impl PowerPlatform for AlwaysOn {
            fn program_wake(&mut self, _: Option<u64>) -> Result<(), PowerHookError> {
                Ok(())
            }
            fn enter(&mut self, _: PowerMode) -> Result<(), PowerHookError> {
                Ok(())
            }
            fn suspend(&mut self, _: u16) -> Result<(), PowerHookError> {
                Ok(())
            }
            fn resume(&mut self, _: u16) -> Result<(), PowerHookError> {
                Ok(())
            }
        }

        // One sync task + one async-reactor task, admitted through the graph.
        let built = AppGraph::<2>::new()
            .task(TaskDecl::periodic("sync", 10_000))
            .unwrap()
            .task(TaskDecl::periodic("async", 10_000))
            .unwrap()
            .build::<3>()
            .unwrap();
        let sync_id = built.module_of("sync").unwrap();
        let mut runtime = Runtime::<4, 4, 8, 4, 8, 4, 16>::admit(
            &built.manifest,
            built.startup_nodes(),
            SystemProfile::NRF52840_CORE,
            FaultThresholds::DEFAULT,
        )
        .unwrap();
        runtime.boot_to_running(0).unwrap();
        let mut kernel =
            KernelExecutor::<4, 4, 4, 8, 4, 8, 4, 16>::new(runtime, ContainmentPolicy::Cooperative);
        for meta in built.tasks.iter().flatten() {
            kernel.add_task(*meta, 0).unwrap();
        }
        kernel.seal().unwrap();

        static QUEUE: TimerQueue<1> = TimerQueue::new();
        let core = leak_core::<1>();
        let mut reactor = ReactorExecutor::bind(core);
        let job = pin!(async {
            assert!(QUEUE.sleep_until(30_000).await);
        });
        reactor.spawn(job).unwrap();

        let mut sync_polls = 0u32;
        let mut power = AlwaysOn;
        for step in 0..8u64 {
            let now = step * 10_000;
            QUEUE.advance(now); // reactor time base = kernel time base
            kernel
                .run_cycle(
                    || now,
                    &mut power,
                    |ctx| {
                        if ctx.module() == sync_id {
                            sync_polls += 1;
                            Ok(TaskPoll::Ready)
                        } else {
                            // The async module: a fuel-bounded reactor slice.
                            // Budgets, measurement, energy, and recovery all apply
                            // because it IS a normal admitted task.
                            reactor.run_ready(4);
                            Ok(if reactor.live() == 0 {
                                TaskPoll::Ready
                            } else {
                                TaskPoll::Pending
                            })
                        }
                    },
                )
                .unwrap();
        }
        assert!(sync_polls >= 2);
        assert_eq!(reactor.live(), 0, "async job completed under the kernel");
        // Measured time was charged to the async module like any other.
        let async_id = built.module_of("async").unwrap();
        assert!(kernel.runtime().object_usage(async_id).is_some());
    }
}
