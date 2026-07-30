//! Bounded EIC/EVSYS/DMAC composition for the SAMD21.
//!
//! `atsamd-hal` owns the EIC and DMAC register drivers. This module owns the
//! NobroRTOS resource session, exact route identity, transfer bounds, and
//! cancellation contract. A final firmware supplies a backend built from the
//! controllers returned by [`crate::board::event_controller`] and
//! [`crate::board::dma_controller`].

use crate::lease::{Samd21LeaseGuard, Samd21Leases, DMAC0_LEASE, EVSYS0_LEASE};

pub const EVENT_CHANNEL: u8 = 0;
pub const EVENT_SOURCE_EIC7: u8 = 19;
pub const EVENT_USER_DMAC0: u8 = 0;
pub const MAX_TRANSFER_WORDS: usize = 256;
pub const DEFAULT_POLL_BUDGET: u32 = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventDmaRoute {
    pub event_channel: u8,
    pub generator: u8,
    pub user: u8,
}

impl EventDmaRoute {
    pub const PN532_IRQ_TO_DMAC0: Self = Self {
        event_channel: EVENT_CHANNEL,
        generator: EVENT_SOURCE_EIC7,
        user: EVENT_USER_DMAC0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventDmaPlan {
    words: usize,
    poll_budget: u32,
}

impl EventDmaPlan {
    pub const fn new(
        source_words: usize,
        destination_words: usize,
        poll_budget: u32,
    ) -> Result<Self, EventDmaError<core::convert::Infallible>> {
        if source_words == 0 {
            return Err(EventDmaError::Empty);
        }
        if source_words != destination_words {
            return Err(EventDmaError::LengthMismatch);
        }
        if source_words > MAX_TRANSFER_WORDS {
            return Err(EventDmaError::TooLong);
        }
        if poll_budget == 0 {
            return Err(EventDmaError::InvalidBudget);
        }
        Ok(Self {
            words: source_words,
            poll_budget,
        })
    }

    pub const fn words(self) -> usize {
        self.words
    }

    pub const fn poll_budget(self) -> u32 {
        self.poll_budget
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventDmaError<E> {
    Lease(nobro_hal::LeaseError),
    Backend(E),
    Empty,
    LengthMismatch,
    TooLong,
    InvalidBudget,
    Timeout,
}

/// Safe blocking boundary implemented by a concrete atsamd-hal composition.
///
/// The backend must stop or complete the channel before returning. This keeps
/// caller-owned buffers from outliving a leaked asynchronous DMA object.
pub trait EventDmaBackend {
    type Error;

    fn copy_event_paced(
        &mut self,
        route: EventDmaRoute,
        source: &[u32],
        destination: &mut [u32],
        poll_budget: u32,
    ) -> Result<bool, Self::Error>;

    fn cancel(&mut self);
}

/// Concrete SAMD21 channel-0 backend.
///
/// `atsamd-hal` safely creates the controller, channel, and descriptor. The
/// HAL does not currently expose the D21 channel event-input fields, so this
/// adapter performs the small, datasheet-defined CHCTRLB sequence directly:
/// `TRIGSRC=DISABLE`, `TRIGACT=BLOCK`, `EVACT=TRIG`, `EVIE=1`. The transfer
/// object is never dropped while the channel is enabled; every success,
/// timeout, and error path disables the channel and executes an acquire fence
/// before returning caller-owned storage.
#[cfg(target_arch = "arm")]
pub struct AtsamdEventDmaBackend {
    _controller: atsamd_hal::dmac::DmaController,
    channel: Option<atsamd_hal::dmac::Channel<atsamd_hal::dmac::Ch0, atsamd_hal::dmac::Ready>>,
    staging: [u32; MAX_TRANSFER_WORDS],
    active: bool,
}

#[cfg(target_arch = "arm")]
impl AtsamdEventDmaBackend {
    pub fn new(mut controller: atsamd_hal::dmac::DmaController) -> Self {
        use atsamd_hal::dmac::PriorityLevel;

        let channels = controller.split();
        Self {
            _controller: controller,
            channel: Some(channels.0.init(PriorityLevel::Lvl0)),
            staging: [0; MAX_TRANSFER_WORDS],
            active: false,
        }
    }

    fn registers() -> &'static atsamd_hal::pac::dmac::RegisterBlock {
        unsafe { &*atsamd_hal::pac::Dmac::ptr() }
    }

    fn select_channel_zero(registers: &atsamd_hal::pac::dmac::RegisterBlock) {
        registers
            .chid()
            .write(|w| unsafe { w.id().bits(EVENT_USER_DMAC0) });
    }

    fn stop_channel(poll_budget: u32) -> bool {
        use core::sync::atomic::{fence, Ordering};

        let registers = Self::registers();
        Self::select_channel_zero(registers);
        registers.chctrla().modify(|_, w| w.enable().clear_bit());
        let mut remaining = poll_budget;
        while registers.chctrla().read().enable().bit_is_set() && remaining != 0 {
            remaining -= 1;
            core::hint::spin_loop();
        }
        registers
            .chctrlb()
            .modify(|_, w| w.evie().clear_bit().evact().noact());
        fence(Ordering::Acquire);
        remaining != 0 || registers.chctrla().read().enable().bit_is_clear()
    }
}

#[cfg(target_arch = "arm")]
impl EventDmaBackend for AtsamdEventDmaBackend {
    type Error = atsamd_hal::dmac::Error;

    fn copy_event_paced(
        &mut self,
        route: EventDmaRoute,
        source: &[u32],
        destination: &mut [u32],
        poll_budget: u32,
    ) -> Result<bool, Self::Error> {
        use core::sync::atomic::{fence, Ordering};

        debug_assert_eq!(route, EventDmaRoute::PN532_IRQ_TO_DMAC0);
        self.staging[..source.len()].copy_from_slice(source);
        let channel = self
            .channel
            .take()
            .expect("bounded backend cannot be entered recursively");

        // SAFETY: both slices have the same checked length, remain borrowed by
        // `transfer`, and the channel is synchronously stopped below on every
        // path before either borrow is returned to its owner.
        let transfer = unsafe {
            atsamd_hal::dmac::Transfer::new_unchecked(
                channel,
                &mut self.staging[..source.len()],
                destination,
                false,
            )
        };

        let registers = Self::registers();
        Self::select_channel_zero(registers);
        registers.chintflag().write(|w| {
            w.terr().set_bit();
            w.tcmpl().set_bit();
            w.susp().set_bit()
        });
        registers.chctrlb().write(|w| {
            w.trigsrc()
                .disable()
                .trigact()
                .block()
                .evact()
                .trig()
                .evie()
                .set_bit()
        });
        fence(Ordering::Release);
        registers.chctrla().modify(|_, w| w.enable().set_bit());
        self.active = true;

        let mut remaining = poll_budget;
        while registers.chctrla().read().enable().bit_is_set() && remaining != 0 {
            remaining -= 1;
            core::hint::spin_loop();
        }
        let flags = registers.chintflag().read();
        let completed = registers.chctrla().read().enable().bit_is_clear();
        let stopped = Self::stop_channel(poll_budget);
        let (channel, _, _) = transfer.free();
        self.channel = Some(channel);
        self.active = false;

        if flags.terr().bit_is_set() {
            return Err(atsamd_hal::dmac::Error::TransferError);
        }
        Ok(completed && stopped)
    }

    fn cancel(&mut self) {
        if self.active {
            let _ = Self::stop_channel(DEFAULT_POLL_BUDGET);
            self.active = false;
        }
    }
}

pub struct Samd21EventDma<B: EventDmaBackend> {
    backend: B,
    dma_lease: Samd21LeaseGuard,
    route_lease: Samd21LeaseGuard,
}

impl<B> Samd21EventDma<B>
where
    B: EventDmaBackend,
{
    pub fn try_new(backend: B, owner: u8) -> Result<Self, nobro_hal::LeaseError> {
        let dma_lease = Samd21Leases::acquire_guard(DMAC0_LEASE, owner)?;
        let route_lease = Samd21Leases::acquire_guard(EVSYS0_LEASE, owner)?;
        Ok(Self {
            backend,
            dma_lease,
            route_lease,
        })
    }

    pub fn copy(
        &mut self,
        source: &[u32],
        destination: &mut [u32],
    ) -> Result<usize, EventDmaError<B::Error>> {
        self.copy_with_budget(source, destination, DEFAULT_POLL_BUDGET)
    }

    pub fn copy_with_budget(
        &mut self,
        source: &[u32],
        destination: &mut [u32],
        poll_budget: u32,
    ) -> Result<usize, EventDmaError<B::Error>> {
        self.dma_lease.ensure_live().map_err(EventDmaError::Lease)?;
        self.route_lease
            .ensure_live()
            .map_err(EventDmaError::Lease)?;
        let plan =
            EventDmaPlan::new(source.len(), destination.len(), poll_budget).map_err(|error| {
                match error {
                    EventDmaError::Empty => EventDmaError::Empty,
                    EventDmaError::LengthMismatch => EventDmaError::LengthMismatch,
                    EventDmaError::TooLong => EventDmaError::TooLong,
                    EventDmaError::InvalidBudget => EventDmaError::InvalidBudget,
                    _ => unreachable!(),
                }
            })?;
        match self.backend.copy_event_paced(
            EventDmaRoute::PN532_IRQ_TO_DMAC0,
            source,
            destination,
            plan.poll_budget(),
        ) {
            Ok(true) => Ok(plan.words()),
            Ok(false) => {
                self.backend.cancel();
                Err(EventDmaError::Timeout)
            }
            Err(error) => {
                self.backend.cancel();
                Err(EventDmaError::Backend(error))
            }
        }
    }
}

impl<B: EventDmaBackend> Drop for Samd21EventDma<B> {
    fn drop(&mut self) {
        self.backend.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeDma {
        finish: bool,
        cancelled: bool,
    }

    impl EventDmaBackend for FakeDma {
        type Error = ();

        fn copy_event_paced(
            &mut self,
            route: EventDmaRoute,
            source: &[u32],
            destination: &mut [u32],
            _: u32,
        ) -> Result<bool, Self::Error> {
            assert_eq!(route, EventDmaRoute::PN532_IRQ_TO_DMAC0);
            if self.finish {
                destination.copy_from_slice(source);
            }
            Ok(self.finish)
        }

        fn cancel(&mut self) {
            self.cancelled = true;
        }
    }

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn plan_rejects_unbounded_shapes() {
        assert_eq!(
            EventDmaRoute::PN532_IRQ_TO_DMAC0,
            EventDmaRoute {
                event_channel: 0,
                generator: 19,
                user: 0,
            }
        );
        assert_eq!(EventDmaPlan::new(0, 0, 1), Err(EventDmaError::Empty));
        assert_eq!(
            EventDmaPlan::new(2, 1, 1),
            Err(EventDmaError::LengthMismatch)
        );
        assert_eq!(
            EventDmaPlan::new(MAX_TRANSFER_WORDS + 1, MAX_TRANSFER_WORDS + 1, 1),
            Err(EventDmaError::TooLong)
        );
        assert_eq!(
            EventDmaPlan::new(1, 1, 0),
            Err(EventDmaError::InvalidBudget)
        );
    }

    #[test]
    fn event_dma_owns_both_resources_and_copies() {
        let _lock = lock();
        let mut dma = Samd21EventDma::try_new(
            FakeDma {
                finish: true,
                cancelled: false,
            },
            3,
        )
        .unwrap();
        let source = [1, 2, 3];
        let mut destination = [0; 3];
        assert_eq!(dma.copy(&source, &mut destination), Ok(3));
        assert_eq!(destination, source);
    }

    #[test]
    fn timeout_is_fail_closed() {
        let _lock = lock();
        let mut dma = Samd21EventDma::try_new(FakeDma::default(), 4).unwrap();
        let source = [1];
        let mut destination = [0];
        assert_eq!(
            dma.copy_with_budget(&source, &mut destination, 1),
            Err(EventDmaError::Timeout)
        );
        assert!(dma.backend.cancelled);
    }
}
