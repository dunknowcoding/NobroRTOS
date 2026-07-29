//! RA4M1 ELC-to-GPT hardware timestamp capture.
//!
//! ELC software event 0 is linked to GPT2 input capture A. The CPU starts the
//! event, but the timestamp is latched by hardware before software observes it.
//! GPT2 is separate from GPT0 PWM and the GPT0/GPT1 event-DMA composition.

use core::sync::atomic::{AtomicU32, Ordering};

use nobro_hal::snapshots::EventCaptureSnapshot;
#[cfg(target_arch = "arm")]
use nobro_hal::HalEventCapture;
use nobro_hal::{LeaseError, LeaseId};

use crate::lease::{Ra4m1LeaseGuard, Ra4m1Leases};

const EVENT_OWNER_DEFAULT: u8 = 0xe2;
#[cfg(target_arch = "arm")]
const PCLKD_HZ: u32 = 48_000_000;
#[cfg(target_arch = "arm")]
const ELC_SOFTWARE_EVENT_0: u32 = 83;

#[cfg(target_arch = "arm")]
static MAX_LATENCY_US: AtomicU32 = AtomicU32::new(0);
static LATENCY_SAMPLES: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventCaptureError {
    Lease(LeaseError),
    NotCaptured,
}

/// Owned event-capture session. Dropping it restores the GPT/ELC route to idle.
pub struct Ra4m1EventCaptureSession {
    timer: Ra4m1LeaseGuard,
    event_router: Ra4m1LeaseGuard,
    software_event: Ra4m1LeaseGuard,
}

impl Ra4m1EventCaptureSession {
    /// Claim GPT2, ELC, and software-event ownership and configure the route.
    pub fn try_new(owner: u8) -> Result<Self, EventCaptureError> {
        let timer = Ra4m1Leases::acquire_guard(LeaseId::EVENT_CAPTURE_TIMER, owner)
            .map_err(EventCaptureError::Lease)?;
        let event_router = Ra4m1Leases::acquire_guard(LeaseId::EVENT_ROUTER, owner)
            .map_err(EventCaptureError::Lease)?;
        let software_event = Ra4m1Leases::acquire_guard(LeaseId::SOFTWARE_EVENT, owner)
            .map_err(EventCaptureError::Lease)?;
        #[cfg(target_arch = "arm")]
        // SAFETY: all three physical resources are held by this session.
        unsafe {
            Ra4m1EventCapture::init()
        };
        Ok(Self {
            timer,
            event_router,
            software_event,
        })
    }

    pub fn new() -> Result<Self, EventCaptureError> {
        Self::try_new(EVENT_OWNER_DEFAULT)
    }

    pub fn trigger_and_latency_us(&self) -> Result<u32, EventCaptureError> {
        self.timer.ensure_live().map_err(EventCaptureError::Lease)?;
        self.event_router
            .ensure_live()
            .map_err(EventCaptureError::Lease)?;
        self.software_event
            .ensure_live()
            .map_err(EventCaptureError::Lease)?;
        #[cfg(target_arch = "arm")]
        {
            // SAFETY: the live guards prove the route still belongs to this session.
            return unsafe { Ra4m1EventCapture::trigger_and_latency_us() }
                .ok_or(EventCaptureError::NotCaptured);
        }
        #[cfg(not(target_arch = "arm"))]
        Err(EventCaptureError::NotCaptured)
    }

    pub fn snapshot(&self) -> Result<EventCaptureSnapshot, EventCaptureError> {
        self.timer.ensure_live().map_err(EventCaptureError::Lease)?;
        self.event_router
            .ensure_live()
            .map_err(EventCaptureError::Lease)?;
        self.software_event
            .ensure_live()
            .map_err(EventCaptureError::Lease)?;
        #[cfg(target_arch = "arm")]
        {
            // SAFETY: this composition exposes one channel and the guards are live.
            return Ok(unsafe { Ra4m1EventCapture::capture_snapshot(0) });
        }
        #[cfg(not(target_arch = "arm"))]
        Ok(EventCaptureSnapshot {
            channel_enabled: false,
            source_wired: false,
            sink_wired: false,
        })
    }
}

