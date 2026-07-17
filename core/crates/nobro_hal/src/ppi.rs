//! nRF52840 PPI routing: timestamp capture and interrupt-driven future wakes.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use cortex_m::peripheral::NVIC;
use nrf52840_pac::{GPIOTE, PPI, TIMER0};

use crate::board::{LED_PIN, MVK_TRIGGER_PIN};
use crate::completion::{CompletionCell, CompletionError};
use crate::lease::{LeaseError, LeaseGuard, Resource, ResourceLease};
use crate::priority_ceiling::CompletionInterruptPriority;

const PPI_CH: usize = 0;
const WAKE_PPI_CH: usize = 2;
const WAKE_EGU_EVENT: usize = 1;
const EGU0_BASE: u32 = 0x4001_4000;
const PPI_BASE: u32 = 0x4001_F000;
const EGU_TASK_TRIGGER0: u32 = 0x000;
const EGU_EVENT_TRIGGERED0: u32 = 0x100;
const EGU_INTENSET: u32 = 0x304;
const EGU_INTENCLR: u32 = 0x308;
const PPI_CHENSET: u32 = 0x504;
const PPI_CHENCLR: u32 = 0x508;
const PPI_CH0_EEP: u32 = 0x510;
const PPI_CH0_TEP: u32 = 0x514;
static PPI_WAKE_COMPLETION: CompletionCell = CompletionCell::new();

fn raw_reg(base: u32, offset: u32) -> *mut u32 {
    (base + offset) as *mut u32
}

