//! RP2350 channel-0 DMA completion future.
//!
//! This provider is deliberately port-local: RP2350 uses a DMA interrupt
//! fabric, not Nordic PPI/DPPI. It arms the waker before starting hardware,
//! disables the interrupt source before cancellation, and waits for DMA abort
//! before releasing its static staging storage.

use core::cell::UnsafeCell;
use core::future::Future;
use core::marker::PhantomPinned;
use core::pin::Pin;
use core::sync::atomic::{compiler_fence, AtomicU32, Ordering};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use cortex_m::peripheral::syst::SystClkSource;
use nobro_hal::traits::HalClock;
use nobro_hal::{CompletionCell, CompletionError, StagedTransferError, StagedTransferPlan};
use rp235x_hal as hal;

use hal::dma::single_buffer::{Config, Transfer};
use hal::dma::{Channel, ReadTarget, CH0};
use hal::pac;

const CHANNEL_MASK: u32 = 1;
const NVIC_PRIORITY_LEVELS: u8 = 16;
const NVIC_PRIORITY_SHIFT: u8 = 4;
const ATOMIC_SET_ALIAS: usize = 0x2000;
const ATOMIC_CLEAR_ALIAS: usize = 0x3000;
const SCB_SCR_SEVONPEND: u32 = 1 << 4;
const DMA_TIMER0_TREQ: u8 = 59;
const SELFTEST_TRANSFER_HZ: u32 = 10_000;
const SELFTEST_TIMEOUT_US: u32 = 20_000;
const SELFTEST_WAKE_LIMIT_US: u32 = 100;
const SELFTEST_MIN_RESIDENCE_US: u32 = 1_000;

/// Maximum words in one staged RP2350 DMA copy.
pub const DMA_COPY_MAX_WORDS: usize = 64;

struct StaticWords(UnsafeCell<[u32; DMA_COPY_MAX_WORDS]>);

// DMA0_COMPLETION and the owned CH0 token serialize access. If a future is
// forgotten, both the channel and its provider borrow are leaked; no second
// safe accessor can alias these arrays.
unsafe impl Sync for StaticWords {}

static DMA_SOURCE: StaticWords = StaticWords(UnsafeCell::new([0; DMA_COPY_MAX_WORDS]));
static DMA_DESTINATION: StaticWords = StaticWords(UnsafeCell::new([0; DMA_COPY_MAX_WORDS]));
static DMA0_COMPLETION: CompletionCell = CompletionCell::new();
static DMA0_IRQ_COUNT: AtomicU32 = AtomicU32::new(0);
static DMA0_IRQ_AT_US: AtomicU32 = AtomicU32::new(0);
static SELFTEST_WAKE_COUNT: AtomicU32 = AtomicU32::new(0);
static SELFTEST_TIMED_OUT: AtomicU32 = AtomicU32::new(0);