impl Drop for Ra4m1EventCaptureSession {
    fn drop(&mut self) {
        #[cfg(target_arch = "arm")]
        unsafe {
            write32(GPT2 + GTSTP, 1 << 2);
            write32(GPT2 + GTWP, GPT_WRITE_UNLOCKED);
            write32(GPT2 + GTICASR, 0);
            write32(GPT2 + GTWP, GPT_WRITE_PROTECTED);
            write16(ELC + ELSR2, 0);
        }
    }
}

pub struct Ra4m1EventCapture;

#[cfg(target_arch = "arm")]
impl HalEventCapture for Ra4m1EventCapture {
    unsafe fn init() {
        start_gpt2_module();
        write32(GPT2 + GTSTP, 1 << 2);
        write32(GPT2 + GTWP, GPT_WRITE_UNLOCKED);
        write32(GPT2 + GTCR, 0);
        write32(GPT2 + GTUDDTYC, 3);
        write32(GPT2 + GTUDDTYC, 1);
        write32(GPT2 + GTICASR, GTICASR_ELC_CAPTURE_A);
        write32(GPT2 + GTPR, u16::MAX as u32);
        write32(GPT2 + GTCNT, 0);
        write32(GPT2 + GTCLR, 1 << 2);
        write32(GPT2 + GTWP, GPT_WRITE_PROTECTED);

        write16(ELC + ELSR2, ELC_SOFTWARE_EVENT_0 as u16);
        write8(ELC + ELCR, read8(ELC + ELCR) | ELC_ENABLE);
        write32(GPT2 + GTSTR, 1 << 2);
    }

    unsafe fn trigger_and_latency_us() -> Option<u32> {
        let before = read32(GPT2 + GTCNT) as u16;
        write8(ELC + ELSEGR0, ELSEGR_WRITE_AND_GENERATE);
        let captured = read32(GPT2 + GTCCRA) as u16;
        let ticks = captured.wrapping_sub(before) as u32;
        let latency_us = ticks.saturating_mul(1_000_000) / PCLKD_HZ;
        increment_samples();
        MAX_LATENCY_US.fetch_max(latency_us, Ordering::AcqRel);
        Some(latency_us)
    }

    fn latency_stats() -> (u32, u32) {
        (
            MAX_LATENCY_US.load(Ordering::Acquire),
            LATENCY_SAMPLES.load(Ordering::Acquire),
        )
    }

    unsafe fn capture_snapshot(channel: usize) -> EventCaptureSnapshot {
        if channel != 0 {
            return EventCaptureSnapshot {
                channel_enabled: false,
                source_wired: false,
                sink_wired: false,
            };
        }
        EventCaptureSnapshot {
            channel_enabled: read8(ELC + ELCR) & ELC_ENABLE != 0,
            source_wired: u32::from(read16(ELC + ELSR2)) & 0x1ff == ELC_SOFTWARE_EVENT_0,
            sink_wired: read32(GPT2 + GTICASR) & GTICASR_ELC_CAPTURE_A != 0,
        }
    }
}

fn increment_samples() {
    let _ = LATENCY_SAMPLES.fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
        Some(value.saturating_add(1))
    });
}

