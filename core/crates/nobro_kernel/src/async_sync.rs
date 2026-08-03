//! Fixed-capacity synchronization and composition primitives for the reactor.
//!
//! These types have no hidden executor, allocation, or background work. Their
//! futures are driven by the caller's admitted reactor cycle.

use core::cell::RefCell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use critical_section::Mutex;
use portable_atomic::{AtomicBool, Ordering};

struct ChannelState<T, const C: usize> {
    ring: [Option<T>; C],
    head: usize,
    len: usize,
    rx_waker: Option<Waker>,
    tx_waker: Option<Waker>,
}

/// Bounded async channel: `send` parks when full and `recv` parks when empty.
/// One parked waker is retained per side, so this primitive is for one producer
/// task and one consumer task. Use the bounded MPMC surface for multiple waiters.
pub struct Channel<T, const C: usize> {
    state: Mutex<RefCell<ChannelState<T, C>>>,
}

impl<T, const C: usize> Channel<T, C> {
    #[allow(clippy::new_without_default)]
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

/// Sticky one-shot notification. `notify` wakes the parked waiter or remains
/// set until the next waiter consumes it.
pub struct Signal {
    set: AtomicBool,
    waker: Mutex<RefCell<Option<Waker>>>,
}

impl Signal {
    #[allow(clippy::new_without_default)]
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

/// Cooperative cancellation token with sticky, idempotent cancellation.
pub struct CancelToken {
    signal: Signal,
    cancelled: AtomicBool,
}

impl CancelToken {
    #[allow(clippy::new_without_default)]
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

/// Await both futures and return both outputs.
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

/// Await whichever future resolves first. Dropping the loser releases any
/// resources owned by that future, such as a timer slot.
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
