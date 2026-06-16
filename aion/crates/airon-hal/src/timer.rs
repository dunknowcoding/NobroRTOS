//! TIMER0 absolute microsecond clock (1 MHz tick, PRESC=4 @ 16 MHz).

use core::sync::atomic::{AtomicU32, Ordering};

use nrf52840_pac::TIMER0;

const PRESCALER: u32 = 4;

static OVERFLOW: AtomicU32 = AtomicU32::new(0);

pub struct MicroTimer;

impl MicroTimer {
    pub unsafe fn init() {
        let t = TIMER0::ptr();
        (*t).tasks_stop.write(|w| w.bits(1));
        (*t).tasks_clear.write(|w| w.bits(1));
        (*t).mode.write(|w| w.mode().timer());
        (*t).bitmode.write(|w| w.bitmode()._32bit());
        (*t).prescaler.write(|w| w.prescaler().bits(PRESCALER as u8));
        OVERFLOW.store(0, Ordering::Release);
        (*t).tasks_start.write(|w| w.bits(1));
    }

    pub fn on_overflow() {
        OVERFLOW.fetch_add(1, Ordering::AcqRel);
    }

    /// Latch running counter into CC[0] and read (1 µs resolution).
    pub fn now_us() -> u64 {
        unsafe {
            let t = TIMER0::ptr();
            (*t).tasks_capture[0].write(|w| w.bits(1));
            let lo = (*t).cc[0].read().bits() as u64;
            let hi = OVERFLOW.load(Ordering::Acquire) as u64;
            (hi << 32) | lo
        }
    }

    pub fn captured_cc1_us() -> u32 {
        unsafe { (*TIMER0::ptr()).cc[1].read().bits() }
    }

    pub fn captured_cc2_us() -> u32 {
        unsafe { (*TIMER0::ptr()).cc[2].read().bits() }
    }

    pub unsafe fn trigger_capture_1() {
        (*TIMER0::ptr()).tasks_capture[1].write(|w| w.bits(1));
    }
}
