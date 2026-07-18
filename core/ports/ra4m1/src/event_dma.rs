//! RA4M1 event-paced DMAC future.
//!
//! The default operation is intentionally one call: GPT0 emits a hardware event
//! every 100 us, ICU/ELC routes each event into DMAC0, and only the final DMAC
//! completion wakes the future. Caller buffers are never exposed to DMA.

use nobro_hal::{StagedTransferError, StagedTransferPlan};

pub const EVENT_DMA_MAX_WORDS: usize = 64;
pub const EVENT_DMA_DEFAULT_PERIOD_US: u32 = 100;
pub const EVENT_DMA_MIN_PERIOD_US: u32 = 10;
pub const EVENT_DMA_MAX_PERIOD_US: u32 = 200;
const PCLKD_CYCLES_PER_US: u32 = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventDmaError {
    Busy,
    EmptyTransfer,
    LengthMismatch,
    TransferTooLong,
    PeriodOutOfRange,
    InterruptsMasked,
    ResourceBusy,
    Timeout,
    CompletionFault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventDmaPlan {
    words: usize,
    period_ticks: u32,
    timeout_ticks: u32,
}

impl EventDmaPlan {
    pub const fn new(
        source_words: usize,
        destination_words: usize,
        period_us: u32,
    ) -> Result<Self, EventDmaError> {
        let staged =
            match StagedTransferPlan::new(source_words, destination_words, EVENT_DMA_MAX_WORDS) {
                Ok(plan) => plan,
                Err(StagedTransferError::Empty) => return Err(EventDmaError::EmptyTransfer),
                Err(StagedTransferError::LengthMismatch) => {
                    return Err(EventDmaError::LengthMismatch);
                }
                Err(StagedTransferError::TooLong) => {
                    return Err(EventDmaError::TransferTooLong);
                }
            };
        if period_us < EVENT_DMA_MIN_PERIOD_US || period_us > EVENT_DMA_MAX_PERIOD_US {
            return Err(EventDmaError::PeriodOutOfRange);
        }
        let period_ticks = period_us * PCLKD_CYCLES_PER_US;
        let transfer_us = staged.words() as u32 * period_us;
        let timeout_us = transfer_us + transfer_us / 2 + 2_000;
        Ok(Self {
            words: staged.words(),
            period_ticks,
            timeout_ticks: timeout_us * PCLKD_CYCLES_PER_US,
        })
    }

    pub const fn words(self) -> usize {
        self.words
    }

    pub const fn period_ticks(self) -> u32 {
        self.period_ticks
    }

    pub const fn timeout_ticks(self) -> u32 {
        self.timeout_ticks
    }
}

#[cfg(all(target_arch = "arm", feature = "event-dma"))]
mod hardware {
    use core::cell::UnsafeCell;
    use core::future::Future;
    use core::marker::PhantomPinned;
    use core::pin::Pin;
    #[cfg(feature = "event-dma-selftest")]
    use core::sync::atomic::AtomicU32;
    use core::sync::atomic::{compiler_fence, AtomicBool, Ordering};
    use core::task::{Context, Poll};
    #[cfg(feature = "event-dma-selftest")]
    use core::task::{RawWaker, RawWakerVTable, Waker};

    use nobro_hal::{CompletionCell, CompletionError};

    #[cfg(feature = "event-dma-selftest")]
    use super::PCLKD_CYCLES_PER_US;
    use super::{EventDmaError, EventDmaPlan, EVENT_DMA_DEFAULT_PERIOD_US, EVENT_DMA_MAX_WORDS};

    const PRCR: usize = 0x4001_E3FE;
    const MSTPCRA: usize = 0x4001_E01C;
    const MSTPCRD: usize = 0x4004_7008;
    const DMAC_MSTP: u32 = 1 << 22;
    const GPT01_MSTP: u32 = 1 << 5;

    const DMA: usize = 0x4000_5200;
    const DMAST: usize = 0x00;
    const DMAC0: usize = 0x4000_5000;
    const DMSAR: usize = 0x00;
    const DMDAR: usize = 0x04;
    const DMCRA: usize = 0x08;
    const DMCRB: usize = 0x0C;
    const DMTMD: usize = 0x10;
    const DMINT: usize = 0x13;
    const DMAMD: usize = 0x14;
    const DMOFR: usize = 0x18;
    const DMCNT: usize = 0x1C;
    const DMREQ: usize = 0x1D;
    const DMSTS: usize = 0x1E;
    const DMSTS_ACTIVE: u8 = 1 << 7;
    const DMTMD_EVENT_WORD_NORMAL: u16 = (2 << 12) | (2 << 8) | 1;
    const DMAMD_INCREMENT_BOTH: u16 = (2 << 14) | (2 << 6);
    const DMINT_TRANSFER_END: u8 = 1 << 4;

    const ICU: usize = 0x4000_6000;
    const DELSR0: usize = ICU + 0x280;
    const IELSR_BASE: usize = ICU + 0x300;
    const IELSR_IR: u32 = 1 << 16;
    const ELC_EVENT_DMAC0_INT: u32 = 17;
    const ELC_EVENT_GPT0_OVERFLOW: u32 = 93;
    const ELC_EVENT_GPT1_OVERFLOW: u32 = 101;

    const GPT0: usize = 0x4007_8000;
    const GPT1: usize = 0x4007_8100;
    const GTWP: usize = 0x00;
    const GTSTR: usize = 0x04;
    const GTSTP: usize = 0x08;
    const GTCLR: usize = 0x0C;
    const GTSSR: usize = 0x10;
    const GTPSR: usize = 0x14;
    const GTCSR: usize = 0x18;
    const GTUPSR: usize = 0x1C;
    const GTDNSR: usize = 0x20;
    const GTICASR: usize = 0x24;
    const GTICBSR: usize = 0x28;
    const GTCR: usize = 0x2C;
    const GTUDDTYC: usize = 0x30;
    const GTIOR: usize = 0x34;
    const GTINTAD: usize = 0x38;
    const GTBER: usize = 0x40;
    const GTCNT: usize = 0x48;
    const GTPR: usize = 0x64;
    const GTPBR: usize = 0x68;
    const GPT_GROUP_SOFTWARE: u32 = 1 << 31;
    const GPT_WRITE_UNLOCKED: u32 = 0xA500;
    const GPT_WRITE_PROTECTED: u32 = 0xA501;

    pub const EVENT_DMA_IRQ: usize = 31;
    pub const EVENT_DMA_TIMEOUT_IRQ: usize = 30;
    const DMA_IRQ_MASK: u32 = 1 << EVENT_DMA_IRQ;
    const TIMEOUT_IRQ_MASK: u32 = 1 << EVENT_DMA_TIMEOUT_IRQ;
    const NVIC_ISER: usize = 0xE000_E100;
    const NVIC_ICER: usize = 0xE000_E180;
    const NVIC_ISPR: usize = 0xE000_E200;
    const NVIC_ICPR: usize = 0xE000_E280;
    const NVIC_IPR: usize = 0xE000_E400;
    const SCB_SCR: usize = 0xE000_ED10;
    const SCR_SEVONPEND: u32 = 1 << 4;

    const DMA_PRIORITY_RAW: u8 = 0x80;
    const TIMEOUT_PRIORITY_RAW: u8 = 0xF0;
    const ABORT_POLLS: usize = 100_000;

    struct StaticWords(UnsafeCell<[u32; EVENT_DMA_MAX_WORDS]>);

    // Access is serialized by the provider lease and CompletionCell. A forgotten
    // future leaks that lease, so safe code cannot alias these arrays.
    unsafe impl Sync for StaticWords {}

    static SOURCE: StaticWords = StaticWords(UnsafeCell::new([0; EVENT_DMA_MAX_WORDS]));
    static DESTINATION: StaticWords = StaticWords(UnsafeCell::new([0; EVENT_DMA_MAX_WORDS]));
    static COMPLETION: CompletionCell = CompletionCell::new();
    static TAKEN: AtomicBool = AtomicBool::new(false);
    #[cfg(feature = "event-dma-selftest")]
    static DMA_IRQ_COUNT: AtomicU32 = AtomicU32::new(0);
    #[cfg(feature = "event-dma-selftest")]
    static TIMEOUT_IRQ_COUNT: AtomicU32 = AtomicU32::new(0);
    #[cfg(feature = "event-dma-selftest")]
    static IRQ_AT_TICK: AtomicU32 = AtomicU32::new(0);
    static TIMED_OUT: AtomicBool = AtomicBool::new(false);
    #[cfg(feature = "event-dma-selftest")]
    static SELFTEST_WAKE_COUNT: AtomicU32 = AtomicU32::new(0);

    #[derive(Clone, Copy, PartialEq, Eq)]
    struct GptSnapshot {
        gtwp: u32,
        gtssr: u32,
        gtpsr: u32,
        gtcsr: u32,
        gtupsr: u32,
        gtdnsr: u32,
        gticasr: u32,
        gticbsr: u32,
        gtcr: u32,
        gtuddtyc: u32,
        gtior: u32,
        gtintad: u32,
        gtber: u32,
        gtcnt: u32,
        gtpr: u32,
        gtpbr: u32,
    }

    impl GptSnapshot {
        unsafe fn capture(base: usize) -> Self {
            Self {
                gtwp: read32(base + GTWP),
                gtssr: read32(base + GTSSR),
                gtpsr: read32(base + GTPSR),
                gtcsr: read32(base + GTCSR),
                gtupsr: read32(base + GTUPSR),
                gtdnsr: read32(base + GTDNSR),
                gticasr: read32(base + GTICASR),
                gticbsr: read32(base + GTICBSR),
                gtcr: read32(base + GTCR),
                gtuddtyc: read32(base + GTUDDTYC),
                gtior: read32(base + GTIOR),
                gtintad: read32(base + GTINTAD),
                gtber: read32(base + GTBER),
                gtcnt: read32(base + GTCNT),
                gtpr: read32(base + GTPR),
                gtpbr: read32(base + GTPBR),
            }
        }

        unsafe fn restore(self, base: usize, channel_mask: u32) {
            write32(base + GTSTP, channel_mask);
            write32(base + GTWP, GPT_WRITE_UNLOCKED);
            write32(base + GTCR, 0);
            write32(base + GTSSR, self.gtssr);
            write32(base + GTPSR, self.gtpsr);
            write32(base + GTCSR, self.gtcsr);
            write32(base + GTUPSR, self.gtupsr);
            write32(base + GTDNSR, self.gtdnsr);
            write32(base + GTICASR, self.gticasr);
            write32(base + GTICBSR, self.gticbsr);
            write32(base + GTUDDTYC, self.gtuddtyc);
            write32(base + GTIOR, self.gtior);
            write32(base + GTINTAD, self.gtintad);
            write32(base + GTBER, self.gtber);
            write32(base + GTPR, self.gtpr);
            write32(base + GTPBR, self.gtpbr);
            write32(base + GTCNT, self.gtcnt);
            write32(base + GTCR, self.gtcr & !1);
            // PRKEY is write-only: a GTWP read preserves the protection bits
            // but reads the key field as zero. Reapply the mandatory A5 key
            // exactly as Renesas FSP does when restoring a saved GTWP value.
            write32(base + GTWP, self.gtwp | GPT_WRITE_UNLOCKED);
            if self.gtcr & 1 != 0 {
                write32(base + GTSTR, channel_mask);
            }
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    struct DmacSnapshot {
        dmsar: u32,
        dmdar: u32,
        dmcra: u32,
        dmcrb: u32,
        dmtmd: u16,
        dmint: u8,
        dmamd: u16,
        dmofr: u32,
        dmcnt: u8,
        dmreq: u8,
    }

    impl DmacSnapshot {
        unsafe fn capture() -> Self {
            Self {
                dmsar: read32(DMAC0 + DMSAR),
                dmdar: read32(DMAC0 + DMDAR),
                dmcra: read32(DMAC0 + DMCRA),
                dmcrb: read32(DMAC0 + DMCRB),
                dmtmd: read16(DMAC0 + DMTMD),
                dmint: read8(DMAC0 + DMINT),
                dmamd: read16(DMAC0 + DMAMD),
                dmofr: read32(DMAC0 + DMOFR),
                dmcnt: read8(DMAC0 + DMCNT),
                dmreq: read8(DMAC0 + DMREQ),
            }
        }

        unsafe fn restore(self) {
            write8(DMAC0 + DMCNT, 0);
            write8(DMAC0 + DMREQ, 0);
            write32(DMAC0 + DMSAR, self.dmsar);
            write32(DMAC0 + DMDAR, self.dmdar);
            write32(DMAC0 + DMCRA, self.dmcra);
            write32(DMAC0 + DMCRB, self.dmcrb);
            write16(DMAC0 + DMTMD, self.dmtmd);
            write8(DMAC0 + DMINT, self.dmint);
            write16(DMAC0 + DMAMD, self.dmamd);
            write32(DMAC0 + DMOFR, self.dmofr);
            write8(DMAC0 + DMREQ, self.dmreq);
            write8(DMAC0 + DMCNT, self.dmcnt);
        }
    }

    struct HardwareSnapshot {
        prcr_bits: u16,
        mstpcra: u32,
        mstpcrd: u32,
        dmast: u8,
        delsr0: u32,
        ielsr_dma: u32,
        ielsr_timeout: u32,
        dma_irq_enabled: bool,
        timeout_irq_enabled: bool,
        dma_irq_pending: bool,
        timeout_irq_pending: bool,
        dma_priority: u8,
        timeout_priority: u8,
        scr: u32,
        dmac: DmacSnapshot,
        gpt0: GptSnapshot,
        gpt1: GptSnapshot,
        restored: bool,
    }

    impl HardwareSnapshot {
        fn acquire() -> Result<Self, EventDmaError> {
            if cortex_m::register::primask::read().is_inactive()
                || cortex_m::register::faultmask::read().is_inactive()
            {
                return Err(EventDmaError::InterruptsMasked);
            }
            unsafe {
                let prcr_bits = read16(PRCR) & 0x0003;
                let mstpcra = read32(MSTPCRA);
                let mstpcrd = read32(MSTPCRD);
                write16(PRCR, 0xA502);
                write32(MSTPCRA, mstpcra & !DMAC_MSTP);
                write32(MSTPCRD, mstpcrd & !GPT01_MSTP);
                write16(PRCR, 0xA500 | prcr_bits);

                let snapshot = Self {
                    prcr_bits,
                    mstpcra,
                    mstpcrd,
                    dmast: read8(DMA + DMAST),
                    delsr0: read32(DELSR0),
                    ielsr_dma: read32(ielsr(EVENT_DMA_IRQ)),
                    ielsr_timeout: read32(ielsr(EVENT_DMA_TIMEOUT_IRQ)),
                    dma_irq_enabled: read32(NVIC_ISER) & DMA_IRQ_MASK != 0,
                    timeout_irq_enabled: read32(NVIC_ISER) & TIMEOUT_IRQ_MASK != 0,
                    dma_irq_pending: read32(NVIC_ISPR) & DMA_IRQ_MASK != 0,
                    timeout_irq_pending: read32(NVIC_ISPR) & TIMEOUT_IRQ_MASK != 0,
                    dma_priority: read8(NVIC_IPR + EVENT_DMA_IRQ),
                    timeout_priority: read8(NVIC_IPR + EVENT_DMA_TIMEOUT_IRQ),
                    scr: read32(SCB_SCR),
                    dmac: DmacSnapshot::capture(),
                    gpt0: GptSnapshot::capture(GPT0),
                    gpt1: GptSnapshot::capture(GPT1),
                    restored: false,
                };
                if read8(DMAC0 + DMCNT) != 0
                    || read8(DMAC0 + DMSTS) & DMSTS_ACTIVE != 0
                    || read32(GPT0 + GTCR) & 1 != 0
                    || read32(GPT1 + GTCR) & 1 != 0
                    || read32(DELSR0) & (0x1ff | IELSR_IR) != 0
                    || read32(ielsr(EVENT_DMA_IRQ)) & (0x1ff | IELSR_IR) != 0
                    || read32(ielsr(EVENT_DMA_TIMEOUT_IRQ)) & (0x1ff | IELSR_IR) != 0
                {
                    let mut snapshot = snapshot;
                    snapshot.restore();
                    return Err(EventDmaError::ResourceBusy);
                }
                Ok(snapshot)
            }
        }

        fn restore(&mut self) {
            if self.restored {
                return;
            }
            unsafe {
                disable_owned_irqs();
                write32(GPT0 + GTSTP, 1);
                write32(GPT1 + GTSTP, 2);
                write8(DMAC0 + DMCNT, 0);
                write32(DELSR0, 0);
                write32(ielsr(EVENT_DMA_IRQ), 0);
                write32(ielsr(EVENT_DMA_TIMEOUT_IRQ), 0);
                for _ in 0..ABORT_POLLS {
                    if read8(DMAC0 + DMSTS) & DMSTS_ACTIVE == 0 {
                        break;
                    }
                    core::hint::spin_loop();
                }
                COMPLETION.cancel();
                self.dmac.restore();
                self.gpt0.restore(GPT0, 1);
                self.gpt1.restore(GPT1, 2);
                write8(DMA + DMAST, self.dmast);
                write32(DELSR0, self.delsr0 & !IELSR_IR);
                write32(ielsr(EVENT_DMA_IRQ), self.ielsr_dma & !IELSR_IR);
                write32(ielsr(EVENT_DMA_TIMEOUT_IRQ), self.ielsr_timeout & !IELSR_IR);
                write8(NVIC_IPR + EVENT_DMA_IRQ, self.dma_priority);
                write8(NVIC_IPR + EVENT_DMA_TIMEOUT_IRQ, self.timeout_priority);
                clear_owned_pending();
                if self.dma_irq_pending {
                    write32(NVIC_ISPR, DMA_IRQ_MASK);
                }
                if self.timeout_irq_pending {
                    write32(NVIC_ISPR, TIMEOUT_IRQ_MASK);
                }
                if self.dma_irq_enabled {
                    write32(NVIC_ISER, DMA_IRQ_MASK);
                }
                if self.timeout_irq_enabled {
                    write32(NVIC_ISER, TIMEOUT_IRQ_MASK);
                }
                write32(SCB_SCR, self.scr);
                write16(PRCR, 0xA502);
                write32(MSTPCRA, self.mstpcra);
                write32(MSTPCRD, self.mstpcrd);
                write16(PRCR, 0xA500 | self.prcr_bits);
            }
            self.restored = true;
        }

        #[cfg(feature = "event-dma-selftest")]
        fn same_owned_state(&self, other: &Self) -> bool {
            self.prcr_bits == other.prcr_bits
                && self.mstpcra == other.mstpcra
                && self.mstpcrd == other.mstpcrd
                && self.dmast == other.dmast
                && self.delsr0 == other.delsr0
                && self.ielsr_dma == other.ielsr_dma
                && self.ielsr_timeout == other.ielsr_timeout
                && self.dma_irq_enabled == other.dma_irq_enabled
                && self.timeout_irq_enabled == other.timeout_irq_enabled
                && self.dma_irq_pending == other.dma_irq_pending
                && self.timeout_irq_pending == other.timeout_irq_pending
                && self.dma_priority == other.dma_priority
                && self.timeout_priority == other.timeout_priority
                && self.scr == other.scr
                && self.dmac == other.dmac
                && self.gpt0 == other.gpt0
                && self.gpt1 == other.gpt1
        }
    }

    impl Drop for HardwareSnapshot {
        fn drop(&mut self) {
            self.restore();
        }
    }

    pub struct Ra4m1EventDma {
        _sealed: (),
    }

    impl Ra4m1EventDma {
        pub fn take() -> Result<Self, EventDmaError> {
            TAKEN
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .map_err(|_| EventDmaError::Busy)?;
            Ok(Self { _sealed: () })
        }

        pub fn copy<'a>(
            &'a mut self,
            source: &'a [u32],
            destination: &'a mut [u32],
        ) -> EventDmaCopy<'a> {
            self.copy_every(source, destination, EVENT_DMA_DEFAULT_PERIOD_US)
        }

        pub fn copy_every<'a>(
            &'a mut self,
            source: &'a [u32],
            destination: &'a mut [u32],
            period_us: u32,
        ) -> EventDmaCopy<'a> {
            let source_words = source.len();
            let destination_words = destination.len();
            EventDmaCopy {
                _provider: self,
                source: Some(source),
                destination: Some(destination),
                plan: EventDmaPlan::new(source_words, destination_words, period_us),
                hardware: None,
                state: TransferState::New,
                _pin: PhantomPinned,
            }
        }
    }

    impl Drop for Ra4m1EventDma {
        fn drop(&mut self) {
            COMPLETION.cancel();
            TAKEN.store(false, Ordering::Release);
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum TransferState {
        New,
        InFlight,
        Done,
    }

    pub struct EventDmaCopy<'a> {
        _provider: &'a mut Ra4m1EventDma,
        source: Option<&'a [u32]>,
        destination: Option<&'a mut [u32]>,
        plan: Result<EventDmaPlan, EventDmaError>,
        hardware: Option<HardwareSnapshot>,
        state: TransferState,
        _pin: PhantomPinned,
    }

    impl Future for EventDmaCopy<'_> {
        type Output = Result<usize, EventDmaError>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            // SAFETY: the future is !Unpin and no pinned field is moved.
            let this = unsafe { self.get_unchecked_mut() };
            match this.state {
                TransferState::New => {
                    let plan = match this.plan {
                        Ok(plan) => plan,
                        Err(error) => {
                            this.state = TransferState::Done;
                            return Poll::Ready(Err(error));
                        }
                    };
                    if COMPLETION.arm(cx.waker()) == Err(CompletionError::Busy) {
                        this.state = TransferState::Done;
                        return Poll::Ready(Err(EventDmaError::Busy));
                    }
                    let hardware = match HardwareSnapshot::acquire() {
                        Ok(hardware) => hardware,
                        Err(error) => {
                            COMPLETION.cancel();
                            this.state = TransferState::Done;
                            return Poll::Ready(Err(error));
                        }
                    };
                    let source = this.source.take().expect("validated source missing");
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            source.as_ptr(),
                            (*SOURCE.0.get()).as_mut_ptr(),
                            plan.words(),
                        );
                        core::ptr::write_bytes(
                            (*DESTINATION.0.get()).as_mut_ptr(),
                            0,
                            plan.words(),
                        );
                        configure_transfer(plan);
                    }
                    compiler_fence(Ordering::SeqCst);
                    TIMED_OUT.store(false, Ordering::Release);
                    #[cfg(feature = "event-dma-selftest")]
                    IRQ_AT_TICK.store(0, Ordering::Release);
                    unsafe {
                        // Consume a stale event before enabling the hardware.
                        cortex_m::asm::sev();
                        cortex_m::asm::wfe();
                        write32(GPT1 + GTSTR, 2);
                        write32(GPT0 + GTSTR, 1);
                    }
                    this.hardware = Some(hardware);
                    this.state = TransferState::InFlight;
                    Poll::Pending
                }
                TransferState::InFlight => {
                    if !COMPLETION.poll_complete(cx) {
                        return Poll::Pending;
                    }
                    let timed_out = TIMED_OUT.load(Ordering::Acquire);
                    let transfer_complete = unsafe {
                        read32(DMAC0 + DMCRA) == 0 && read8(DMAC0 + DMSTS) & DMSTS_ACTIVE == 0
                    };
                    let words = this.plan.expect("in-flight plan missing").words();
                    if !timed_out && transfer_complete {
                        compiler_fence(Ordering::SeqCst);
                        unsafe {
                            let staged_destination: &[u32; EVENT_DMA_MAX_WORDS] =
                                &*DESTINATION.0.get();
                            this.destination
                                .take()
                                .expect("validated destination missing")
                                .copy_from_slice(&staged_destination[..words]);
                        }
                    }
                    if let Some(mut hardware) = this.hardware.take() {
                        hardware.restore();
                    }
                    this.state = TransferState::Done;
                    if timed_out {
                        Poll::Ready(Err(EventDmaError::Timeout))
                    } else if !transfer_complete {
                        Poll::Ready(Err(EventDmaError::CompletionFault))
                    } else {
                        Poll::Ready(Ok(words))
                    }
                }
                TransferState::Done => panic!("EventDmaCopy polled after completion"),
            }
        }
    }

    impl Drop for EventDmaCopy<'_> {
        fn drop(&mut self) {
            if self.state == TransferState::InFlight {
                if let Some(mut hardware) = self.hardware.take() {
                    hardware.restore();
                }
                self.state = TransferState::Done;
            }
        }
    }

    #[cfg(feature = "event-dma-selftest")]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct EventDmaSelfTestReport {
        pub passed: bool,
        pub cancellation_output_untouched: bool,
        pub timeout_path_passed: bool,
        pub state_restored: bool,
        pub words: usize,
        pub polls: u32,
        pub dma_irqs: u32,
        pub timeout_irqs: u32,
        pub task_wakes: u32,
        pub idle_entries: u32,
        pub idle_residence_us: u32,
        pub completion_us: u32,
        pub wake_latency_us: u32,
    }

    #[cfg(feature = "event-dma-selftest")]
    pub fn run_event_dma_selftest(provider: &mut Ra4m1EventDma) -> EventDmaSelfTestReport {
        let mut baseline = match HardwareSnapshot::acquire() {
            Ok(snapshot) => snapshot,
            Err(_) => {
                return EventDmaSelfTestReport {
                    passed: false,
                    cancellation_output_untouched: false,
                    timeout_path_passed: false,
                    state_restored: false,
                    words: 0,
                    polls: 0,
                    dma_irqs: 0,
                    timeout_irqs: 0,
                    task_wakes: 0,
                    idle_entries: 0,
                    idle_residence_us: 0,
                    completion_us: 0,
                    wake_latency_us: u32::MAX,
                };
            }
        };
        baseline.restore();
        let mut source = [0u32; EVENT_DMA_MAX_WORDS];
        for (index, word) in source.iter_mut().enumerate() {
            *word = 0x4D31_0000 ^ index as u32;
        }
        let mut destination = [0xDEAD_BEEFu32; EVENT_DMA_MAX_WORDS];
        let waker = selftest_waker();
        let mut context = Context::from_waker(&waker);

        {
            let mut cancelled = core::pin::pin!(provider.copy(&source, &mut destination));
            let _ = Future::poll(cancelled.as_mut(), &mut context);
        }
        let cancellation_output_untouched = destination.iter().all(|word| *word == 0xDEAD_BEEF);
        let cancellation_state_restored = HardwareSnapshot::acquire()
            .map(|mut snapshot| {
                let restored = baseline.same_owned_state(&snapshot);
                snapshot.restore();
                restored
            })
            .unwrap_or(false);

        let timeout_irq_before = TIMEOUT_IRQ_COUNT.load(Ordering::Acquire);
        let timeout_wakes_before = SELFTEST_WAKE_COUNT.load(Ordering::Acquire);
        let timeout_result = {
            let mut timed_out = core::pin::pin!(provider.copy(&source, &mut destination));
            let first_poll = Future::poll(timed_out.as_mut(), &mut context);
            unsafe {
                // Qualification-only fault injection: remove the GPT0 -> DMAC
                // route after arming so GPT1 must bound the missing completion.
                write32(DELSR0, 0);
            }
            cortex_m::asm::dsb();
            cortex_m::asm::wfe();
            let second_poll = Future::poll(timed_out.as_mut(), &mut context);
            (first_poll, second_poll)
        };
        let timeout_path_passed = timeout_result
            == (Poll::Pending, Poll::Ready(Err(EventDmaError::Timeout)))
            && destination.iter().all(|word| *word == 0xDEAD_BEEF)
            && TIMEOUT_IRQ_COUNT
                .load(Ordering::Acquire)
                .wrapping_sub(timeout_irq_before)
                == 1
            && SELFTEST_WAKE_COUNT
                .load(Ordering::Acquire)
                .wrapping_sub(timeout_wakes_before)
                == 1;
        let timeout_state_restored = HardwareSnapshot::acquire()
            .map(|mut snapshot| {
                let restored = baseline.same_owned_state(&snapshot);
                snapshot.restore();
                restored
            })
            .unwrap_or(false);
        destination.fill(0);

        let dma_before = DMA_IRQ_COUNT.load(Ordering::Acquire);
        let timeout_before = TIMEOUT_IRQ_COUNT.load(Ordering::Acquire);
        let wakes_before = SELFTEST_WAKE_COUNT.load(Ordering::Acquire);
        let mut polls = 0;
        let mut idle_entries = 0u32;
        let mut idle_ticks = 0u32;
        let mut completion_ticks = 0u32;
        let result = {
            let mut transfer = core::pin::pin!(provider.copy(&source, &mut destination));
            loop {
                polls += 1;
                match Future::poll(transfer.as_mut(), &mut context) {
                    Poll::Ready(result) => break result,
                    Poll::Pending => {
                        let started = unsafe { read32(GPT1 + GTCNT) };
                        cortex_m::asm::dsb();
                        cortex_m::asm::wfe();
                        let finished = unsafe { read32(GPT1 + GTCNT) };
                        idle_entries = idle_entries.wrapping_add(1);
                        idle_ticks = idle_ticks.wrapping_add(finished.wrapping_sub(started));
                        completion_ticks = finished;
                    }
                }
            }
        };
        let irq_tick = IRQ_AT_TICK.load(Ordering::Acquire);
        let dma_irqs = DMA_IRQ_COUNT
            .load(Ordering::Acquire)
            .wrapping_sub(dma_before);
        let timeout_irqs = TIMEOUT_IRQ_COUNT
            .load(Ordering::Acquire)
            .wrapping_sub(timeout_before);
        let task_wakes = SELFTEST_WAKE_COUNT
            .load(Ordering::Acquire)
            .wrapping_sub(wakes_before);
        let idle_residence_us = idle_ticks / PCLKD_CYCLES_PER_US;
        let completion_us = completion_ticks / PCLKD_CYCLES_PER_US;
        let wake_latency_us = completion_ticks.wrapping_sub(irq_tick) / PCLKD_CYCLES_PER_US;
        let completion_state_restored = HardwareSnapshot::acquire()
            .map(|mut snapshot| {
                let restored = baseline.same_owned_state(&snapshot);
                snapshot.restore();
                restored
            })
            .unwrap_or(false);
        let state_restored =
            cancellation_state_restored && timeout_state_restored && completion_state_restored;
        let passed = cancellation_output_untouched
            && timeout_path_passed
            && state_restored
            && result == Ok(EVENT_DMA_MAX_WORDS)
            && destination == source
            && polls == 2
            && dma_irqs == 1
            && timeout_irqs == 0
            && task_wakes == 1
            && idle_entries == 1
            && idle_residence_us >= 5_000
            && completion_us >= 6_000
            && wake_latency_us <= 100;

        EventDmaSelfTestReport {
            passed,
            cancellation_output_untouched,
            timeout_path_passed,
            state_restored,
            words: EVENT_DMA_MAX_WORDS,
            polls,
            dma_irqs,
            timeout_irqs,
            task_wakes,
            idle_entries,
            idle_residence_us,
            completion_us,
            wake_latency_us,
        }
    }

    unsafe fn configure_transfer(plan: EventDmaPlan) {
        disable_owned_irqs();
        clear_owned_pending();
        write32(SCB_SCR, read32(SCB_SCR) | SCR_SEVONPEND);
        write8(NVIC_IPR + EVENT_DMA_IRQ, DMA_PRIORITY_RAW);
        write8(NVIC_IPR + EVENT_DMA_TIMEOUT_IRQ, TIMEOUT_PRIORITY_RAW);

        configure_gpt(GPT0, 1, plan.period_ticks());
        configure_gpt(GPT1, 2, plan.timeout_ticks());

        write8(DMAC0 + DMCNT, 0);
        write8(DMAC0 + DMREQ, 0);
        write32(DMAC0 + DMSAR, (*SOURCE.0.get()).as_ptr() as u32);
        write32(DMAC0 + DMDAR, (*DESTINATION.0.get()).as_mut_ptr() as u32);
        write32(DMAC0 + DMCRA, plan.words() as u32);
        write32(DMAC0 + DMCRB, 0);
        write16(DMAC0 + DMTMD, DMTMD_EVENT_WORD_NORMAL);
        write8(DMAC0 + DMINT, DMINT_TRANSFER_END);
        write16(DMAC0 + DMAMD, DMAMD_INCREMENT_BOTH);
        write32(DMAC0 + DMOFR, 0);
        write8(DMA + DMAST, 1);

        write32(DELSR0, ELC_EVENT_GPT0_OVERFLOW);
        write32(ielsr(EVENT_DMA_IRQ), ELC_EVENT_DMAC0_INT);
        write32(ielsr(EVENT_DMA_TIMEOUT_IRQ), ELC_EVENT_GPT1_OVERFLOW);
        clear_owned_pending();
        write32(NVIC_ISER, DMA_IRQ_MASK | TIMEOUT_IRQ_MASK);
        write8(DMAC0 + DMCNT, 1);
    }

    unsafe fn configure_gpt(base: usize, channel_mask: u32, ticks: u32) {
        write32(base + GTSTP, channel_mask);
        write32(base + GTWP, GPT_WRITE_UNLOCKED);
        write32(base + GTCR, 0);
        write32(base + GTSSR, GPT_GROUP_SOFTWARE);
        write32(base + GTPSR, GPT_GROUP_SOFTWARE);
        write32(base + GTCSR, GPT_GROUP_SOFTWARE);
        write32(base + GTUPSR, 0);
        write32(base + GTDNSR, 0);
        write32(base + GTICASR, 0);
        write32(base + GTICBSR, 0);
        write32(base + GTIOR, 0);
        write32(base + GTINTAD, 0);
        write32(base + GTBER, 0);
        write32(base + GTPR, ticks - 1);
        write32(base + GTPBR, ticks - 1);
        write32(base + GTCNT, 0);
        write32(base + GTCLR, channel_mask);
        write32(base + GTUDDTYC, 3);
        write32(base + GTUDDTYC, 1);
        write32(base + GTWP, GPT_WRITE_PROTECTED);
    }

    #[inline]
    const fn ielsr(irq: usize) -> usize {
        IELSR_BASE + irq * 4
    }

    unsafe fn disable_owned_irqs() {
        write32(NVIC_ICER, DMA_IRQ_MASK | TIMEOUT_IRQ_MASK);
    }

    unsafe fn clear_owned_pending() {
        write32(
            ielsr(EVENT_DMA_IRQ),
            read32(ielsr(EVENT_DMA_IRQ)) & !IELSR_IR,
        );
        write32(
            ielsr(EVENT_DMA_TIMEOUT_IRQ),
            read32(ielsr(EVENT_DMA_TIMEOUT_IRQ)) & !IELSR_IR,
        );
        write32(NVIC_ICPR, DMA_IRQ_MASK | TIMEOUT_IRQ_MASK);
    }

    /// DMAC0 completion vector installed by the feature-on RA4M1 image.
    ///
    /// # Safety
    ///
    /// This function may only be entered by NVIC slot [`EVENT_DMA_IRQ`] while
    /// the provider owns the matching ICU route and DMAC0 registers.
    pub unsafe extern "C" fn event_dma_irq() {
        write32(
            ielsr(EVENT_DMA_IRQ),
            read32(ielsr(EVENT_DMA_IRQ)) & !IELSR_IR,
        );
        write32(GPT0 + GTSTP, 1);
        compiler_fence(Ordering::SeqCst);
        #[cfg(feature = "event-dma-selftest")]
        IRQ_AT_TICK.store(read32(GPT1 + GTCNT), Ordering::Release);
        #[cfg(feature = "event-dma-selftest")]
        DMA_IRQ_COUNT.fetch_add(1, Ordering::AcqRel);
        COMPLETION.complete_from_isr();
    }

    /// GPT1 fail-safe vector installed by the feature-on RA4M1 image.
    ///
    /// # Safety
    ///
    /// This function may only be entered by NVIC slot
    /// [`EVENT_DMA_TIMEOUT_IRQ`] while the provider owns GPT1 and its ICU route.
    pub unsafe extern "C" fn event_dma_timeout_irq() {
        write32(
            ielsr(EVENT_DMA_TIMEOUT_IRQ),
            read32(ielsr(EVENT_DMA_TIMEOUT_IRQ)) & !IELSR_IR,
        );
        write32(GPT0 + GTSTP, 1);
        TIMED_OUT.store(true, Ordering::Release);
        #[cfg(feature = "event-dma-selftest")]
        TIMEOUT_IRQ_COUNT.fetch_add(1, Ordering::AcqRel);
        COMPLETION.complete_from_isr();
    }

    #[cfg(feature = "event-dma-selftest")]
    unsafe fn selftest_waker_clone(_: *const ()) -> RawWaker {
        RawWaker::new(core::ptr::null(), &SELFTEST_WAKER_VTABLE)
    }

    #[cfg(feature = "event-dma-selftest")]
    unsafe fn selftest_waker_wake(_: *const ()) {
        SELFTEST_WAKE_COUNT.fetch_add(1, Ordering::AcqRel);
    }

    #[cfg(feature = "event-dma-selftest")]
    unsafe fn selftest_waker_drop(_: *const ()) {}

    #[cfg(feature = "event-dma-selftest")]
    static SELFTEST_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        selftest_waker_clone,
        selftest_waker_wake,
        selftest_waker_wake,
        selftest_waker_drop,
    );

    #[cfg(feature = "event-dma-selftest")]
    fn selftest_waker() -> Waker {
        unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &SELFTEST_WAKER_VTABLE)) }
    }

    #[inline]
    unsafe fn read8(address: usize) -> u8 {
        (address as *const u8).read_volatile()
    }

    #[inline]
    unsafe fn write8(address: usize, value: u8) {
        (address as *mut u8).write_volatile(value);
    }

    #[inline]
    unsafe fn read16(address: usize) -> u16 {
        (address as *const u16).read_volatile()
    }

    #[inline]
    unsafe fn write16(address: usize, value: u16) {
        (address as *mut u16).write_volatile(value);
    }

    #[inline]
    unsafe fn read32(address: usize) -> u32 {
        (address as *const u32).read_volatile()
    }

    #[inline]
    unsafe fn write32(address: usize, value: u32) {
        (address as *mut u32).write_volatile(value);
    }
}

