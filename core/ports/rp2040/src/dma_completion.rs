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
use core::sync::atomic::{compiler_fence, AtomicU32, Ordering};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use nobro_hal::{
    CompletionCell, CompletionError, DmaBufferDescriptor, DmaCoherency, DmaDirection,
    DmaLeaseBackend, DmaLeaseError, DmaLeaseRegistry, DmaLeaseRequest, DmaOwnerId,
    DmaRecoveryReason, StagedTransferError, StagedTransferPlan,
};
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
static DMA0_IRQ_COUNT: AtomicU32 = AtomicU32::new(0);
static SELFTEST_WAKE_COUNT: AtomicU32 = AtomicU32::new(0);

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaSelfTestReport {
    pub passed: bool,
    pub cancellation_output_untouched: bool,
    pub words: usize,
    pub polls: u32,
    pub irq_wakes: u32,
    pub task_wakes: u32,
    pub ownership_fault_rejected: bool,
    pub stale_generation_rejected: bool,
    pub partial_completion: bool,
    pub timeout_recovered: bool,
}

struct Rp2040DmaLeaseBackend;

impl DmaLeaseBackend for Rp2040DmaLeaseBackend {
    type Error = core::convert::Infallible;

    fn prepare(&mut self, _: DmaBufferDescriptor) -> Result<(), Self::Error> {
        compiler_fence(Ordering::Release);
        Ok(())
    }

    fn complete(&mut self, _: DmaBufferDescriptor, _: usize) -> Result<(), Self::Error> {
        compiler_fence(Ordering::Acquire);
        Ok(())
    }

    fn cancel(&mut self, _: DmaBufferDescriptor) -> Result<(), Self::Error> {
        disable_channel_irq();
        DMA0_COMPLETION.cancel();
        clear_channel_irq();
        compiler_fence(Ordering::Acquire);
        Ok(())
    }

    fn reset(&mut self, _: u16, _: u16) -> Result<(), Self::Error> {
        disable_channel_irq();
        DMA0_COMPLETION.cancel();
        clear_channel_irq();
        Ok(())
    }
}