struct PacedWords(&'static [u32]);

// SAFETY: PacedWords owns an immutable static slice for the whole transfer,
// reports its exact address/count, and advances only within that allocation.
unsafe impl ReadTarget for PacedWords {
    type ReceivedWord = u32;

    fn rx_treq() -> Option<u8> {
        Some(DMA_TIMER0_TREQ)
    }

    fn rx_address_count(&self) -> (u32, u32) {
        (self.0.as_ptr() as u32, self.0.len() as u32)
    }

    fn rx_increment(&self) -> bool {
        true
    }
}

type UnpacedTransfer = Transfer<Channel<CH0>, &'static [u32], &'static mut [u32]>;
type PacedTransfer = Transfer<Channel<CH0>, PacedWords, &'static mut [u32]>;

enum CopyTransfer {
    Unpaced(UnpacedTransfer),
    Paced(PacedTransfer),
}

impl CopyTransfer {
    fn is_done(&self) -> bool {
        match self {
            Self::Unpaced(transfer) => transfer.is_done(),
            Self::Paced(transfer) => transfer.is_done(),
        }
    }

    fn wait(self) -> (Channel<CH0>, &'static mut [u32]) {
        match self {
            Self::Unpaced(transfer) => {
                let (channel, _, destination) = transfer.wait();
                (channel, destination)
            }
            Self::Paced(transfer) => {
                let (channel, _, destination) = transfer.wait();
                (channel, destination)
            }
        }
    }

    fn abort(self) -> (Channel<CH0>, &'static mut [u32]) {
        match self {
            Self::Unpaced(transfer) => {
                let (channel, _, destination) = transfer.abort();
                (channel, destination)
            }
            Self::Paced(transfer) => {
                let (channel, _, destination) = transfer.abort();
                (channel, destination)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaCompletionPriorityError {
    InvalidLogicalPriority,
}

/// Validated RP2350 NVIC priority for the DMA completion interrupt.
///
/// RP2350 exposes four implemented priority bits. This token validates that
/// width; unlike the nRF BASEPRI profile, the current RP2350 critical-section
/// implementation masks interrupts globally, so it does not claim that a
/// deadline IRQ remains serviceable inside a critical section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaCompletionPriority {
    logical: u8,
}

impl DmaCompletionPriority {
    pub const fn new(logical: u8) -> Result<Self, DmaCompletionPriorityError> {
        if logical >= NVIC_PRIORITY_LEVELS {
            return Err(DmaCompletionPriorityError::InvalidLogicalPriority);
        }
        Ok(Self { logical })
    }

    pub const fn port_default() -> Self {
        Self { logical: 8 }
    }

    pub const fn logical(self) -> u8 {
        self.logical
    }

    const fn raw(self) -> u8 {
        self.logical << NVIC_PRIORITY_SHIFT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaCopyError {
    Busy,
    EmptyTransfer,
    LengthMismatch,
    TransferTooLong,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransferState {
    New,
    InFlight,
    Done,
}

/// Exclusive owner of RP2350 DMA channel 0 and its completion interrupt.
pub struct Dma0Completion {
    channel: Option<Channel<CH0>>,
    interrupt_priority: DmaCompletionPriority,
}

impl Dma0Completion {
    pub fn new(channel: Channel<CH0>, interrupt_priority: DmaCompletionPriority) -> Self {
        disable_channel_irq();
        clear_channel_irq();
        DMA0_COMPLETION.cancel();
        unsafe {
            let mut core = cortex_m::Peripherals::steal();
            // An IRQ that becomes pending between a completion check and WFE
            // must leave an event behind, even if the ISR returns first.
            core.SCB.scr.modify(|value| value | SCB_SCR_SEVONPEND);
            core.NVIC
                .set_priority(pac::Interrupt::DMA_IRQ_0, interrupt_priority.raw());
            cortex_m::peripheral::NVIC::unpend(pac::Interrupt::DMA_IRQ_0);
            cortex_m::peripheral::NVIC::unmask(pac::Interrupt::DMA_IRQ_0);
        }
        Self {
            channel: Some(channel),
            interrupt_priority,
        }
    }

    pub const fn interrupt_priority(&self) -> DmaCompletionPriority {
        self.interrupt_priority
    }

    /// Copy equal-length word slices through DMA channel 0.
    ///
    /// DMA reads and writes fixed static staging rather than caller memory.
    /// The output slice is updated only after IRQ completion is consumed.
    pub fn copy<'a>(&'a mut self, source: &'a [u32], destination: &'a mut [u32]) -> DmaCopy<'a> {
        self.copy_inner(source, destination, false)
    }

    fn copy_paced_for_selftest<'a>(
        &'a mut self,
        source: &'a [u32],
        destination: &'a mut [u32],
    ) -> DmaCopy<'a> {
        self.copy_inner(source, destination, true)
    }

    fn copy_inner<'a>(
        &'a mut self,
        source: &'a [u32],
        destination: &'a mut [u32],
        paced: bool,
    ) -> DmaCopy<'a> {
        let plan = StagedTransferPlan::new(source.len(), destination.len(), DMA_COPY_MAX_WORDS);
        let validation = match plan {
            Ok(_) => None,
            Err(StagedTransferError::Empty) => Some(DmaCopyError::EmptyTransfer),
            Err(StagedTransferError::LengthMismatch) => Some(DmaCopyError::LengthMismatch),
            Err(StagedTransferError::TooLong) => Some(DmaCopyError::TransferTooLong),
        };
        DmaCopy {
            provider: self,
            source: Some(source),
            destination: Some(destination),
            transfer: None,
            validation,
            words: plan.map_or(source.len(), StagedTransferPlan::words),
            paced,
            state: TransferState::New,
            _pin: PhantomPinned,
        }
    }

    /// Number of channel-0 completion IRQs observed since boot.
    pub fn irq_count(&self) -> u32 {
        DMA0_IRQ_COUNT.load(Ordering::Acquire)
    }
}

impl Drop for Dma0Completion {
    fn drop(&mut self) {
        disable_channel_irq();
        clear_channel_irq();
        DMA0_COMPLETION.cancel();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaSelfTestReport {
    pub passed: bool,
    pub cancellation_output_untouched: bool,
    pub words: usize,
    pub polls: u32,
    pub irq_wakes: u32,
    pub task_wakes: u32,
    pub idle_entries: u32,
    pub idle_residence_us: u32,
    pub completion_us: u32,
    pub wake_latency_us: u32,
}

/// Bounded boot-time check used by the RP2350 status application.
///
/// It first starts and drops a transfer before consuming completion, proving
/// that cancellation does not publish staged output. A second transfer must
/// complete through `DMA_IRQ_0`, invoke the registered waker exactly once, and
/// copy the expected pattern. DMA timer 0 deliberately paces this boot-only
/// transfer so the pending future enters System-ON WFE long enough for typed
/// residence and IRQ-to-task wake latency to be observed.
pub fn run_dma_selftest(provider: &mut Dma0Completion, system_clock_hz: u32) -> DmaSelfTestReport {
    const WORDS: usize = DMA_COPY_MAX_WORDS;

    let mut source = [0u32; WORDS];
    for (index, word) in source.iter_mut().enumerate() {
        *word = 0xA5A5_0000 ^ index as u32;
    }
    let mut destination = [0xDEAD_BEEFu32; WORDS];
    let waker = selftest_waker();
    let mut context = Context::from_waker(&waker);

    {
        let mut cancelled = core::pin::pin!(provider.copy(&source, &mut destination));
        let _ = Future::poll(cancelled.as_mut(), &mut context);
    }
    let cancellation_output_untouched = destination.iter().all(|word| *word == 0xDEAD_BEEF);
    destination.fill(0);

    let pacing_ok = configure_selftest_pacing(system_clock_hz, SELFTEST_TRANSFER_HZ);
    if !pacing_ok || !start_selftest_timeout(system_clock_hz) {
        return DmaSelfTestReport {
            passed: false,
            cancellation_output_untouched,
            words: WORDS,
            polls: 0,
            irq_wakes: 0,
            task_wakes: 0,
            idle_entries: 0,
            idle_residence_us: 0,
            completion_us: 0,
            wake_latency_us: u32::MAX,
        };
    }
    let irq_before = provider.irq_count();
    let wake_before = SELFTEST_WAKE_COUNT.load(Ordering::Acquire);
    DMA0_IRQ_AT_US.store(0, Ordering::Release);
    // Consume any stale event before arming the transfer. SEVONPEND, configured
    // by the provider, closes the remaining poll-to-WFE race.
    cortex_m::asm::sev();
    cortex_m::asm::wfe();
    cortex_m::asm::dsb();

    let mut polls = 0;
    let mut idle_entries = 0u32;
    let mut idle_residence_us = 0u32;
    let completion_started_at = crate::portable::Rp2350::now_us() as u32;
    let result = {
        let mut transfer =
            core::pin::pin!(provider.copy_paced_for_selftest(&source, &mut destination));
        loop {
            polls += 1;
            match Future::poll(transfer.as_mut(), &mut context) {
                Poll::Ready(value) => break value,
                Poll::Pending => {
                    if SELFTEST_TIMED_OUT.load(Ordering::Acquire) != 0 {
                        break Err(DmaCopyError::Busy);
                    }
                    let idle_started_at = crate::portable::Rp2350::now_us() as u32;
                    cortex_m::asm::dsb();
                    cortex_m::asm::wfe();
                    let idle_finished_at = crate::portable::Rp2350::now_us() as u32;
                    idle_entries = idle_entries.wrapping_add(1);
                    idle_residence_us = idle_residence_us
                        .wrapping_add(idle_finished_at.wrapping_sub(idle_started_at));
                }
            }
        }
    };
    stop_selftest_timeout();
    let completed_at = crate::portable::Rp2350::now_us() as u32;
    let completion_us = completed_at.wrapping_sub(completion_started_at);
    let irq_at = DMA0_IRQ_AT_US.load(Ordering::Acquire);
    let wake_latency_us = completed_at.wrapping_sub(irq_at);
    let irq_wakes = provider.irq_count().wrapping_sub(irq_before);
    let task_wakes = SELFTEST_WAKE_COUNT
        .load(Ordering::Acquire)
        .wrapping_sub(wake_before);
    let passed = cancellation_output_untouched
        && pacing_ok
        && SELFTEST_TIMED_OUT.load(Ordering::Acquire) == 0
        && result == Ok(WORDS)
        && destination == source
        && irq_wakes == 1
        && task_wakes == 1
        && idle_entries >= 1
        && idle_residence_us >= SELFTEST_MIN_RESIDENCE_US
        && irq_at != 0
        && wake_latency_us <= SELFTEST_WAKE_LIMIT_US;

    DmaSelfTestReport {
        passed,
        cancellation_output_untouched,
        words: WORDS,
        polls,
        irq_wakes,
        task_wakes,
        idle_entries,
        idle_residence_us,
        completion_us,
        wake_latency_us,
    }
}

unsafe fn selftest_waker_clone(_: *const ()) -> RawWaker {
    RawWaker::new(core::ptr::null(), &SELFTEST_WAKER_VTABLE)
}

unsafe fn selftest_waker_wake(_: *const ()) {
    SELFTEST_WAKE_COUNT.fetch_add(1, Ordering::AcqRel);
}

unsafe fn selftest_waker_drop(_: *const ()) {}

static SELFTEST_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    selftest_waker_clone,
    selftest_waker_wake,
    selftest_waker_wake,
    selftest_waker_drop,
);

fn selftest_waker() -> Waker {
    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &SELFTEST_WAKER_VTABLE)) }
}

