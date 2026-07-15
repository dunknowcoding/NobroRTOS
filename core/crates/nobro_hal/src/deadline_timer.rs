//! TIMER1 50 Hz deadline slot interrupt at the highest NVIC priority.

use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::peripheral::NVIC;
use nrf52840_pac::TIMER1;

use crate::lease::LeaseError;

const PRESCALER: u32 = 4;
const TICKS_PER_PERIOD: u32 = 20_000;
static PENDING_PERIOD_US: AtomicU32 = AtomicU32::new(0);

pub struct DeadlineTimer;

impl DeadlineTimer {
    /// # Safety
    /// Caller must own the TIMER1 lease and call once; reprograms TIMER1's mode,
    /// prescaler and compare registers for the 50 Hz deadline slot.
    pub unsafe fn init() {
        let t = TIMER1::ptr();
        (*t).tasks_stop.write(|w| w.bits(1));
        (*t).tasks_clear.write(|w| w.bits(1));
        (*t).mode.write(|w| w.mode().timer());
        (*t).bitmode.write(|w| w.bitmode()._32bit());
        (*t).prescaler
            .write(|w| w.prescaler().bits(PRESCALER as u8));
        (*t).cc[0].write(|w| w.bits(TICKS_PER_PERIOD));
        (*t).shorts.write(|w| w.compare0_clear().set_bit());
        (*t).intenset.write(|w| w.compare0().set_bit());
        PENDING_PERIOD_US.store(0, Ordering::Release);
        (*t).tasks_start.write(|w| w.bits(1));
    }

    /// Queue a 1 MHz compare interval while the caller holds the TIMER1
    /// session. The live ISR applies it at the next compare boundary, so no
    /// kernel path masks the deadline source or races a stop/clear sequence.
    pub(crate) unsafe fn set_period_us(period_us: u32) -> Result<(), LeaseError> {
        if period_us == 0 {
            return Err(LeaseError::Unsupported);
        }
        PENDING_PERIOD_US.store(period_us, Ordering::Release);
        Ok(())
    }

    pub fn enable_irq() {
        unsafe {
            let mut core = cortex_m::Peripherals::steal();
            core.NVIC.set_priority(nrf52840_pac::Interrupt::TIMER1, 0);
            NVIC::unmask(nrf52840_pac::Interrupt::TIMER1);
        }
    }

    pub fn on_isr() {
        unsafe {
            let t = TIMER1::ptr();
            if (*t).events_compare[0].read().bits() != 0 {
                (*t).events_compare[0].reset();
                let period_us = PENDING_PERIOD_US.swap(0, Ordering::AcqRel);
                if period_us != 0 {
                    (*t).cc[0].write(|w| w.bits(period_us));
                }
            }
        }
    }
}
