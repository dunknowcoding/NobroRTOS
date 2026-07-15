//! Measured Cortex-M0+ critical-section provider.
//!
//! Cortex-M0+ has no BASEPRI, so shared-state exclusion necessarily uses
//! PRIMASK. Every outer section is timed with the free-running SysTick counter;
//! nested sections inherit the outer measurement. A counter wrap is reported
//! as an unbounded interval rather than being mistaken for a short section.

use critical_section::{set_impl, Impl, RawRestoreState};
use portable_atomic::{AtomicBool, AtomicU32, Ordering};

const SYST_CSR: *mut u32 = 0xE000_E010 as *mut u32;
const SYST_RVR: *mut u32 = 0xE000_E014 as *mut u32;
const SYST_CVR: *mut u32 = 0xE000_E018 as *mut u32;
const SYST_ENABLE_CLKSOURCE: u32 = (1 << 2) | 1;
const SYST_COUNTFLAG: u32 = 1 << 16;
const SYST_MASK: u32 = 0x00FF_FFFF;

static OUTER_START: AtomicU32 = AtomicU32::new(0);
static MAX_MASKED_CYCLES: AtomicU32 = AtomicU32::new(0);
static COUNTER_WRAPPED: AtomicBool = AtomicBool::new(false);

struct MeasuredPrimask;
set_impl!(MeasuredPrimask);

/// Start a 24-bit core-clock counter without enabling the SysTick interrupt.
/// Call once after the system clock is stable and before entering a section.
pub fn init() {
    unsafe {
        SYST_CSR.write_volatile(0);
        SYST_RVR.write_volatile(SYST_MASK);
        SYST_CVR.write_volatile(0);
        SYST_CSR.write_volatile(SYST_ENABLE_CLKSOURCE);
        let _ = SYST_CSR.read_volatile();
    }
}

pub fn max_masked_cycles() -> u32 {
    MAX_MASKED_CYCLES.load(Ordering::Acquire)
}

pub fn counter_wrapped() -> bool {
    COUNTER_WRAPPED.load(Ordering::Acquire)
}

pub fn max_masked_us_ceil(core_hz: u32) -> u32 {
    let cycles_per_us = core_hz / 1_000_000;
    if cycles_per_us == 0 {
        return u32::MAX;
    }
    max_masked_cycles().saturating_add(cycles_per_us - 1) / cycles_per_us
}

pub fn within_us(core_hz: u32, bound_us: u32) -> bool {
    !counter_wrapped() && max_masked_us_ceil(core_hz) <= bound_us
}

unsafe impl Impl for MeasuredPrimask {
    unsafe fn acquire() -> RawRestoreState {
        let restore_interrupts = cortex_m::register::primask::read().is_active();
        cortex_m::interrupt::disable();
        if restore_interrupts {
            // Reading CSR clears a stale COUNTFLAG before the outer interval.
            let _ = unsafe { SYST_CSR.read_volatile() };
            OUTER_START.store(
                unsafe { SYST_CVR.read_volatile() & SYST_MASK },
                Ordering::Relaxed,
            );
        }
        restore_interrupts
    }

    unsafe fn release(restore_interrupts: RawRestoreState) {
        if restore_interrupts {
            let end = unsafe { SYST_CVR.read_volatile() & SYST_MASK };
            let wrapped = unsafe { SYST_CSR.read_volatile() & SYST_COUNTFLAG != 0 };
            let elapsed = OUTER_START.load(Ordering::Relaxed).wrapping_sub(end) & SYST_MASK;
            if wrapped {
                COUNTER_WRAPPED.store(true, Ordering::Release);
                MAX_MASKED_CYCLES.store(SYST_MASK, Ordering::Release);
            } else if elapsed > MAX_MASKED_CYCLES.load(Ordering::Relaxed) {
                MAX_MASKED_CYCLES.store(elapsed, Ordering::Release);
            }
            unsafe {
                cortex_m::interrupt::enable();
            }
        }
    }
}