/// Cancellation-safe DMA copy future returned by [`Dma0Completion::copy`].
///
/// The future is `!Unpin`: after its first poll, safe code cannot move and
/// forget the in-flight operation while reusing the provider. DMA itself only
/// sees permanent staging, so even an intentional capacity leak cannot expose
/// reclaimed caller memory to hardware.
pub struct DmaCopy<'a> {
    provider: &'a mut Dma0Completion,
    source: Option<&'a [u32]>,
    destination: Option<&'a mut [u32]>,
    transfer: Option<CopyTransfer>,
    validation: Option<DmaCopyError>,
    words: usize,
    paced: bool,
    state: TransferState,
    _pin: PhantomPinned,
}

impl Future for DmaCopy<'_> {
    type Output = Result<usize, DmaCopyError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: no field is moved out except through Option::take, and all
        // DMA-visible storage is static. Pinning prevents safe post-poll move
        // plus forget from making the provider available while CH0 is active.
        let this = unsafe { self.get_unchecked_mut() };
        match this.state {
            TransferState::New => {
                if let Some(error) = this.validation {
                    this.state = TransferState::Done;
                    return Poll::Ready(Err(error));
                }
                if DMA0_COMPLETION.arm(cx.waker()) == Err(CompletionError::Busy) {
                    this.state = TransferState::Done;
                    return Poll::Ready(Err(DmaCopyError::Busy));
                }
                let Some(channel) = this.provider.channel.take() else {
                    DMA0_COMPLETION.cancel();
                    this.state = TransferState::Done;
                    return Poll::Ready(Err(DmaCopyError::Busy));
                };

                clear_channel_irq();
                let source = this.source.take().expect("validated DMA source missing");
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        source.as_ptr(),
                        (*DMA_SOURCE.0.get()).as_mut_ptr(),
                        this.words,
                    );
                    core::ptr::write_bytes((*DMA_DESTINATION.0.get()).as_mut_ptr(), 0, this.words);
                }
                compiler_fence(Ordering::SeqCst);
                enable_channel_irq();

                // SAFETY: CH0 ownership plus the completion cell excludes a
                // second transfer. The references remain valid forever; the
                // transfer's active prefix is bounded by `words`.
                let staged_source = unsafe {
                    core::slice::from_raw_parts((*DMA_SOURCE.0.get()).as_ptr(), this.words)
                };
                let staged_destination = unsafe {
                    core::slice::from_raw_parts_mut(
                        (*DMA_DESTINATION.0.get()).as_mut_ptr(),
                        this.words,
                    )
                };
                this.transfer = Some(if this.paced {
                    CopyTransfer::Paced(
                        Config::new(channel, PacedWords(staged_source), staged_destination).start(),
                    )
                } else {
                    CopyTransfer::Unpaced(
                        Config::new(channel, staged_source, staged_destination).start(),
                    )
                });
                this.state = TransferState::InFlight;
                Poll::Pending
            }
            TransferState::InFlight => {
                if !DMA0_COMPLETION.poll_complete(cx) {
                    return Poll::Pending;
                }
                disable_channel_irq();
                clear_channel_irq();
                let transfer = this
                    .transfer
                    .take()
                    .expect("in-flight DMA transfer missing");
                assert!(
                    transfer.is_done(),
                    "DMA IRQ published before channel completion"
                );
                let (channel, staged_destination) = transfer.wait();
                this.destination
                    .take()
                    .expect("DMA destination missing")
                    .copy_from_slice(&staged_destination[..this.words]);
                this.provider.channel = Some(channel);
                this.state = TransferState::Done;
                Poll::Ready(Ok(this.words))
            }
            TransferState::Done => panic!("DmaCopy polled after completion"),
        }
    }
}

