//! Simulated radio RX using EGU event and PPI capture.

use core::sync::atomic::{AtomicU32, Ordering};

use nrf52840_pac::{EGU0, PPI, TIMER0};

use crate::timer::MicroTimer;

const PPI_CH: usize = 1;

static MAX_LATENCY_US: AtomicU32 = AtomicU32::new(0);
static LATENCY_SAMPLES: AtomicU32 = AtomicU32::new(0);

pub struct RadioRxSim;

impl RadioRxSim {
    /// # Safety
    /// Caller must own the EGU0 + PPI leases and call once; wires EGU0 to TIMER0
    /// capture as the radio-event stand-in.
    pub unsafe fn init() {
        Self::init_ppi();
    }

    unsafe fn init_ppi() {
        let egu = EGU0::ptr();
        let event = core::ptr::addr_of!((*egu).events_triggered[0]) as u32;
        let timer = TIMER0::ptr();
        let cap = core::ptr::addr_of!((*timer).tasks_capture[2]) as u32;
        let ppi = PPI::ptr();
        (*ppi).ch[PPI_CH].eep.write(|w| w.bits(event));
        (*ppi).ch[PPI_CH].tep.write(|w| w.bits(cap));
        (*ppi).chenset.write(|w| w.bits(1 << PPI_CH));
    }

    /// Fire EGU + PPI capture; return EGU to CAPTURE latency in microseconds.
    ///
    /// # Safety
    /// Requires a prior [`RadioRxSim::init`]; triggers live EGU0 tasks.
    pub unsafe fn trigger_and_latency_us() -> Option<u32> {
        (*EGU0::ptr()).tasks_trigger[0].write(|w| w.bits(1));
        for _ in 0..500 {
            if (*EGU0::ptr()).events_triggered[0].read().bits() != 0 {
                (*EGU0::ptr()).events_triggered[0].reset();
                let captured = (*TIMER0::ptr()).cc[2].read().bits();
                let now = MicroTimer::now_us() as u32;
                let latency = now.wrapping_sub(captured);
                LATENCY_SAMPLES.fetch_add(1, Ordering::AcqRel);
                let mut cur = MAX_LATENCY_US.load(Ordering::Relaxed);
                while latency > cur {
                    match MAX_LATENCY_US.compare_exchange_weak(
                        cur,
                        latency,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(v) => cur = v,
                    }
                }
                return Some(latency);
            }
        }
        None
    }

    pub fn latency_stats() -> (u32, u32) {
        (
            MAX_LATENCY_US.load(Ordering::Acquire),
            LATENCY_SAMPLES.load(Ordering::Acquire),
        )
    }
}