/// Run a bounded, live DMA completion and cancellation check.
///
/// The first transfer is polled once and cancelled; its caller output must
/// remain untouched. The second must complete through `DMA_IRQ_0`, wake the
/// registered task once, and publish the exact copied pattern. The poll bound
/// prevents a bad interrupt route from wedging USB enumeration at boot.
pub fn run_dma_selftest(provider: &mut Dma0Completion) -> DmaSelfTestReport {
    const WORDS: usize = DMA_COPY_MAX_WORDS / 2;
    const POLL_LIMIT: u32 = 1_000_000;

    let owner = DmaOwnerId(1);
    let request = |direction| DmaLeaseRequest {
        alignment: core::mem::align_of::<u32>(),
        direction,
        coherency: DmaCoherency::Uncached,
        peripheral: 0,
        channel: 0,
    };
    let source_address = DMA_SOURCE.0.get() as *mut u32 as usize;
    let destination_address = DMA_DESTINATION.0.get() as *mut u32 as usize;
    let region_bytes = DMA_COPY_MAX_WORDS * core::mem::size_of::<u32>();
    let mut registry = DmaLeaseRegistry::<2>::new();
    let first_source = unsafe {
        registry
            .acquire_region(
                owner,
                source_address,
                region_bytes,
                request(DmaDirection::MemoryToPeripheral),
            )
            .unwrap()
    };
    let first_destination = unsafe {
        registry
            .acquire_region(
                owner,
                destination_address,
                region_bytes,
                request(DmaDirection::PeripheralToMemory),
            )
            .unwrap()
    };
    let ownership_fault_rejected = matches!(
        registry.descriptor::<core::convert::Infallible>(first_source, DmaOwnerId(2)),
        Err(DmaLeaseError::WrongOwner)
    );
    let mut lease_backend = Rp2040DmaLeaseBackend;
    registry
        .begin(first_source, owner, &mut lease_backend)
        .unwrap();
    registry
        .begin(first_destination, owner, &mut lease_backend)
        .unwrap();

    let mut source = [0u32; WORDS];
    for (index, word) in source.iter_mut().enumerate() {
        *word = 0x5245_5000 ^ index as u32;
    }
    let mut destination = [0xDEAD_BEEFu32; WORDS];
    let waker = selftest_waker();
    let mut context = Context::from_waker(&waker);
    let priority_ok = DmaCompletionPriority::new(provider.interrupt_priority().logical())
        == Ok(provider.interrupt_priority());

    {
        let mut cancelled = core::pin::pin!(provider.copy(&source, &mut destination));
        let _ = Future::poll(cancelled.as_mut(), &mut context);
    }
    let cancellation_output_untouched = destination.iter().all(|word| *word == 0xDEAD_BEEF);
    let first_recovery = registry
        .recover(
            first_source,
            owner,
            DmaRecoveryReason::Timeout,
            &mut lease_backend,
        )
        .unwrap();
    let second_recovery = registry
        .recover(
            first_destination,
            owner,
            DmaRecoveryReason::Timeout,
            &mut lease_backend,
        )
        .unwrap();
    let timeout_recovered =
        first_recovery.peripheral_reset && second_recovery.peripheral_reset && registry.is_empty();
    let stale_generation_rejected = matches!(
        registry.descriptor::<core::convert::Infallible>(first_source, owner),
        Err(DmaLeaseError::InvalidHandle)
    );
    let source_lease = unsafe {
        registry
            .acquire_region(
                owner,
                source_address,
                region_bytes,
                request(DmaDirection::MemoryToPeripheral),
            )
            .unwrap()
    };
    let destination_lease = unsafe {
        registry
            .acquire_region(
                owner,
                destination_address,
                region_bytes,
                request(DmaDirection::PeripheralToMemory),
            )
            .unwrap()
    };
    registry
        .begin(source_lease, owner, &mut lease_backend)
        .unwrap();
    registry
        .begin(destination_lease, owner, &mut lease_backend)
        .unwrap();
    destination.fill(0);

    let irq_before = DMA0_IRQ_COUNT.load(Ordering::Acquire);
    let wake_before = SELFTEST_WAKE_COUNT.load(Ordering::Acquire);
    let mut polls = 0u32;
    let result = {
        let mut transfer = core::pin::pin!(provider.copy(&source, &mut destination));
        loop {
            polls = polls.saturating_add(1);
            match Future::poll(transfer.as_mut(), &mut context) {
                Poll::Ready(value) => break value,
                Poll::Pending if polls < POLL_LIMIT => core::hint::spin_loop(),
                Poll::Pending => break Err(DmaCopyError::Busy),
            }
        }
    };
    let irq_wakes = DMA0_IRQ_COUNT
        .load(Ordering::Acquire)
        .wrapping_sub(irq_before);
    let task_wakes = SELFTEST_WAKE_COUNT
        .load(Ordering::Acquire)
        .wrapping_sub(wake_before);
    let transferred_bytes = WORDS * core::mem::size_of::<u32>();
    let partial_completion = if result == Ok(WORDS) {
        let source_completion = registry
            .complete(source_lease, owner, transferred_bytes, &mut lease_backend)
            .unwrap();
        let destination_completion = registry
            .complete(
                destination_lease,
                owner,
                transferred_bytes,
                &mut lease_backend,
            )
            .unwrap();
        source_completion.partial && destination_completion.partial && registry.is_empty()
    } else {
        let _ = registry.recover(
            source_lease,
            owner,
            DmaRecoveryReason::PeripheralFault,
            &mut lease_backend,
        );
        let _ = registry.recover(
            destination_lease,
            owner,
            DmaRecoveryReason::PeripheralFault,
            &mut lease_backend,
        );
        false
    };
    let passed = priority_ok
        && cancellation_output_untouched
        && result == Ok(WORDS)
        && destination == source
        && irq_wakes == 1
        && task_wakes == 1
        && ownership_fault_rejected
        && stale_generation_rejected
        && partial_completion
        && timeout_recovered;

    DmaSelfTestReport {
        passed,
        cancellation_output_untouched,
        words: WORDS,
        polls,
        irq_wakes,
        task_wakes,
        ownership_fault_rejected,
        stale_generation_rejected,
        partial_completion,
        timeout_recovered,
    }
}

unsafe fn selftest_waker_clone(_: *const ()) -> RawWaker {
    RawWaker::new(core::ptr::null(), &SELFTEST_WAKER_VTABLE)
}

unsafe fn selftest_waker_wake(_: *const ()) {
    let next = SELFTEST_WAKE_COUNT.load(Ordering::Relaxed).wrapping_add(1);
    SELFTEST_WAKE_COUNT.store(next, Ordering::Release);
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
    let next = DMA0_IRQ_COUNT.load(Ordering::Relaxed).wrapping_add(1);
    DMA0_IRQ_COUNT.store(next, Ordering::Release);
    DMA0_COMPLETION.complete_from_isr();
}