fn route_barrier() {
    #[cfg(target_arch = "arm")]
    cortex_m::asm::dsb();
    #[cfg(not(target_arch = "arm"))]
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

fn egu_event_offset() -> u32 {
    EGU_EVENT_TRIGGERED0 + WAKE_EGU_EVENT as u32 * 4
}

fn egu_task_offset() -> u32 {
    EGU_TASK_TRIGGER0 + WAKE_EGU_EVENT as u32 * 4
}

fn ppi_eep_offset() -> u32 {
    PPI_CH0_EEP + WAKE_PPI_CH as u32 * 8
}

fn ppi_tep_offset() -> u32 {
    PPI_CH0_TEP + WAKE_PPI_CH as u32 * 8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PpiWakeError {
    Lease(LeaseError),
    Busy,
    InvalidEndpoint,
}

impl From<LeaseError> for PpiWakeError {
    fn from(error: LeaseError) -> Self {
        Self::Lease(error)
    }
}

/// One leased PPI event -> EGU task -> EGU interrupt completion route.
///
/// The peripheral event reaches EGU without CPU intervention. The EGU ISR then
/// performs the minimum software handoff needed to invoke the waiting future's
/// ordinary waker. This separates zero-CPU event routing from executor policy.
pub struct PpiWakeRoute {
    source_event: *mut u32,
    event_router: LeaseGuard,
    software_event: LeaseGuard,
    interrupt_priority: CompletionInterruptPriority,
}

impl PpiWakeRoute {
    /// Configure the fixed completion route for one nRF peripheral event.
    ///
    /// # Safety
    /// `source_event` must be an aligned, writable nRF peripheral EVENTS
    /// register that remains valid for the route's lifetime. The caller must
    /// own and configure the source peripheral separately.
    pub unsafe fn acquire(owner: u8, source_event: *mut u32) -> Result<Self, PpiWakeError> {
        Self::acquire_with_priority(
            owner,
            source_event,
            CompletionInterruptPriority::board_default(),
        )
    }

    /// Configure the route with an explicitly admitted completion-ISR
    /// priority.
    ///
    /// Convert the logical priority selected for this reactor domain through
    /// [`CompletionInterruptPriority::new`] first. The token guarantees that
    /// the EGU ISR cannot preempt the critical section protecting its waker.
    ///
    /// # Safety
    /// The same endpoint and peripheral-ownership requirements as
    /// [`Self::acquire`] apply.
    pub unsafe fn acquire_with_priority(
        owner: u8,
        source_event: *mut u32,
        interrupt_priority: CompletionInterruptPriority,
    ) -> Result<Self, PpiWakeError> {
        if source_event.is_null() || (source_event as usize) & 3 != 0 {
            return Err(PpiWakeError::InvalidEndpoint);
        }
        let event_router = ResourceLease::acquire_guard(Resource::Ppi, owner)?;
        let software_event = ResourceLease::acquire_guard(Resource::Egu0, owner)?;

        *raw_reg(PPI_BASE, PPI_CHENCLR) = 1 << WAKE_PPI_CH;
        *source_event = 0;
        *raw_reg(EGU0_BASE, egu_event_offset()) = 0;
        *raw_reg(EGU0_BASE, EGU_INTENCLR) = 1 << WAKE_EGU_EVENT;
        *raw_reg(PPI_BASE, ppi_eep_offset()) = source_event as u32;
        *raw_reg(PPI_BASE, ppi_tep_offset()) = EGU0_BASE + egu_task_offset();
        route_barrier();

        Ok(Self {
            source_event,
            event_router,
            software_event,
            interrupt_priority,
        })
    }

    pub const fn interrupt_priority(&self) -> CompletionInterruptPriority {
        self.interrupt_priority
    }

    /// Wait for the next routed event. `start` runs exactly once after the
    /// completion waker, EGU interrupt, and PPI channel are fully armed.
    pub fn wait<F>(&self, start: F) -> PpiWake<'_, F>
    where
        F: FnOnce() + Unpin,
    {
        PpiWake {
            route: self,
            start: Some(start),
            state: PpiWakeState::New,
        }
    }

    fn ensure_live(&self) -> Result<(), PpiWakeError> {
        self.event_router.ensure_live()?;
        self.software_event.ensure_live()?;
        Ok(())
    }

    unsafe fn arm(&self, waker: &Waker) -> Result<(), PpiWakeError> {
        self.ensure_live()?;
        PPI_WAKE_COMPLETION
            .arm(waker)
            .map_err(|CompletionError::Busy| PpiWakeError::Busy)?;
        *raw_reg(PPI_BASE, PPI_CHENCLR) = 1 << WAKE_PPI_CH;
        *self.source_event = 0;
        *raw_reg(EGU0_BASE, egu_event_offset()) = 0;
        *raw_reg(EGU0_BASE, EGU_INTENCLR) = 1 << WAKE_EGU_EVENT;
        NVIC::unpend(nrf52840_pac::Interrupt::SWI0_EGU0);
        let mut core = cortex_m::Peripherals::steal();
        core.NVIC.set_priority(
            nrf52840_pac::Interrupt::SWI0_EGU0,
            self.interrupt_priority.raw(),
        );
        NVIC::unmask(nrf52840_pac::Interrupt::SWI0_EGU0);
        *raw_reg(EGU0_BASE, EGU_INTENSET) = 1 << WAKE_EGU_EVENT;
        *raw_reg(PPI_BASE, PPI_CHENSET) = 1 << WAKE_PPI_CH;
        route_barrier();
        Ok(())
    }

    fn disarm(&self) {
        unsafe {
            *raw_reg(PPI_BASE, PPI_CHENCLR) = 1 << WAKE_PPI_CH;
            *raw_reg(EGU0_BASE, EGU_INTENCLR) = 1 << WAKE_EGU_EVENT;
            *raw_reg(EGU0_BASE, egu_event_offset()) = 0;
            route_barrier();
        }
    }
}

impl Drop for PpiWakeRoute {
    fn drop(&mut self) {
        PPI_WAKE_COMPLETION.cancel();
        self.disarm();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PpiWakeState {
    New,
    InFlight,
    Done,
}

/// Cancellation-safe future returned by [`PpiWakeRoute::wait`].
pub struct PpiWake<'a, F>
where
    F: FnOnce() + Unpin,
{
    route: &'a PpiWakeRoute,
    start: Option<F>,
    state: PpiWakeState,
}

impl<F> Future for PpiWake<'_, F>
where
    F: FnOnce() + Unpin,
{
    type Output = Result<(), PpiWakeError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.state {
            PpiWakeState::New => {
                if let Err(error) = unsafe { self.route.arm(cx.waker()) } {
                    self.state = PpiWakeState::Done;
                    return Poll::Ready(Err(error));
                }
                self.state = PpiWakeState::InFlight;
                self.start.take().expect("PpiWake start already consumed")();
                Poll::Pending
            }
            PpiWakeState::InFlight => {
                if !PPI_WAKE_COMPLETION.poll_complete(cx) {
                    return Poll::Pending;
                }
                self.route.disarm();
                self.state = PpiWakeState::Done;
                Poll::Ready(Ok(()))
            }
            PpiWakeState::Done => panic!("PpiWake polled after completion"),
        }
    }
}

impl<F> Drop for PpiWake<'_, F>
where
    F: FnOnce() + Unpin,
{
    fn drop(&mut self) {
        if self.state != PpiWakeState::InFlight {
            return;
        }
        self.route.disarm();
        PPI_WAKE_COMPLETION.cancel();
        self.state = PpiWakeState::Done;
    }
}

#[no_mangle]
#[allow(non_snake_case)]
#[cfg(target_arch = "arm")]
unsafe extern "C" fn SWI0_EGU0() {
    if *raw_reg(EGU0_BASE, egu_event_offset()) == 0 {
        return;
    }
    *raw_reg(PPI_BASE, PPI_CHENCLR) = 1 << WAKE_PPI_CH;
    *raw_reg(EGU0_BASE, EGU_INTENCLR) = 1 << WAKE_EGU_EVENT;
    *raw_reg(EGU0_BASE, egu_event_offset()) = 0;
    route_barrier();
    PPI_WAKE_COMPLETION.complete_from_isr();
}

/// # Safety
/// Caller must own the PPI + GPIOTE + TIMER0 leases and call once; wires GPIOTE
/// channel 0 and PPI channel 0 to TIMER0 capture (pin edge -> hardware timestamp).
pub unsafe fn mvk_setup_gpiote_ppi_capture() {
    let gpiote = GPIOTE::ptr();
    (*gpiote).config[0].write(|w| {
        w.mode()
            .event()
            .psel()
            .bits(MVK_TRIGGER_PIN)
            .polarity()
            .lo_to_hi()
            .outinit()
            .low()
    });
    (*gpiote).intenclr.write(|w| w.in0().clear_bit());

    let gpiote_event = core::ptr::addr_of!((*gpiote).events_in[0]) as u32;
    let timer = TIMER0::ptr();
    let timer_capture1 = core::ptr::addr_of!((*timer).tasks_capture[1]) as u32;

    let ppi = PPI::ptr();
    (*ppi).ch[PPI_CH]
        .eep
        .write(|w| unsafe { w.bits(gpiote_event) });
    (*ppi).ch[PPI_CH]
        .tep
        .write(|w| unsafe { w.bits(timer_capture1) });
    (*ppi).chenset.write(|w| unsafe { w.bits(1 << PPI_CH) });
}

/// # Safety
/// Writes the board LED pin's PIN_CNF; caller must not have muxed that pin to
/// another peripheral.
pub unsafe fn led_init_output() {
    gpio_output(LED_PIN, false);
}

/// # Safety
/// Read-modify-writes the GPIO OUT register unsynchronized; only call from one
/// context (no ISR/main interleaving on the same port).
pub unsafe fn led_toggle() {
    let pin = LED_PIN as u32;
    if pin < 32 {
        let p = nrf52840_pac::P0::ptr();
        let cur = (*p).out.read().bits();
        (*p).out.write(|w| w.bits(cur ^ (1 << pin)));
    } else {
        let bit = pin - 32;
        let p = nrf52840_pac::P1::ptr();
        let cur = (*p).out.read().bits();
        (*p).out.write(|w| w.bits(cur ^ (1 << bit)));
    }
}

/// # Safety
/// Configures the MVK trigger pin as input+pull-up; caller must not have muxed
/// that pin elsewhere.
pub unsafe fn trigger_input_init() {
    gpio_input_pullup(MVK_TRIGGER_PIN);
}

unsafe fn gpio_output(pin: u8, high: bool) {
    let pin = pin as u32;
    if pin < 32 {
        let p = nrf52840_pac::P0::ptr();
        (*p).pin_cnf[pin as usize].write(|w| w.dir().output());
        if high {
            (*p).outset.write(|w| w.bits(1 << pin));
        } else {
            (*p).outclr.write(|w| w.bits(1 << pin));
        }
    } else {
        let bit = pin - 32;
        let p = nrf52840_pac::P1::ptr();
        (*p).pin_cnf[bit as usize].write(|w| w.dir().output());
        if high {
            (*p).outset.write(|w| w.bits(1 << bit));
        } else {
            (*p).outclr.write(|w| w.bits(1 << bit));
        }
    }
}

unsafe fn gpio_input_pullup(pin: u8) {
    let pin = pin as u32;
    if pin < 32 {
        let p = nrf52840_pac::P0::ptr();
        (*p).pin_cnf[pin as usize].write(|w| w.dir().input().input().connect().pull().pullup());
    } else {
        let bit = pin - 32;
        let p = nrf52840_pac::P1::ptr();
        (*p).pin_cnf[bit as usize].write(|w| w.dir().input().input().connect().pull().pullup());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_route_rejects_invalid_endpoints_before_hardware_or_lease_access() {
        assert!(matches!(
            unsafe { PpiWakeRoute::acquire(1, core::ptr::null_mut()) },
            Err(PpiWakeError::InvalidEndpoint)
        ));
        let misaligned = core::ptr::dangling_mut::<u8>().cast::<u32>();
        assert!(matches!(
            unsafe { PpiWakeRoute::acquire(1, misaligned) },
            Err(PpiWakeError::InvalidEndpoint)
        ));
    }
}
