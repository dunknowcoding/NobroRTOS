//! First-class bounded async (UX-02): a real reactor, still no-alloc.
//!
//! [`BoundedExecutor`](crate::BoundedExecutor) re-polls every pending
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
use nobro_admission::InterruptProfile;
// Drop-in atomics: native CAS where the ISA has it, critical-section fallback
// on CAS-less cores (thumbv6m/AVR) — matches scheduler.rs.
use portable_atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU8, Ordering};

use crate::async_exec::SpawnedTask;
use crate::{FaultContext, FaultSource, HealthFault, KernelError};

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

    /// The waker for task slot `index` — wake it to re-poll that task (e.g. from
    /// an ISR, or when parking it in a [`WaitQueue`](crate::async_mpmc)).
    /// Panics if `index >= N`.
    pub fn waker_for(&'static self, index: usize) -> Waker {
        assert!(index < N, "task slot out of range");
        self.waker(index)
    }

    /// True when at least one task has been woken and still needs a reactor
    /// poll. Kernel adapters use this lock-free signal after any peripheral
    /// interrupt, so hardware completion does not have to wait for a timer or
    /// periodic release.
    pub fn has_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire) != 0
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

    fn core_identity(&self) -> usize {
        core::ptr::from_ref(self.core).addr()
    }

    /// Poll ready tasks only, spending at most `fuel` polls. Wakes arriving
    /// during a task's own poll are honored on a later pass (dedup by bit), so
    /// a self-waking task cannot starve its peers or the cycle budget.
    pub fn run_ready(&mut self, fuel: u32) -> ReactorStats {
        let mut stats = ReactorStats::default();
        let mut budget = fuel;
        loop {
            let mut batch = self.core.ready.fetch_and(0, Ordering::AcqRel);
            if batch == 0 {
                break;
            }
            while batch != 0 {
                let ready_bit = batch & batch.wrapping_neg();
                let index = ready_bit.trailing_zeros() as usize;
                if budget == 0 {
                    // Preserve this bit and the batch's un-polled remainder.
                    self.core.ready.fetch_or(batch, Ordering::AcqRel);
                    stats.fuel_exhausted = true;
                    stats.live = self.count as u32;
                    return stats;
                }
                batch &= !ready_bit;
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
        self.core.has_ready()
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

    /// Wrap a caller-pinned future in a deadline guard backed by this timer
    /// queue. The wrapper resolves to `Err(DeadlineFault)` when the compare
    /// deadline fires first, so a late async completion is never silently
    /// treated as success.
    pub fn with_deadline<'a, F: Future + ?Sized>(
        &'a self,
        phase_us: u64,
        period_us: u64,
        deadline_us: u64,
        future: Pin<&'a mut F>,
    ) -> DeadlineFuture<'a, F, T> {
        with_deadline(self, phase_us, period_us, deadline_us, future)
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

// -------------------------------------------------------- deadline guard

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeadlineContractError {
    ZeroPeriod,
    ZeroDeadline,
    DeadlineExceedsPeriod,
    AbsoluteDeadlineOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AsyncDeadline {
    pub phase_us: u64,
    pub period_us: u64,
    pub deadline_us: u64,
    pub absolute_deadline_us: u64,
}

impl AsyncDeadline {
    pub const fn new(
        phase_us: u64,
        period_us: u64,
        deadline_us: u64,
    ) -> Result<Self, DeadlineContractError> {
        if period_us == 0 {
            return Err(DeadlineContractError::ZeroPeriod);
        }
        if deadline_us == 0 {
            return Err(DeadlineContractError::ZeroDeadline);
        }
        if deadline_us > period_us {
            return Err(DeadlineContractError::DeadlineExceedsPeriod);
        }
        let Some(absolute_deadline_us) = phase_us.checked_add(deadline_us) else {
            return Err(DeadlineContractError::AbsoluteDeadlineOverflow);
        };
        Ok(Self {
            phase_us,
            period_us,
            deadline_us,
            absolute_deadline_us,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeadlineFaultKind {
    InvalidContract(DeadlineContractError),
    TimerUnavailable,
    Missed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeadlineFault {
    pub kind: DeadlineFaultKind,
    pub deadline: AsyncDeadline,
}

impl DeadlineFault {
    pub const fn invalid(error: DeadlineContractError) -> Self {
        Self {
            kind: DeadlineFaultKind::InvalidContract(error),
            deadline: AsyncDeadline {
                phase_us: 0,
                period_us: 0,
                deadline_us: 0,
                absolute_deadline_us: 0,
            },
        }
    }

    pub const fn timer_unavailable(deadline: AsyncDeadline) -> Self {
        Self {
            kind: DeadlineFaultKind::TimerUnavailable,
            deadline,
        }
    }

    pub const fn missed(deadline: AsyncDeadline) -> Self {
        Self {
            kind: DeadlineFaultKind::Missed,
            deadline,
        }
    }

    pub const fn health_fault(self) -> HealthFault {
        let (error, source, code) = match self.kind {
            DeadlineFaultKind::Missed => (KernelError::DeadlineMissed, FaultSource::Scheduler, 1),
            DeadlineFaultKind::TimerUnavailable => {
                (KernelError::QuotaBreach, FaultSource::Kernel, 2)
            }
            DeadlineFaultKind::InvalidContract(_) => {
                (KernelError::QuotaBreach, FaultSource::Kernel, 3)
            }
        };
        HealthFault::new(
            error,
            FaultContext::new(
                source,
                code,
                self.deadline.absolute_deadline_us as u32,
                self.deadline.deadline_us as u32,
            ),
        )
    }
}

/// Build a deadline-scoped async operation over a caller-pinned future.
///
/// The deadline is registered in the supplied [`TimerQueue`] and is polled
/// before the wrapped future on later wakes. Once the queue fires, the wrapper
/// returns a [`DeadlineFaultKind::Missed`] fault even if the wrapped future
/// would also become ready in that same pass. This preserves the RTOS rule that
/// missed deadlines flow through health/recovery instead of becoming silent
/// late successes.
pub fn with_deadline<'a, F: Future + ?Sized, const T: usize>(
    queue: &'a TimerQueue<T>,
    phase_us: u64,
    period_us: u64,
    deadline_us: u64,
    future: Pin<&'a mut F>,
) -> DeadlineFuture<'a, F, T> {
    let deadline = AsyncDeadline::new(phase_us, period_us, deadline_us);
    DeadlineFuture {
        queue,
        future,
        deadline: deadline.ok(),
        invalid: deadline.err(),
        sleep: None,
    }
}

pub struct DeadlineFuture<'a, F: Future + ?Sized, const T: usize> {
    queue: &'a TimerQueue<T>,
    future: Pin<&'a mut F>,
    deadline: Option<AsyncDeadline>,
    invalid: Option<DeadlineContractError>,
    sleep: Option<Sleep<'a, T>>,
}

impl<F: Future + ?Sized, const T: usize> Unpin for DeadlineFuture<'_, F, T> {}

impl<F: Future + ?Sized, const T: usize> Future for DeadlineFuture<'_, F, T> {
    type Output = Result<F::Output, DeadlineFault>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        if let Some(error) = this.invalid.take() {
            return Poll::Ready(Err(DeadlineFault::invalid(error)));
        }
        let deadline = this
            .deadline
            .expect("deadline future polled after completion");
        if this.sleep.is_none() {
            this.sleep = Some(this.queue.sleep_until(deadline.absolute_deadline_us));
        }
        if let Some(sleep) = this.sleep.as_mut() {
            match Pin::new(sleep).poll(cx) {
                Poll::Ready(false) => {
                    this.sleep = None;
                    return Poll::Ready(Err(DeadlineFault::timer_unavailable(deadline)));
                }
                Poll::Ready(true) => {
                    this.sleep = None;
                    return Poll::Ready(Err(DeadlineFault::missed(deadline)));
                }
                Poll::Pending => {}
            }
        }
        match this.future.as_mut().poll(cx) {
            Poll::Ready(output) => {
                this.sleep = None;
                this.deadline = None;
                Poll::Ready(Ok(output))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

// ---------------------------------------------------- priority domains

/// Contract for one async reactor domain. Each domain is expected to be driven
/// by one already-admitted kernel task; this plan only validates the async-side
/// capacity and cross-domain wiring before the application constructs the
/// concrete [`ReactorExecutor`] instances.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReactorDomainContract {
    pub id: u8,
    /// Lower numbers are more urgent; the value is portable metadata and is
    /// mapped to NVIC/ISR priorities only by a board-specific backend.
    pub priority_band: u8,
    pub task_slots: u8,
    pub timer_slots: u8,
    pub fuel_per_cycle: u32,
}

impl ReactorDomainContract {
    pub const fn new(id: u8, priority_band: u8) -> Self {
        Self {
            id,
            priority_band,
            task_slots: 1,
            timer_slots: 0,
            fuel_per_cycle: 1,
        }
    }

    pub const fn task_slots(mut self, task_slots: u8) -> Self {
        self.task_slots = task_slots;
        self
    }

    pub const fn timer_slots(mut self, timer_slots: u8) -> Self {
        self.timer_slots = timer_slots;
        self
    }

    pub const fn fuel_per_cycle(mut self, fuel_per_cycle: u32) -> Self {
        self.fuel_per_cycle = fuel_per_cycle;
        self
    }
}

/// Declares one bounded channel between async reactor domains. Same-domain
/// channels are allowed; cross-domain channels are surfaced explicitly in the
/// admitted plan so applications use an MPMC/cross-domain-safe transport rather
/// than accidentally sharing a single-waker SPSC channel across domains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReactorChannelContract {
    pub from_domain: u8,
    pub to_domain: u8,
    pub capacity: u8,
    pub waiter_slots: u8,
}

impl ReactorChannelContract {
    pub const fn new(from_domain: u8, to_domain: u8, capacity: u8) -> Self {
        Self {
            from_domain,
            to_domain,
            capacity,
            waiter_slots: 2,
        }
    }

    pub const fn waiter_slots(mut self, waiter_slots: u8) -> Self {
        self.waiter_slots = waiter_slots;
        self
    }

    pub const fn is_cross_domain(self) -> bool {
        self.from_domain != self.to_domain
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReactorAdmissionError {
    EmptyDomains,
    DuplicateDomain(u8),
    InvalidTaskSlots { domain: u8, task_slots: u8 },
    InvalidFuel { domain: u8 },
    InvalidChannelCapacity { index: usize },
    InvalidWaiterSlots { index: usize },
    UnknownChannelDomain { index: usize, domain: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReactorAdmissionPlan<const D: usize, const C: usize> {
    pub domains: [Option<ReactorDomainContract>; D],
    pub channels: [Option<ReactorChannelContract>; C],
    pub cross_domain_channels: [Option<ReactorChannelContract>; C],
    pub domain_len: usize,
    pub channel_len: usize,
    pub cross_domain_len: usize,
}

/// Explicit target binding for one portable reactor urgency domain.
///
/// `logical_priority` uses the same convention as
/// [`InterruptProfile`]: zero is most urgent, before any target-specific
/// register shift. A board backend must consume an admitted
/// [`ReactorPriorityPlan`] rather than programming priorities from the
/// portable `priority_band` directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReactorPriorityBinding {
    pub domain: u8,
    pub logical_priority: u8,
}

impl ReactorPriorityBinding {
    pub const fn new(domain: u8, logical_priority: u8) -> Self {
        Self {
            domain,
            logical_priority,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReactorPriorityError {
    InvalidInterruptProfile {
        priority_levels: u8,
        max_nesting: u8,
    },
    UnknownDomain {
        domain: u8,
    },
    DuplicateDomainBinding {
        domain: u8,
    },
    MissingDomainBinding {
        domain: u8,
    },
    PriorityOutOfRange {
        domain: u8,
        priority: u8,
        priority_levels: u8,
    },
    ReservedPriority {
        domain: u8,
        priority: u8,
    },
    DuplicatePriority {
        priority: u8,
        first_domain: u8,
        duplicate_domain: u8,
    },
    DuplicatePriorityBand {
        priority_band: u8,
        first_domain: u8,
        duplicate_domain: u8,
    },
    PriorityOrderMismatch {
        more_urgent_domain: u8,
        less_urgent_domain: u8,
    },
    NestingExceeded {
        domains: usize,
        max_nesting: u8,
    },
}

/// Reactor plan whose portable urgency domains have been bound to concrete
/// target logical interrupt priorities. This is the only mapping a board
/// backend should use when arming ISR-driven reactor wakes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReactorPriorityPlan<const D: usize, const C: usize> {
    pub reactor: ReactorAdmissionPlan<D, C>,
    pub bindings: [Option<ReactorPriorityBinding>; D],
    pub binding_len: usize,
    pub interrupt_profile: InterruptProfile,
}

impl<const D: usize, const C: usize> ReactorPriorityPlan<D, C> {
    pub fn bindings(&self) -> impl Iterator<Item = &ReactorPriorityBinding> + '_ {
        self.bindings.iter().flatten()
    }

    pub fn binding(&self, domain: u8) -> Option<&ReactorPriorityBinding> {
        self.bindings().find(|binding| binding.domain == domain)
    }
}

trait ReactorCoreSource {
    fn has_ready(&self) -> bool;
    fn task_slots(&self) -> usize;
    fn identity(&self) -> usize;
}

impl<const N: usize> ReactorCoreSource for AsyncCore<N> {
    fn has_ready(&self) -> bool {
        AsyncCore::has_ready(self)
    }

    fn task_slots(&self) -> usize {
        N
    }

    fn identity(&self) -> usize {
        core::ptr::from_ref(self).addr()
    }
}

trait ReactorTimerSource {
    fn advance(&self, now_us: u64) -> usize;
    fn next_deadline_us(&self) -> Option<u64>;
    fn timer_slots(&self) -> usize;
    fn identity(&self) -> usize;
}

impl<const T: usize> ReactorTimerSource for TimerQueue<T> {
    fn advance(&self, now_us: u64) -> usize {
        TimerQueue::advance(self, now_us)
    }

    fn next_deadline_us(&self) -> Option<u64> {
        TimerQueue::next_deadline_us(self)
    }

    fn timer_slots(&self) -> usize {
        T
    }

    fn identity(&self) -> usize {
        core::ptr::from_ref(self).addr()
    }
}

/// Concrete bounded core/timer storage for one admitted reactor domain.
///
/// Construction does not admit the runtime by itself. Pass one value per
/// domain to [`GraphReactorAdmission::bind_runtime`](crate::GraphReactorAdmission::bind_runtime);
/// that step checks IDs, exact capacities, graph drivers, and scheduler
/// priority order before a [`KernelExecutor`](crate::KernelExecutor) may use
/// the set.
#[derive(Clone, Copy)]
pub struct ReactorRuntimeDomain<'a> {
    id: u8,
    core: &'a dyn ReactorCoreSource,
    timers: &'a dyn ReactorTimerSource,
}

impl<'a> ReactorRuntimeDomain<'a> {
    pub fn new<const TASKS: usize, const TIMERS: usize>(
        id: u8,
        core: &'a AsyncCore<TASKS>,
        timers: &'a TimerQueue<TIMERS>,
    ) -> Self {
        Self { id, core, timers }
    }

    pub const fn id(&self) -> u8 {
        self.id
    }

    pub fn task_slots(&self) -> usize {
        self.core.task_slots()
    }

    pub fn timer_slots(&self) -> usize {
        self.timers.timer_slots()
    }

    pub(crate) fn core_identity(&self) -> usize {
        self.core.identity()
    }

    pub(crate) fn timer_identity(&self) -> usize {
        self.timers.identity()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReactorRuntimeError {
    UnknownDomain {
        domain: u8,
    },
    DuplicateDomain {
        domain: u8,
    },
    DuplicateCore {
        first_domain: u8,
        duplicate_domain: u8,
    },
    DuplicateTimerQueue {
        first_domain: u8,
        duplicate_domain: u8,
    },
    MissingDomain {
        domain: u8,
    },
    TaskSlotsMismatch {
        domain: u8,
        admitted: u8,
        runtime: usize,
    },
    TimerSlotsMismatch {
        domain: u8,
        admitted: u8,
        runtime: usize,
    },
    DuplicatePriorityBand {
        priority_band: u8,
        first_domain: u8,
        duplicate_domain: u8,
    },
    DriverPriorityOrderMismatch {
        more_urgent_domain: u8,
        less_urgent_domain: u8,
    },
    UnknownModule {
        module: crate::ModuleId,
    },
    CoreMismatch {
        domain: u8,
    },
}

#[derive(Clone, Copy)]
pub(crate) struct BoundReactorRuntime<'a> {
    domain: u8,
    fuel_per_cycle: u32,
    module: crate::ModuleId,
    runtime: ReactorRuntimeDomain<'a>,
}

/// Fixed, graph-linked runtime sources for every admitted reactor domain.
///
/// The set contains references only: concrete [`ReactorExecutor`] instances
/// remain application-owned and are polled by their normal admitted graph
/// tasks. This integrates wake/deadline state without creating a hidden
/// executor or allocating.
#[derive(Clone, Copy)]
pub struct ReactorRuntimeSet<'a, const D: usize> {
    pub(crate) domains: [Option<BoundReactorRuntime<'a>>; D],
    len: usize,
}

impl<'a, const D: usize> ReactorRuntimeSet<'a, D> {
    pub(crate) const fn from_bound(
        domains: [Option<BoundReactorRuntime<'a>>; D],
        len: usize,
    ) -> Self {
        Self { domains, len }
    }

    pub(crate) const fn bind_domain(
        domain: ReactorDomainContract,
        module: crate::ModuleId,
        runtime: ReactorRuntimeDomain<'a>,
    ) -> BoundReactorRuntime<'a> {
        BoundReactorRuntime {
            domain: domain.id,
            fuel_per_cycle: domain.fuel_per_cycle,
            module,
            runtime,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Retained bytes for the binding set itself. Concrete cores, timer queues,
    /// executors, futures, and channels remain separate admitted storage.
    pub const fn storage_bytes() -> usize {
        core::mem::size_of::<Self>()
    }

    pub fn module_for_domain(&self, domain: u8) -> Option<crate::ModuleId> {
        self.domains
            .iter()
            .flatten()
            .find(|bound| bound.domain == domain)
            .map(|bound| bound.module)
    }

    pub fn fuel_for_module(&self, module: crate::ModuleId) -> Option<u32> {
        self.domains
            .iter()
            .flatten()
            .find(|bound| bound.module == module)
            .map(|bound| bound.fuel_per_cycle)
    }

    /// Poll the concrete executor only with its admitted per-cycle fuel.
    /// A different core, even with the same capacity, is rejected.
    pub fn run_domain<const N: usize>(
        &self,
        module: crate::ModuleId,
        reactor: &mut ReactorExecutor<'_, N>,
    ) -> Result<ReactorStats, ReactorRuntimeError> {
        let Some(bound) = self
            .domains
            .iter()
            .flatten()
            .find(|bound| bound.module == module)
        else {
            return Err(ReactorRuntimeError::UnknownModule { module });
        };
        if bound.runtime.core.identity() != reactor.core_identity() {
            return Err(ReactorRuntimeError::CoreMismatch {
                domain: bound.domain,
            });
        }
        Ok(reactor.run_ready(bound.fuel_per_cycle))
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &BoundReactorRuntime<'a>> + '_ {
        self.domains.iter().flatten()
    }

    pub(crate) fn next_deadline_us(&self) -> Option<u64> {
        self.iter()
            .filter_map(|bound| bound.runtime.timers.next_deadline_us())
            .min()
    }

    pub(crate) fn has_ready(&self) -> bool {
        self.iter().any(|bound| bound.runtime.core.has_ready())
    }
}

impl BoundReactorRuntime<'_> {
    pub(crate) const fn module(&self) -> crate::ModuleId {
        self.module
    }

    pub(crate) fn advance(&self, now_us: u64) -> usize {
        self.runtime.timers.advance(now_us)
    }

    pub(crate) fn has_ready(&self) -> bool {
        self.runtime.core.has_ready()
    }
}

impl<const D: usize, const C: usize> ReactorAdmissionPlan<D, C> {
    pub fn domains(&self) -> impl Iterator<Item = &ReactorDomainContract> + '_ {
        self.domains.iter().flatten()
    }

    pub fn channels(&self) -> impl Iterator<Item = &ReactorChannelContract> + '_ {
        self.channels.iter().flatten()
    }

    pub fn cross_domain_channels(&self) -> impl Iterator<Item = &ReactorChannelContract> + '_ {
        self.cross_domain_channels.iter().flatten()
    }

    pub fn domain(&self, id: u8) -> Option<&ReactorDomainContract> {
        self.domains().find(|domain| domain.id == id)
    }

    /// Bind every portable reactor urgency domain to one target logical
    /// interrupt priority using the same profile as P-ISR admission.
    ///
    /// The mapping rejects reserved/out-of-range levels, missing or duplicate
    /// bindings, priority sharing, duplicate portable bands, target nesting
    /// overflow, and any inversion of portable urgency. It does not enable an
    /// interrupt; target code must program hardware only from the returned
    /// admitted plan.
    pub fn bind_interrupt_priorities(
        self,
        bindings: [Option<ReactorPriorityBinding>; D],
        interrupt_profile: InterruptProfile,
    ) -> Result<ReactorPriorityPlan<D, C>, ReactorPriorityError> {
        if interrupt_profile.priority_levels == 0
            || interrupt_profile.priority_levels > 8
            || interrupt_profile.max_nesting == 0
        {
            return Err(ReactorPriorityError::InvalidInterruptProfile {
                priority_levels: interrupt_profile.priority_levels,
                max_nesting: interrupt_profile.max_nesting,
            });
        }

        let mut binding_len = 0usize;
        for binding in bindings.iter().flatten() {
            binding_len += 1;
            if self.domain(binding.domain).is_none() {
                return Err(ReactorPriorityError::UnknownDomain {
                    domain: binding.domain,
                });
            }
            if binding.logical_priority >= interrupt_profile.priority_levels {
                return Err(ReactorPriorityError::PriorityOutOfRange {
                    domain: binding.domain,
                    priority: binding.logical_priority,
                    priority_levels: interrupt_profile.priority_levels,
                });
            }
            if interrupt_profile.reserved_priorities & (1u8 << binding.logical_priority) != 0 {
                return Err(ReactorPriorityError::ReservedPriority {
                    domain: binding.domain,
                    priority: binding.logical_priority,
                });
            }
        }

        for (index, binding) in bindings.iter().flatten().enumerate() {
            for previous in bindings.iter().flatten().take(index) {
                if previous.domain == binding.domain {
                    return Err(ReactorPriorityError::DuplicateDomainBinding {
                        domain: binding.domain,
                    });
                }
                if previous.logical_priority == binding.logical_priority {
                    return Err(ReactorPriorityError::DuplicatePriority {
                        priority: binding.logical_priority,
                        first_domain: previous.domain,
                        duplicate_domain: binding.domain,
                    });
                }
            }
        }

        for domain in self.domains() {
            if !bindings
                .iter()
                .flatten()
                .any(|binding| binding.domain == domain.id)
            {
                return Err(ReactorPriorityError::MissingDomainBinding { domain: domain.id });
            }
        }

        if binding_len > usize::from(interrupt_profile.max_nesting) {
            return Err(ReactorPriorityError::NestingExceeded {
                domains: binding_len,
                max_nesting: interrupt_profile.max_nesting,
            });
        }

        for (index, domain) in self.domains().enumerate() {
            let Some(binding) = bindings
                .iter()
                .flatten()
                .find(|binding| binding.domain == domain.id)
            else {
                return Err(ReactorPriorityError::MissingDomainBinding { domain: domain.id });
            };
            for previous_domain in self.domains().take(index) {
                let Some(previous_binding) = bindings
                    .iter()
                    .flatten()
                    .find(|candidate| candidate.domain == previous_domain.id)
                else {
                    return Err(ReactorPriorityError::MissingDomainBinding {
                        domain: previous_domain.id,
                    });
                };
                if previous_domain.priority_band == domain.priority_band {
                    return Err(ReactorPriorityError::DuplicatePriorityBand {
                        priority_band: domain.priority_band,
                        first_domain: previous_domain.id,
                        duplicate_domain: domain.id,
                    });
                }
                let previous_more_urgent = previous_domain.priority_band < domain.priority_band;
                let previous_priority_higher =
                    previous_binding.logical_priority < binding.logical_priority;
                if previous_more_urgent != previous_priority_higher {
                    let (more_urgent_domain, less_urgent_domain) = if previous_more_urgent {
                        (previous_domain.id, domain.id)
                    } else {
                        (domain.id, previous_domain.id)
                    };
                    return Err(ReactorPriorityError::PriorityOrderMismatch {
                        more_urgent_domain,
                        less_urgent_domain,
                    });
                }
            }
        }

        Ok(ReactorPriorityPlan {
            reactor: self,
            bindings,
            binding_len,
            interrupt_profile,
        })
    }
}

/// Validate async reactor domains and channel contracts before runtime wiring.
pub fn admit_reactor_domains<const D: usize, const C: usize>(
    domains: [Option<ReactorDomainContract>; D],
    channels: [Option<ReactorChannelContract>; C],
) -> Result<ReactorAdmissionPlan<D, C>, ReactorAdmissionError> {
    let mut domain_len = 0usize;
    for (index, domain) in domains.iter().flatten().enumerate() {
        domain_len = index + 1;
        if domain.task_slots == 0 || domain.task_slots > 32 {
            return Err(ReactorAdmissionError::InvalidTaskSlots {
                domain: domain.id,
                task_slots: domain.task_slots,
            });
        }
        if domain.fuel_per_cycle == 0 {
            return Err(ReactorAdmissionError::InvalidFuel { domain: domain.id });
        }
    }
    if domain_len == 0 {
        return Err(ReactorAdmissionError::EmptyDomains);
    }
    for (i, a) in domains.iter().flatten().enumerate() {
        for b in domains.iter().flatten().skip(i + 1) {
            if a.id == b.id {
                return Err(ReactorAdmissionError::DuplicateDomain(a.id));
            }
        }
    }

    let mut cross_domain_channels = [None; C];
    let mut channel_len = 0usize;
    let mut cross_domain_len = 0usize;
    for (index, channel) in channels.iter().enumerate() {
        let Some(channel) = channel else {
            continue;
        };
        channel_len += 1;
        if channel.capacity == 0 {
            return Err(ReactorAdmissionError::InvalidChannelCapacity { index });
        }
        if channel.waiter_slots == 0 || (channel.is_cross_domain() && channel.waiter_slots < 2) {
            return Err(ReactorAdmissionError::InvalidWaiterSlots { index });
        }
        for domain in [channel.from_domain, channel.to_domain] {
            if !domains.iter().flatten().any(|known| known.id == domain) {
                return Err(ReactorAdmissionError::UnknownChannelDomain { index, domain });
            }
        }
        if channel.is_cross_domain() {
            cross_domain_channels[cross_domain_len] = Some(*channel);
            cross_domain_len += 1;
        }
    }

    Ok(ReactorAdmissionPlan {
        domains,
        channels,
        cross_domain_channels,
        domain_len,
        channel_len,
        cross_domain_len,
    })
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
    fn sparse_ready_bits_preserve_unpolled_fuel_remainder() {
        static POLLS: [portable_atomic::AtomicU32; 8] =
            [const { portable_atomic::AtomicU32::new(0) }; 8];
        for polls in &POLLS {
            polls.store(0, Ordering::Relaxed);
        }

        struct Counting {
            index: usize,
        }
        impl Future for Counting {
            type Output = ();
            fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
                POLLS[self.index].fetch_add(1, Ordering::Relaxed);
                Poll::Pending
            }
        }

        let core = leak_core::<8>();
        let mut exec = ReactorExecutor::bind(core);
        let task0 = pin!(Counting { index: 0 });
        let task1 = pin!(Counting { index: 1 });
        let task2 = pin!(Counting { index: 2 });
        let task3 = pin!(Counting { index: 3 });
        let task4 = pin!(Counting { index: 4 });
        let task5 = pin!(Counting { index: 5 });
        let task6 = pin!(Counting { index: 6 });
        let task7 = pin!(Counting { index: 7 });
        exec.spawn(task0).unwrap();
        exec.spawn(task1).unwrap();
        exec.spawn(task2).unwrap();
        exec.spawn(task3).unwrap();
        exec.spawn(task4).unwrap();
        exec.spawn(task5).unwrap();
        exec.spawn(task6).unwrap();
        exec.spawn(task7).unwrap();
        assert_eq!(exec.run_ready(8).polled, 8);

        core.waker_for(7).wake_by_ref();
        core.waker_for(3).wake_by_ref();
        let first = exec.run_ready(1);
        assert_eq!(first.polled, 1);
        assert!(first.fuel_exhausted);
        assert_eq!(POLLS[3].load(Ordering::Relaxed), 2);
        assert_eq!(POLLS[7].load(Ordering::Relaxed), 1);

        let second = exec.run_ready(1);
        assert_eq!(second.polled, 1);
        assert!(!second.fuel_exhausted);
        assert_eq!(POLLS[7].load(Ordering::Relaxed), 2);
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
    fn deadline_future_completes_before_compare_and_releases_slot() {
        static QUEUE: TimerQueue<1> = TimerQueue::new();
        static OUT: AtomicU32 = AtomicU32::new(0);
        OUT.store(0, Ordering::Relaxed);
        let core = leak_core::<1>();
        let mut exec = ReactorExecutor::bind(core);

        let task = pin!(async {
            let mut work = core::future::ready(42u32);
            let result = with_deadline(&QUEUE, 10, 100, 50, Pin::new(&mut work)).await;
            OUT.store(result.unwrap(), Ordering::Relaxed);
        });
        exec.spawn(task).unwrap();
        let stats = exec.run_ready(4);
        assert_eq!(stats.completed, 1);
        assert_eq!(OUT.load(Ordering::Relaxed), 42);
        assert_eq!(QUEUE.next_deadline_us(), None);
    }

    #[test]
    fn deadline_future_reports_health_fault_when_compare_fires_first() {
        static QUEUE: TimerQueue<1> = TimerQueue::new();
        static FAULT_CODE: AtomicU32 = AtomicU32::new(0);
        FAULT_CODE.store(0, Ordering::Relaxed);
        let core = leak_core::<1>();
        let mut exec = ReactorExecutor::bind(core);

        struct Never;
        impl Future for Never {
            type Output = ();
            fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
                Poll::Pending
            }
        }

        let task = pin!(async {
            let mut work = Never;
            let fault = with_deadline(&QUEUE, 1_000, 10_000, 2_000, Pin::new(&mut work))
                .await
                .unwrap_err();
            assert_eq!(fault.kind, DeadlineFaultKind::Missed);
            assert_eq!(fault.deadline.absolute_deadline_us, 3_000);
            let health = fault.health_fault();
            assert_eq!(health.error, KernelError::DeadlineMissed);
            assert_eq!(health.context.source, FaultSource::Scheduler);
            FAULT_CODE.store(u32::from(health.context.code), Ordering::Relaxed);
        });
        exec.spawn(task).unwrap();
        assert_eq!(exec.run_ready(4).completed, 0); // registered and parked
        assert_eq!(QUEUE.next_deadline_us(), Some(3_000));
        QUEUE.advance(3_000);
        assert_eq!(exec.run_ready(4).completed, 1);
        assert_eq!(FAULT_CODE.load(Ordering::Relaxed), 1);
        assert_eq!(QUEUE.next_deadline_us(), None);
    }

    #[test]
    fn deadline_future_fails_closed_on_invalid_contract_or_no_timer_slot() {
        static EMPTY_QUEUE: TimerQueue<0> = TimerQueue::new();
        static QUEUE: TimerQueue<1> = TimerQueue::new();
        let core = leak_core::<2>();
        let mut exec = ReactorExecutor::bind(core);

        struct Never;
        impl Future for Never {
            type Output = ();
            fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
                Poll::Pending
            }
        }

        let invalid = pin!(async {
            let mut work = Never;
            let fault = with_deadline(&QUEUE, 0, 100, 101, Pin::new(&mut work))
                .await
                .unwrap_err();
            assert_eq!(
                fault.kind,
                DeadlineFaultKind::InvalidContract(DeadlineContractError::DeadlineExceedsPeriod)
            );
            assert_eq!(fault.health_fault().error, KernelError::QuotaBreach);
        });
        let no_slot = pin!(async {
            let mut work = Never;
            let fault = EMPTY_QUEUE
                .with_deadline(0, 100, 50, Pin::new(&mut work))
                .await
                .unwrap_err();
            assert_eq!(fault.kind, DeadlineFaultKind::TimerUnavailable);
            assert_eq!(fault.health_fault().error, KernelError::QuotaBreach);
        });
        exec.spawn(invalid).unwrap();
        exec.spawn(no_slot).unwrap();
        assert_eq!(exec.run_ready(4).completed, 2);
    }

    #[test]
    fn reactor_domain_admission_surfaces_cross_domain_channels() {
        let control = ReactorDomainContract::new(0, 0)
            .task_slots(4)
            .timer_slots(2)
            .fuel_per_cycle(4);
        let telemetry = ReactorDomainContract::new(1, 3)
            .task_slots(8)
            .timer_slots(4)
            .fuel_per_cycle(8);
        let plan = admit_reactor_domains::<2, 2>(
            [Some(control), Some(telemetry)],
            [
                Some(ReactorChannelContract::new(0, 1, 4).waiter_slots(4)),
                Some(ReactorChannelContract::new(1, 1, 2)),
            ],
        )
        .unwrap();

        assert_eq!(plan.domain_len, 2);
        assert_eq!(plan.channel_len, 2);
        assert_eq!(plan.cross_domain_len, 1);
        assert_eq!(plan.domain(0), Some(&control));
        assert_eq!(
            plan.cross_domain_channels().next().copied(),
            Some(ReactorChannelContract::new(0, 1, 4).waiter_slots(4))
        );
        assert_eq!(plan.cross_domain_channels().count(), 1);
    }

    #[test]
    fn reactor_domain_admission_rejects_invalid_domains_and_channels() {
        let domain = ReactorDomainContract::new(0, 0).task_slots(1);
        assert_eq!(
            admit_reactor_domains::<0, 0>([], []),
            Err(ReactorAdmissionError::EmptyDomains)
        );
        assert_eq!(
            admit_reactor_domains::<2, 0>(
                [Some(domain), Some(ReactorDomainContract::new(0, 1))],
                [],
            ),
            Err(ReactorAdmissionError::DuplicateDomain(0))
        );
        assert_eq!(
            admit_reactor_domains::<1, 0>(
                [Some(ReactorDomainContract::new(2, 0).task_slots(33))],
                [],
            ),
            Err(ReactorAdmissionError::InvalidTaskSlots {
                domain: 2,
                task_slots: 33
            })
        );
        assert_eq!(
            admit_reactor_domains::<1, 0>(
                [Some(ReactorDomainContract::new(3, 0).fuel_per_cycle(0))],
                [],
            ),
            Err(ReactorAdmissionError::InvalidFuel { domain: 3 })
        );
        assert_eq!(
            admit_reactor_domains::<1, 1>(
                [Some(domain)],
                [Some(ReactorChannelContract::new(0, 1, 1))],
            ),
            Err(ReactorAdmissionError::UnknownChannelDomain {
                index: 0,
                domain: 1
            })
        );
        assert_eq!(
            admit_reactor_domains::<1, 1>(
                [Some(domain)],
                [Some(ReactorChannelContract::new(0, 0, 0))],
            ),
            Err(ReactorAdmissionError::InvalidChannelCapacity { index: 0 })
        );
        assert_eq!(
            admit_reactor_domains::<1, 1>(
                [Some(domain)],
                [Some(ReactorChannelContract::new(0, 0, 1).waiter_slots(0))],
            ),
            Err(ReactorAdmissionError::InvalidWaiterSlots { index: 0 })
        );
        assert_eq!(
            admit_reactor_domains::<2, 1>(
                [Some(domain), Some(ReactorDomainContract::new(1, 1))],
                [Some(ReactorChannelContract::new(0, 1, 1).waiter_slots(1))],
            ),
            Err(ReactorAdmissionError::InvalidWaiterSlots { index: 0 })
        );
    }

    #[test]
    fn reactor_priorities_bind_through_the_p_isr_profile() {
        let reactor = admit_reactor_domains::<2, 1>(
            [
                Some(ReactorDomainContract::new(7, 0).task_slots(4)),
                Some(ReactorDomainContract::new(9, 3).task_slots(2)),
            ],
            [Some(ReactorChannelContract::new(7, 9, 4).waiter_slots(4))],
        )
        .unwrap();

        let mapped = reactor
            .bind_interrupt_priorities(
                [
                    Some(ReactorPriorityBinding::new(7, 2)),
                    Some(ReactorPriorityBinding::new(9, 6)),
                ],
                InterruptProfile::NRF52840_S140,
            )
            .unwrap();

        assert_eq!(mapped.binding_len, 2);
        assert_eq!(mapped.binding(7), Some(&ReactorPriorityBinding::new(7, 2)));
        assert_eq!(mapped.reactor.cross_domain_len, 1);
        assert_eq!(mapped.interrupt_profile, InterruptProfile::NRF52840_S140);
    }

    #[test]
    fn reactor_priority_mapping_rejects_unsafe_target_bindings() {
        fn plan() -> ReactorAdmissionPlan<2, 0> {
            admit_reactor_domains(
                [
                    Some(ReactorDomainContract::new(0, 0)),
                    Some(ReactorDomainContract::new(1, 3)),
                ],
                [],
            )
            .unwrap()
        }

        assert_eq!(
            plan().bind_interrupt_priorities(
                [
                    Some(ReactorPriorityBinding::new(0, 1)),
                    Some(ReactorPriorityBinding::new(1, 6)),
                ],
                InterruptProfile::NRF52840_S140,
            ),
            Err(ReactorPriorityError::ReservedPriority {
                domain: 0,
                priority: 1,
            })
        );
        assert_eq!(
            plan().bind_interrupt_priorities(
                [
                    Some(ReactorPriorityBinding::new(0, 2)),
                    Some(ReactorPriorityBinding::new(1, 8)),
                ],
                InterruptProfile::NRF52840_BARE,
            ),
            Err(ReactorPriorityError::PriorityOutOfRange {
                domain: 1,
                priority: 8,
                priority_levels: 8,
            })
        );
        assert_eq!(
            plan().bind_interrupt_priorities(
                [
                    Some(ReactorPriorityBinding::new(0, 2)),
                    Some(ReactorPriorityBinding::new(0, 3)),
                ],
                InterruptProfile::NRF52840_BARE,
            ),
            Err(ReactorPriorityError::DuplicateDomainBinding { domain: 0 })
        );
        assert_eq!(
            plan().bind_interrupt_priorities(
                [Some(ReactorPriorityBinding::new(0, 2)), None],
                InterruptProfile::NRF52840_BARE,
            ),
            Err(ReactorPriorityError::MissingDomainBinding { domain: 1 })
        );
        assert_eq!(
            plan().bind_interrupt_priorities(
                [
                    Some(ReactorPriorityBinding::new(0, 2)),
                    Some(ReactorPriorityBinding::new(1, 2)),
                ],
                InterruptProfile::NRF52840_BARE,
            ),
            Err(ReactorPriorityError::DuplicatePriority {
                priority: 2,
                first_domain: 0,
                duplicate_domain: 1,
            })
        );
        assert_eq!(
            plan().bind_interrupt_priorities(
                [
                    Some(ReactorPriorityBinding::new(0, 6)),
                    Some(ReactorPriorityBinding::new(1, 2)),
                ],
                InterruptProfile::NRF52840_BARE,
            ),
            Err(ReactorPriorityError::PriorityOrderMismatch {
                more_urgent_domain: 0,
                less_urgent_domain: 1,
            })
        );
        assert_eq!(
            plan().bind_interrupt_priorities(
                [
                    Some(ReactorPriorityBinding::new(0, 2)),
                    Some(ReactorPriorityBinding::new(3, 6)),
                ],
                InterruptProfile::NRF52840_BARE,
            ),
            Err(ReactorPriorityError::UnknownDomain { domain: 3 })
        );
        assert_eq!(
            plan().bind_interrupt_priorities(
                [
                    Some(ReactorPriorityBinding::new(0, 2)),
                    Some(ReactorPriorityBinding::new(1, 6)),
                ],
                InterruptProfile::new(0, 0, 1, 256),
            ),
            Err(ReactorPriorityError::InvalidInterruptProfile {
                priority_levels: 0,
                max_nesting: 1,
            })
        );

        let duplicate_band = admit_reactor_domains::<2, 0>(
            [
                Some(ReactorDomainContract::new(0, 4)),
                Some(ReactorDomainContract::new(1, 4)),
            ],
            [],
        )
        .unwrap();
        assert_eq!(
            duplicate_band.bind_interrupt_priorities(
                [
                    Some(ReactorPriorityBinding::new(0, 2)),
                    Some(ReactorPriorityBinding::new(1, 6)),
                ],
                InterruptProfile::NRF52840_BARE,
            ),
            Err(ReactorPriorityError::DuplicatePriorityBand {
                priority_band: 4,
                first_domain: 0,
                duplicate_domain: 1,
            })
        );

        let four_domains = admit_reactor_domains::<4, 0>(
            [
                Some(ReactorDomainContract::new(0, 0)),
                Some(ReactorDomainContract::new(1, 1)),
                Some(ReactorDomainContract::new(2, 2)),
                Some(ReactorDomainContract::new(3, 3)),
            ],
            [],
        )
        .unwrap();
        assert_eq!(
            four_domains.bind_interrupt_priorities(
                [
                    Some(ReactorPriorityBinding::new(0, 0)),
                    Some(ReactorPriorityBinding::new(1, 1)),
                    Some(ReactorPriorityBinding::new(2, 2)),
                    Some(ReactorPriorityBinding::new(3, 3)),
                ],
                InterruptProfile::new(8, 0, 3, 1_024),
            ),
            Err(ReactorPriorityError::NestingExceeded {
                domains: 4,
                max_nesting: 3,
            })
        );
    }

    #[test]
    fn reactor_priority_mapping_property_grid_preserves_urgency() {
        let allowed = [2u8, 3, 6, 7];
        let cases = if cfg!(miri) { 16 } else { 256 };
        for seed in 0..cases {
            let left_band = ((seed * 17 + 3) & 0x7f) as u8;
            let right_band = left_band.saturating_add(1);
            let lower_index = seed as usize % (allowed.len() - 1);
            let higher_index = lower_index + 1;
            let reactor = admit_reactor_domains::<2, 0>(
                [
                    Some(ReactorDomainContract::new(10, left_band)),
                    Some(ReactorDomainContract::new(11, right_band)),
                ],
                [],
            )
            .unwrap();
            let mapped = reactor
                .bind_interrupt_priorities(
                    [
                        Some(ReactorPriorityBinding::new(10, allowed[lower_index])),
                        Some(ReactorPriorityBinding::new(11, allowed[higher_index])),
                    ],
                    InterruptProfile::NRF52840_S140,
                )
                .unwrap();
            assert!(
                mapped.binding(10).unwrap().logical_priority
                    < mapped.binding(11).unwrap().logical_priority
            );

            let inverted = mapped.reactor.bind_interrupt_priorities(
                [
                    Some(ReactorPriorityBinding::new(10, allowed[higher_index])),
                    Some(ReactorPriorityBinding::new(11, allowed[lower_index])),
                ],
                InterruptProfile::NRF52840_S140,
            );
            assert_eq!(
                inverted,
                Err(ReactorPriorityError::PriorityOrderMismatch {
                    more_urgent_domain: 10,
                    less_urgent_domain: 11,
                })
            );
        }
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
    fn channel_backpressure_is_stable_over_many_messages() {
        // Stability: a capacity-1 channel forces a park/wake handoff on every
        // message. Push a thousand through under bounded per-cycle fuel and
        // require every one to arrive in order-sum, the run to terminate (no
        // fuel starvation or lost wake), and the channel to drain empty.
        const N: u32 = 1000;
        static CH: Channel<u32, 1> = Channel::new();
        static SUM: AtomicU32 = AtomicU32::new(0);
        static RECEIVED: AtomicU32 = AtomicU32::new(0);
        let core = leak_core::<2>();
        let mut exec = ReactorExecutor::bind(core);

        let producer = pin!(async {
            for i in 0..N {
                CH.send(i).await;
            }
        });
        let consumer = pin!(async {
            for _ in 0..N {
                SUM.fetch_add(CH.recv().await, Ordering::Relaxed);
                RECEIVED.fetch_add(1, Ordering::Relaxed);
            }
        });
        exec.spawn(producer).unwrap();
        exec.spawn(consumer).unwrap();

        let mut cycles = 0u32;
        while exec.live() != 0 {
            exec.run_ready(8);
            cycles += 1;
            assert!(cycles < 100_000, "must terminate; a lost wake would hang");
        }
        assert_eq!(RECEIVED.load(Ordering::Relaxed), N, "no message dropped");
        assert_eq!(SUM.load(Ordering::Relaxed), N * (N - 1) / 2, "in-order sum");
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

    #[test]
    fn kernel_compare_wakes_reactor_without_polling_or_phase_shift() {
        use crate::{
            AppGraph, ContainmentPolicy, FaultThresholds, KernelExecutor, Poll as TaskPoll,
            Runtime, SystemProfile, TaskDecl,
        };
        use nobro_power::{PowerHookError, PowerMode, PowerPlatform};

        #[derive(Default)]
        struct CompareProvider {
            armed_deadline: Option<u64>,
            armed_ready_mask: u32,
            entered: u32,
            wake_latency_us: u32,
        }

        impl PowerPlatform for CompareProvider {
            fn program_wake(&mut self, deadline_us: Option<u64>) -> Result<(), PowerHookError> {
                self.armed_deadline = deadline_us;
                Ok(())
            }

            fn program_deadline_release(
                &mut self,
                deadline_us: Option<u64>,
                ready_mask: u32,
            ) -> Result<(), PowerHookError> {
                self.armed_ready_mask = ready_mask;
                self.program_wake(deadline_us)
            }

            fn take_deadline_releases(&mut self, now_us: u64) -> u32 {
                if let Some(deadline) = self.armed_deadline.take() {
                    self.wake_latency_us = now_us.saturating_sub(deadline) as u32;
                }
                let ready = self.armed_ready_mask;
                self.armed_ready_mask = 0;
                ready
            }

            fn observed_wake_latency_us(&self) -> u32 {
                self.wake_latency_us
            }

            fn enter(&mut self, _: PowerMode) -> Result<(), PowerHookError> {
                self.entered += 1;
                Ok(())
            }

            fn suspend(&mut self, _: u16) -> Result<(), PowerHookError> {
                Ok(())
            }

            fn resume(&mut self, _: u16) -> Result<(), PowerHookError> {
                Ok(())
            }
        }

        let built = AppGraph::<1>::new()
            .task(TaskDecl::periodic("async", 100_000).reactor_domain(0))
            .unwrap()
            .build::<2>()
            .unwrap();
        let reactor_id = built.module_of("async").unwrap();
        let mut runtime = Runtime::<3, 3, 2, 1, 1, 2, 4>::admit(
            &built.manifest,
            built.startup_nodes(),
            SystemProfile::NRF52840_CORE,
            FaultThresholds::DEFAULT,
        )
        .unwrap();
        runtime.boot_to_running(0).unwrap();
        let mut kernel =
            KernelExecutor::<1, 3, 3, 2, 1, 1, 2, 4>::new(runtime, ContainmentPolicy::Cooperative);
        kernel.add_task(built.tasks[0].unwrap(), 0).unwrap();
        kernel.seal().unwrap();

        static QUEUE: TimerQueue<1> = TimerQueue::new();
        let core = leak_core::<1>();
        let mut reactor = ReactorExecutor::bind(core);
        static FINISHED: AtomicBool = AtomicBool::new(false);
        FINISHED.store(false, Ordering::Relaxed);
        let job = pin!(async {
            assert!(QUEUE.sleep_until(30).await);
            FINISHED.store(true, Ordering::Release);
        });
        reactor.spawn(job).unwrap();

        let mut provider = CompareProvider::default();
        let first = kernel
            .run_cycle_with_reactor(
                || 0,
                &mut provider,
                reactor_id,
                core,
                &QUEUE,
                |_| {
                    reactor.run_ready(4);
                    Ok(TaskPoll::Pending)
                },
            )
            .unwrap();
        assert_eq!(first.polled, Some(reactor_id));
        assert_eq!(first.async_timer_wakes, 0);
        assert_eq!(first.idle_until_us, Some(30));
        assert_eq!(provider.armed_deadline, Some(30));
        assert_eq!(provider.armed_ready_mask, 0);
        assert_eq!(provider.entered, 1);
        assert!(!FINISHED.load(Ordering::Acquire));

        let second = kernel
            .run_cycle_with_reactor(
                || 35,
                &mut provider,
                reactor_id,
                core,
                &QUEUE,
                |_| {
                    reactor.run_ready(4);
                    Ok(TaskPoll::Ready)
                },
            )
            .unwrap();
        assert_eq!(second.async_timer_wakes, 1);
        assert_eq!(second.polled, Some(reactor_id));
        assert_eq!(second.observed_wake_latency_us, 5);
        assert!(FINISHED.load(Ordering::Acquire));
        assert_eq!(reactor.live(), 0);
        assert_eq!(
            kernel.tasks().get(reactor_id).unwrap().stats.next_due_us,
            100_000,
            "event-driven poll must preserve the periodic phase"
        );

        static DEVICE_DONE: AtomicBool = AtomicBool::new(false);
        DEVICE_DONE.store(false, Ordering::Release);
        let device_job = pin!(core::future::poll_fn(|_| {
            if DEVICE_DONE.load(Ordering::Acquire) {
                core::task::Poll::Ready(())
            } else {
                core::task::Poll::Pending
            }
        }));
        let device_slot = reactor.spawn(device_job).unwrap();

        let parked = kernel
            .run_cycle_with_reactor(
                || 40,
                &mut provider,
                reactor_id,
                core,
                &QUEUE,
                |_| {
                    reactor.run_ready(4);
                    Ok(TaskPoll::Pending)
                },
            )
            .unwrap();
        assert!(parked.async_event_wake);
        assert_eq!(parked.polled, Some(reactor_id));
        assert_eq!(reactor.live(), 1);

        // Model a peripheral completion ISR: publish device state, then invoke
        // the task's ordinary waker. The next cycle must dispatch immediately,
        // although neither the 100-ms periodic release nor a timer is due.
        DEVICE_DONE.store(true, Ordering::Release);
        core.waker_for(device_slot).wake_by_ref();
        let completed = kernel
            .run_cycle_with_reactor(
                || 41,
                &mut provider,
                reactor_id,
                core,
                &QUEUE,
                |_| {
                    reactor.run_ready(4);
                    Ok(TaskPoll::Ready)
                },
            )
            .unwrap();
        assert!(completed.async_event_wake);
        assert_eq!(completed.async_timer_wakes, 0);
        assert_eq!(completed.polled, Some(reactor_id));
        assert_eq!(reactor.live(), 0);
        assert_eq!(
            kernel.tasks().get(reactor_id).unwrap().stats.next_due_us,
            100_000,
            "hardware completion must preserve the periodic phase"
        );
    }

    #[test]
    fn graph_bound_multi_domain_runtime_merges_deadlines_and_preserves_priority() {
        use crate::{
            AppGraph, ContainmentPolicy, FaultThresholds, KernelExecutor, MpmcChannel,
            Poll as TaskPoll, ReportIdentity, Runtime, SystemProfile, TaskDecl,
        };
        use core::sync::atomic::{AtomicU32, Ordering};
        use nobro_power::{PowerHookError, PowerMode, PowerPlatform};

        #[derive(Default)]
        struct HostPower {
            armed: Option<u64>,
        }

        impl PowerPlatform for HostPower {
            fn program_wake(&mut self, deadline_us: Option<u64>) -> Result<(), PowerHookError> {
                self.armed = deadline_us;
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

        static CROSS_DOMAIN: MpmcChannel<u32, 1, 2> = MpmcChannel::new();
        static RECEIVED: AtomicU32 = AtomicU32::new(0);
        RECEIVED.store(0, Ordering::Release);

        let built = AppGraph::<2>::new()
            .task(TaskDecl::control("control-reactor", 100_000).reactor_domain(0))
            .unwrap()
            .task(
                TaskDecl::service("telemetry-reactor", 100_000)
                    .reactor_domain(1)
                    .after("control-reactor"),
            )
            .unwrap()
            .build::<3>()
            .unwrap();
        let admitted = built
            .admit_reactor_domains::<2, 1>(
                [
                    Some(
                        ReactorDomainContract::new(0, 0)
                            .task_slots(1)
                            .timer_slots(1)
                            .fuel_per_cycle(1),
                    ),
                    Some(
                        ReactorDomainContract::new(1, 3)
                            .task_slots(1)
                            .timer_slots(1)
                            .fuel_per_cycle(1),
                    ),
                ],
                [Some(ReactorChannelContract::new(0, 1, 1))],
            )
            .unwrap();
        let control_module = built.module_of("control-reactor").unwrap();
        let telemetry_module = built.module_of("telemetry-reactor").unwrap();

        let control_core = leak_core::<1>();
        let telemetry_core = leak_core::<1>();
        let control_timers = Box::leak(Box::new(TimerQueue::<1>::new()));
        let telemetry_timers = Box::leak(Box::new(TimerQueue::<1>::new()));
        let runtime_set = admitted
            .bind_runtime([
                Some(ReactorRuntimeDomain::new(0, control_core, control_timers)),
                Some(ReactorRuntimeDomain::new(
                    1,
                    telemetry_core,
                    telemetry_timers,
                )),
            ])
            .unwrap();
        assert_eq!(runtime_set.len(), 2);
        assert_eq!(runtime_set.module_for_domain(0), Some(control_module));
        assert_eq!(runtime_set.fuel_for_module(control_module), Some(1));
        let unrelated_core = leak_core::<1>();
        let mut unrelated_reactor = ReactorExecutor::bind(unrelated_core);
        assert_eq!(
            runtime_set.run_domain(control_module, &mut unrelated_reactor),
            Err(ReactorRuntimeError::CoreMismatch { domain: 0 })
        );

        let mut control_reactor = ReactorExecutor::bind(control_core);
        let mut telemetry_reactor = ReactorExecutor::bind(telemetry_core);
        let control_job = pin!(async {
            assert!(control_timers.sleep_until(1_000).await);
            CROSS_DOMAIN.send(41).await.unwrap();
        });
        let telemetry_job = pin!(async {
            let value = CROSS_DOMAIN
                .recv()
                .await
                .unwrap()
                .expect("channel remained open");
            RECEIVED.store(value + 1, Ordering::Release);
        });
        control_reactor.spawn(control_job).unwrap();
        telemetry_reactor.spawn(telemetry_job).unwrap();

        let mut runtime = Runtime::<3, 3, 4, 0, 0, 3, 0>::admit(
            &built.manifest,
            built.startup_nodes(),
            SystemProfile::NRF52840_CORE,
            FaultThresholds::DEFAULT,
        )
        .unwrap();
        runtime.boot_to_running(0).unwrap();
        let mut kernel =
            KernelExecutor::<2, 3, 3, 4, 0, 0, 3, 0>::new(runtime, ContainmentPolicy::Cooperative);
        for meta in built.tasks.iter().flatten() {
            kernel.add_task(*meta, 0).unwrap();
        }
        kernel.seal().unwrap();

        let mut power = HostPower::default();
        let mut instrumentation =
            crate::ExecutorInstrumentation::<2>::with_identity(ReportIdentity::new(1, 2, 3));
        {
            let mut dispatch = |ctx: &mut crate::ModuleCtx<'_, 3, 3, 4, 0, 0, 3, 0>| {
                if ctx.module() == control_module {
                    runtime_set
                        .run_domain(control_module, &mut control_reactor)
                        .unwrap();
                    Ok(if control_reactor.live() == 0 {
                        TaskPoll::Ready
                    } else {
                        TaskPoll::Pending
                    })
                } else {
                    assert_eq!(ctx.module(), telemetry_module);
                    runtime_set
                        .run_domain(telemetry_module, &mut telemetry_reactor)
                        .unwrap();
                    Ok(if telemetry_reactor.live() == 0 {
                        TaskPoll::Ready
                    } else {
                        TaskPoll::Pending
                    })
                }
            };

            let first = kernel
                .run_cycle_with_reactors_instrumented(
                    || 0,
                    &mut power,
                    &runtime_set,
                    &mut dispatch,
                    &mut instrumentation,
                )
                .unwrap();
            assert_eq!(first.async_domains_woken, 2);
            assert_eq!(
                first.polled,
                Some(control_module),
                "the more urgent domain wins simultaneous runtime wakes"
            );

            let second = kernel
                .run_cycle_with_reactors(|| 0, &mut power, &runtime_set, &mut dispatch)
                .unwrap();
            assert_eq!(second.polled, Some(telemetry_module));
            assert_eq!(second.idle_until_us, Some(1_000));
            assert_eq!(power.armed, Some(1_000));

            let third = kernel
                .run_cycle_with_reactors(|| 1_000, &mut power, &runtime_set, &mut dispatch)
                .unwrap();
            assert_eq!(third.async_timer_wakes, 1);
            assert_eq!(third.async_domains_woken, 1);
            assert_eq!(third.polled, Some(control_module));

            let fourth = kernel
                .run_cycle_with_reactors(|| 1_000, &mut power, &runtime_set, &mut dispatch)
                .unwrap();
            assert!(fourth.async_event_wake);
            assert_eq!(fourth.async_domains_woken, 1);
            assert_eq!(fourth.polled, Some(telemetry_module));
            assert_eq!(RECEIVED.load(Ordering::Acquire), 42);
        }
        assert_eq!(control_reactor.live(), 0);
        assert_eq!(telemetry_reactor.live(), 0);
        assert_eq!(
            kernel
                .tasks()
                .get(control_module)
                .unwrap()
                .stats
                .next_due_us,
            100_000
        );
        assert_eq!(
            kernel
                .tasks()
                .get(telemetry_module)
                .unwrap()
                .stats
                .next_due_us,
            100_000
        );
    }
}
