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

use nobro_hal::{
    CompletionCell, CompletionError, StagedTransferError, StagedTransferPlan,
};
use rp235x_hal as hal;

use hal::dma::single_buffer::{Config, Transfer};
use hal::dma::{Channel, CH0};
use hal::pac;

const CHANNEL_MASK: u32 = 1;
const NVIC_PRIORITY_LEVELS: u8 = 16;
const NVIC_PRIORITY_SHIFT: u8 = 4;
const ATOMIC_SET_ALIAS: usize = 0x2000;
const ATOMIC_CLEAR_ALIAS: usize = 0x3000;

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
static SELFTEST_WAKE_COUNT: AtomicU32 = AtomicU32::new(0);

type CopyTransfer = Transfer<Channel<CH0>, &'static [u32], &'static mut [u32]>;

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
        let plan =
            StagedTransferPlan::new(source.len(), destination.len(), DMA_COPY_MAX_WORDS);
        let validation = match plan {
            Ok(_) => None,
            Err(StagedTransferError::Empty) => Some(DmaCopyError::EmptyTransfer),
            Err(StagedTransferError::LengthMismatch) => {
                Some(DmaCopyError::LengthMismatch)
            }
            Err(StagedTransferError::TooLong) => Some(DmaCopyError::TransferTooLong),
        };
        DmaCopy {
            provider: self,
            source: Some(source),
            destination: Some(destination),
            transfer: None,
            validation,
            words: plan.map_or(source.len(), StagedTransferPlan::words),
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
}

/// Bounded boot-time check used by the RP2350 status application.
///
/// It first starts and drops a transfer before consuming completion, proving
/// that cancellation does not publish staged output. A second transfer must
/// complete through `DMA_IRQ_0`, invoke the registered waker exactly once, and
/// copy the expected pattern. This check actively polls, so it is functional
/// evidence rather than idle-residence evidence.
pub fn run_dma_selftest(provider: &mut Dma0Completion) -> DmaSelfTestReport {
    const WORDS: usize = DMA_COPY_MAX_WORDS;
    const POLL_LIMIT: u32 = 100_000;

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

    let irq_before = provider.irq_count();
    let wake_before = SELFTEST_WAKE_COUNT.load(Ordering::Acquire);
    let mut polls = 0;
    let mut result = Err(DmaCopyError::Busy);
    {
        let mut transfer = core::pin::pin!(provider.copy(&source, &mut destination));
        while polls < POLL_LIMIT {
            polls += 1;
            match Future::poll(transfer.as_mut(), &mut context) {
                Poll::Ready(value) => {
                    result = value;
                    break;
                }
                Poll::Pending => core::hint::spin_loop(),
            }
        }
    }
    let irq_wakes = provider.irq_count().wrapping_sub(irq_before);
    let task_wakes = SELFTEST_WAKE_COUNT
        .load(Ordering::Acquire)
        .wrapping_sub(wake_before);
    let passed = cancellation_output_untouched
        && result == Ok(WORDS)
        && destination == source
        && irq_wakes == 1
        && task_wakes == 1;

    DmaSelfTestReport {
        passed,
        cancellation_output_untouched,
        words: WORDS,
        polls,
        irq_wakes,
        task_wakes,
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
                this.transfer =
                    Some(Config::new(channel, staged_source, staged_destination).start());
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
                let (channel, _, staged_destination) = transfer.wait();
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
            let (channel, _, _) = transfer.abort();
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

#[no_mangle]
#[allow(non_snake_case)]
unsafe extern "C" fn DMA_IRQ_0() {
    if dma().ints0().read().bits() & CHANNEL_MASK == 0 {
        return;
    }
    disable_channel_irq();
    clear_channel_irq();
    compiler_fence(Ordering::SeqCst);
    DMA0_IRQ_COUNT.fetch_add(1, Ordering::AcqRel);
    DMA0_COMPLETION.complete_from_isr();
}