impl Drop for DmaCopy<'_> {
    fn drop(&mut self) {
        if self.state != TransferState::InFlight {
            return;
        }

        // Stop delivery before cancelling the waker. The hardware abort then
        // waits until channel BUSY clears, so static staging is no longer
        // hardware-owned before the channel is returned to the provider.
        disable_channel_irq();
        DMA0_COMPLETION.cancel();
        if let Some(transfer) = self.transfer.take() {
            let (channel, _) = transfer.abort();
            self.provider.channel = Some(channel);
        }
        clear_channel_irq();
        self.state = TransferState::Done;
    }
}

#[inline]
fn dma() -> &'static pac::dma::RegisterBlock {
    unsafe { &*pac::DMA::ptr() }
}

#[inline]
fn atomic_alias(register: *mut u32, alias_offset: usize) -> *mut u32 {
    (register as usize + alias_offset) as *mut u32
}

fn enable_channel_irq() {
    unsafe {
        core::ptr::write_volatile(
            atomic_alias(dma().inte0().as_ptr(), ATOMIC_SET_ALIAS),
            CHANNEL_MASK,
        );
    }
}

fn disable_channel_irq() {
    unsafe {
        core::ptr::write_volatile(
            atomic_alias(dma().inte0().as_ptr(), ATOMIC_CLEAR_ALIAS),
            CHANNEL_MASK,
        );
    }
}

