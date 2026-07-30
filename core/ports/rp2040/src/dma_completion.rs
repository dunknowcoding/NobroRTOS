//! RP2040 DMA channel-0 completion future.
//!
//! This is the RP2040 counterpart of the RP2350 provider. It deliberately
//! keeps the Cortex-M0+ interrupt-priority width and the 12-channel DMA engine
//! local instead of flattening those silicon differences into `nobro_hal`.
//! Caller buffers are copied through fixed staging so cancellation never
//! leaves DMA accessing reclaimed task memory.

use core::cell::UnsafeCell;
use core::future::Future;
use core::marker::PhantomPinned;
use core::pin::Pin;
use core::sync::atomic::{compiler_fence, Ordering};
use core::task::{Context, Poll};

use nobro_hal::{CompletionCell, CompletionError, StagedTransferError, StagedTransferPlan};
use rp2040_hal as hal;

use hal::dma::single_buffer::{Config, Transfer};
use hal::dma::{Channel, CH0};
use hal::pac;

const CHANNEL_MASK: u32 = 1;
const NVIC_PRIORITY_LEVELS: u8 = 4;
const NVIC_PRIORITY_SHIFT: u8 = 6;
const ATOMIC_SET_ALIAS: usize = 0x2000;
const ATOMIC_CLEAR_ALIAS: usize = 0x3000;
const SCB_SCR_SEVONPEND: u32 = 1 << 4;

/// Maximum words in one staged RP2040 DMA copy.
pub const DMA_COPY_MAX_WORDS: usize = 64;

struct StaticWords(UnsafeCell<[u32; DMA_COPY_MAX_WORDS]>);

// The completion cell and owned CH0 token serialize access. Forgetting an
// in-flight future leaks that ownership; safe code cannot create an alias.
unsafe impl Sync for StaticWords {}

static DMA_SOURCE: StaticWords = StaticWords(UnsafeCell::new([0; DMA_COPY_MAX_WORDS]));
static DMA_DESTINATION: StaticWords = StaticWords(UnsafeCell::new([0; DMA_COPY_MAX_WORDS]));
static DMA0_COMPLETION: CompletionCell = CompletionCell::new();

type CopyTransfer = Transfer<Channel<CH0>, &'static [u32], &'static mut [u32]>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaCompletionPriorityError {
    InvalidLogicalPriority,
}

/// Validated RP2040 DMA IRQ priority.
///
/// Cortex-M0+ implements two priority bits on RP2040, unlike the four bits
/// available to the RP2350 Cortex-M33 port.
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
        Self { logical: 2 }
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

/// Exclusive owner of RP2040 DMA channel 0 and `DMA_IRQ_0`.
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
            // If the ISR runs between a completion poll and WFE, the pending
            // transition still leaves an event for the sleeping executor.
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
    pub fn copy<'a>(&'a mut self, source: &'a [u32], destination: &'a mut [u32]) -> DmaCopy<'a> {
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
            state: TransferState::New,
            _pin: PhantomPinned,
        }
    }
}

/// Exercise the non-mutating construction/cancellation boundary at boot.
///
/// The returned future is deliberately dropped before its first poll, so this
/// validates staging limits and provider ownership without starting hardware.
pub fn validate_contract(provider: &mut Dma0Completion) -> bool {
    let source = [0x4e4f_4252];
    let mut destination = [0u32];
    let transfer = provider.copy(&source, &mut destination);
    drop(transfer);
    DmaCompletionPriority::new(provider.interrupt_priority().logical())
        == Ok(provider.interrupt_priority())
        && destination == [0]
}

impl Drop for Dma0Completion {
    fn drop(&mut self) {
        disable_channel_irq();
        clear_channel_irq();
        DMA0_COMPLETION.cancel();
    }
}

/// Cancellation-safe DMA copy future returned by [`Dma0Completion::copy`].
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
        // SAFETY: no pinned field is moved. DMA-visible storage is static and
        // the provider remains exclusively borrowed until completion/drop.
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

                // SAFETY: CH0 ownership and the completion cell exclude a
                // second transfer; the active prefixes are bounded by words.
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
    DMA0_COMPLETION.complete_from_isr();
}