#[cfg(target_arch = "arm")]
const PRCR: usize = 0x4001_E3FE;
#[cfg(target_arch = "arm")]
const MSTPCRE: usize = 0x4004_700c;
#[cfg(target_arch = "arm")]
const GPT2_MSTP: u32 = 1 << 29;
#[cfg(target_arch = "arm")]
const GPT2: usize = 0x4007_8200;
#[cfg(target_arch = "arm")]
const GTWP: usize = 0x00;
#[cfg(target_arch = "arm")]
const GTSTR: usize = 0x04;
#[cfg(target_arch = "arm")]
const GTSTP: usize = 0x08;
#[cfg(target_arch = "arm")]
const GTCLR: usize = 0x0c;
#[cfg(target_arch = "arm")]
const GTICASR: usize = 0x24;
#[cfg(target_arch = "arm")]
const GTCR: usize = 0x2c;
#[cfg(target_arch = "arm")]
const GTUDDTYC: usize = 0x30;
#[cfg(target_arch = "arm")]
const GTCNT: usize = 0x48;
#[cfg(target_arch = "arm")]
const GTCCRA: usize = 0x4c;
#[cfg(target_arch = "arm")]
const GTPR: usize = 0x64;
#[cfg(target_arch = "arm")]
const GTICASR_ELC_CAPTURE_A: u32 = 1 << 16;
#[cfg(target_arch = "arm")]
const GPT_WRITE_UNLOCKED: u32 = 0xa500;
#[cfg(target_arch = "arm")]
const GPT_WRITE_PROTECTED: u32 = 0xa501;

#[cfg(target_arch = "arm")]
const ELC: usize = 0x4004_1000;
#[cfg(target_arch = "arm")]
const ELCR: usize = 0x00;
#[cfg(target_arch = "arm")]
const ELSEGR0: usize = 0x02;
#[cfg(target_arch = "arm")]
const ELSR2: usize = 0x18;
#[cfg(target_arch = "arm")]
const ELC_ENABLE: u8 = 1 << 7;
#[cfg(target_arch = "arm")]
const ELSEGR_WRITE_AND_GENERATE: u8 = (1 << 6) | 1;

#[cfg(target_arch = "arm")]
unsafe fn start_gpt2_module() {
    let prior = read16(PRCR) & 0x0003;
    write16(PRCR, 0xa502);
    write32(MSTPCRE, read32(MSTPCRE) & !GPT2_MSTP);
    write16(PRCR, 0xa500 | prior);
}

#[cfg(target_arch = "arm")]
unsafe fn read8(address: usize) -> u8 {
    (address as *const u8).read_volatile()
}

#[cfg(target_arch = "arm")]
unsafe fn write8(address: usize, value: u8) {
    (address as *mut u8).write_volatile(value);
}

#[cfg(target_arch = "arm")]
unsafe fn read16(address: usize) -> u16 {
    (address as *const u16).read_volatile()
}

#[cfg(target_arch = "arm")]
unsafe fn write16(address: usize, value: u16) {
    (address as *mut u16).write_volatile(value);
}

#[cfg(target_arch = "arm")]
unsafe fn read32(address: usize) -> u32 {
    (address as *const u32).read_volatile()
}

#[cfg(target_arch = "arm")]
unsafe fn write32(address: usize, value: u32) {
    (address as *mut u32).write_volatile(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use nobro_hal::HalLease;

    #[test]
    fn session_owns_all_route_resources_and_releases_them() {
        let _lock = crate::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let session = Ra4m1EventCaptureSession::try_new(44).unwrap();
        assert!(Ra4m1Leases::is_held(LeaseId::EVENT_CAPTURE_TIMER));
        assert!(Ra4m1Leases::is_held(LeaseId::EVENT_ROUTER));
        assert!(Ra4m1Leases::is_held(LeaseId::SOFTWARE_EVENT));
        drop(session);
        assert!(!Ra4m1Leases::is_held(LeaseId::EVENT_CAPTURE_TIMER));
        assert!(!Ra4m1Leases::is_held(LeaseId::EVENT_ROUTER));
        assert!(!Ra4m1Leases::is_held(LeaseId::SOFTWARE_EVENT));
    }

    #[test]
    fn latency_sample_count_saturates_instead_of_wrapping() {
        let _lock = crate::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        LATENCY_SAMPLES.store(u32::MAX, Ordering::Release);
        increment_samples();
        assert_eq!(LATENCY_SAMPLES.load(Ordering::Acquire), u32::MAX);
        LATENCY_SAMPLES.store(0, Ordering::Release);
    }
}