#[cfg(all(target_arch = "arm", feature = "event-dma"))]
pub use hardware::{
    event_dma_irq, event_dma_timeout_irq, Ra4m1EventDma, EVENT_DMA_IRQ, EVENT_DMA_TIMEOUT_IRQ,
};
#[cfg(all(target_arch = "arm", feature = "event-dma-selftest"))]
pub use hardware::{run_event_dma_selftest, EventDmaSelfTestReport};

#[cfg(test)]
mod tests {
    use super::*;

    fn shadow(
        source_words: usize,
        destination_words: usize,
        period_us: u32,
    ) -> Result<usize, EventDmaError> {
        if source_words == 0 {
            return Err(EventDmaError::EmptyTransfer);
        }
        if source_words != destination_words {
            return Err(EventDmaError::LengthMismatch);
        }
        if source_words > EVENT_DMA_MAX_WORDS {
            return Err(EventDmaError::TransferTooLong);
        }
        if !(EVENT_DMA_MIN_PERIOD_US..=EVENT_DMA_MAX_PERIOD_US).contains(&period_us) {
            return Err(EventDmaError::PeriodOutOfRange);
        }
        Ok(source_words)
    }

    #[test]
    fn default_contract_is_one_call_sized_and_bounded() {
        let plan = EventDmaPlan::new(
            EVENT_DMA_MAX_WORDS,
            EVENT_DMA_MAX_WORDS,
            EVENT_DMA_DEFAULT_PERIOD_US,
        )
        .unwrap();
        assert_eq!(plan.words(), EVENT_DMA_MAX_WORDS);
        assert_eq!(plan.period_ticks(), 4_800);
        assert_eq!(plan.timeout_ticks(), 556_800);
    }

    #[test]
    fn typed_validation_matches_shadow_grid() {
        let mut seed = 0x4D31_81D0_u32;
        for _ in 0..2_048 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let source = (seed as usize >> 3) % 72;
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let destination = (seed as usize >> 5) % 72;
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let period = seed % 220;
            assert_eq!(
                EventDmaPlan::new(source, destination, period).map(EventDmaPlan::words),
                shadow(source, destination, period)
            );
        }
    }
}