fn clear_channel_irq() {
    dma().ints0().write(|w| unsafe { w.bits(CHANNEL_MASK) });
}

fn configure_selftest_pacing(system_hz: u32, transfer_hz: u32) -> bool {
    if transfer_hz == 0 || transfer_hz > system_hz {
        return false;
    }
    let divisor = gcd(system_hz, transfer_hz);
    let x = transfer_hz / divisor;
    let y = system_hz / divisor;
    if x == 0 || y == 0 || x > u32::from(u16::MAX) || y > u32::from(u16::MAX) {
        return false;
    }
    dma().timer0().write(|w| unsafe {
        w.x().bits(x as u16);
        w.y().bits(y as u16);
        w
    });
    let configured = dma().timer0().read();
    u32::from(configured.x().bits()) == x && u32::from(configured.y().bits()) == y
}

fn start_selftest_timeout(system_hz: u32) -> bool {
    let ticks = u64::from(system_hz).saturating_mul(u64::from(SELFTEST_TIMEOUT_US)) / 1_000_000;
    if ticks == 0 || ticks > 0x0100_0000 {
        return false;
    }
    SELFTEST_TIMED_OUT.store(0, Ordering::Release);
    let mut core = unsafe { cortex_m::Peripherals::steal() };
    core.SYST.disable_counter();
    core.SYST.disable_interrupt();
    core.SYST.set_clock_source(SystClkSource::Core);
    core.SYST.set_reload(ticks as u32 - 1);
    core.SYST.clear_current();
    core.SYST.enable_interrupt();
    core.SYST.enable_counter();
    true
}

fn stop_selftest_timeout() {
    let mut core = unsafe { cortex_m::Peripherals::steal() };
    core.SYST.disable_counter();
    core.SYST.disable_interrupt();
}

fn gcd(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

#[cortex_m_rt::exception]
fn SysTick() {
    SELFTEST_TIMED_OUT.store(1, Ordering::Release);
}

#[no_mangle]
#[allow(non_snake_case)]
unsafe extern "C" fn DMA_IRQ_0() {
    if dma().ints0().read().bits() & CHANNEL_MASK == 0 {
        return;
    }
    disable_channel_irq();
    clear_channel_irq();
    compiler_fence(Ordering::SeqCst);
    DMA0_IRQ_AT_US.store(crate::portable::Rp2350::now_us() as u32, Ordering::Release);
    DMA0_IRQ_COUNT.fetch_add(1, Ordering::AcqRel);
    DMA0_COMPLETION.complete_from_isr();
}
