//! Deadline-programmed System-ON sleep using the owned nRF52840 TIMER0 clock.

use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::peripheral::NVIC;
use nobro_power::{PowerHookError, PowerMode, PowerPlatform};
use nrf52840_pac::TIMER0;

const COMPARE: usize = 3;
const SCB_SCR_SEVONPEND: u32 = 1 << 4;
#[cfg(feature = "board-nicenano-s140")]
const TIMER0_PRIORITY_RAW: u8 = 2 << 5;
#[cfg(not(feature = "board-nicenano-s140"))]
const TIMER0_PRIORITY_RAW: u8 = 0;
static ARMED_READY: AtomicU32 = AtomicU32::new(0);
static PENDING_READY: AtomicU32 = AtomicU32::new(0);
static ARMED_DEADLINE: AtomicU32 = AtomicU32::new(0);
static PENDING_DEADLINE: AtomicU32 = AtomicU32::new(0);

pub struct NrfTimerPower {
    residency_us: u64,
    entries: u32,
    wake_at: Option<u32>,
    wake_latency_max_us: u32,
}

impl NrfTimerPower {
    /// # Safety
    /// The caller must exclusively own TIMER0 and its interrupt.
    pub unsafe fn init() -> Self {
        let timer = TIMER0::ptr();
        (*timer).tasks_stop.write(|w| w.bits(1));
        (*timer).tasks_clear.write(|w| w.bits(1));
        (*timer).mode.write(|w| w.mode().timer());
        (*timer).bitmode.write(|w| w.bitmode()._32bit());
        (*timer).prescaler.write(|w| w.prescaler().bits(4));
        (*timer).events_compare[COMPARE].reset();
        ARMED_READY.store(0, Ordering::Release);
        PENDING_READY.store(0, Ordering::Release);
        ARMED_DEADLINE.store(0, Ordering::Release);
        PENDING_DEADLINE.store(0, Ordering::Release);
        (*timer).tasks_start.write(|w| w.bits(1));
        // SEVONPEND closes the check-to-sleep race without PRIMASK: if the
        // compare becomes pending immediately before WFE, the event register
        // remains set even after the ISR runs and WFE returns instead of
        // sleeping past the release. S140-compatible builds use application
        // priority 2; priorities 0/1 remain reserved by the SoftDevice.
        let mut core = cortex_m::Peripherals::steal();
        core.SCB.scr.modify(|value| value | SCB_SCR_SEVONPEND);
        core.NVIC
            .set_priority(nrf52840_pac::Interrupt::TIMER0, TIMER0_PRIORITY_RAW);
        NVIC::unmask(nrf52840_pac::Interrupt::TIMER0);
        Self {
            residency_us: 0,
            entries: 0,
            wake_at: None,
            wake_latency_max_us: 0,
        }
    }

    pub fn now_us() -> u64 {
        unsafe {
            let timer = TIMER0::ptr();
            (*timer).tasks_capture[0].write(|w| w.bits(1));
            u64::from((*timer).cc[0].read().bits())
        }
    }

    pub const fn residency_us(&self) -> u64 {
        self.residency_us
    }

    pub const fn entries(&self) -> u32 {
        self.entries
    }

    pub fn on_interrupt() {
        unsafe {
            (*TIMER0::ptr()).events_compare[COMPARE].reset();
        }
        PENDING_READY.fetch_or(ARMED_READY.swap(0, Ordering::AcqRel), Ordering::AcqRel);
        PENDING_DEADLINE.store(ARMED_DEADLINE.swap(0, Ordering::AcqRel), Ordering::Release);
    }
}

impl PowerPlatform for NrfTimerPower {
    fn program_wake(&mut self, deadline_us: Option<u64>) -> Result<(), PowerHookError> {
        let Some(deadline) = deadline_us else {
            unsafe {
                let timer = TIMER0::ptr();
                (*timer).intenclr.write(|w| w.compare3().set_bit());
                (*timer).events_compare[COMPARE].reset();
            }
            ARMED_READY.store(0, Ordering::Release);
            ARMED_DEADLINE.store(0, Ordering::Release);
            self.wake_at = None;
            return Ok(());
        };
        unsafe {
            let timer = TIMER0::ptr();
            let now = Self::now_us() as u32;
            let requested = deadline as u32;
            let compare = if requested.wrapping_sub(now) < 0x8000_0000 && requested != now {
                requested
            } else {
                now.wrapping_add(2)
            };
            (*timer).events_compare[COMPARE].reset();
            (*timer).cc[COMPARE].write(|w| w.bits(compare));
            (*timer).intenset.write(|w| w.compare3().set_bit());
            self.wake_at = Some(compare);
            ARMED_DEADLINE.store(compare, Ordering::Release);
        }
        Ok(())
    }

    fn program_deadline_release(
        &mut self,
        deadline_us: Option<u64>,
        ready_mask: u32,
    ) -> Result<(), PowerHookError> {
        ARMED_READY.store(ready_mask, Ordering::Release);
        if let Err(error) = self.program_wake(deadline_us) {
            ARMED_READY.store(0, Ordering::Release);
            return Err(error);
        }
        Ok(())
    }

    fn take_deadline_releases(&mut self, now_us: u64) -> u32 {
        let ready = PENDING_READY.swap(0, Ordering::AcqRel);
        let deadline = PENDING_DEADLINE.swap(0, Ordering::AcqRel);
        if ready != 0 && deadline != 0 {
            self.wake_latency_max_us = self
                .wake_latency_max_us
                .max((now_us as u32).wrapping_sub(deadline));
        }
        ready
    }

    fn observed_wake_latency_us(&self) -> u32 {
        self.wake_latency_max_us
    }

    fn enter(&mut self, mode: PowerMode) -> Result<(), PowerHookError> {
        if mode == PowerMode::Active {
            return Ok(());
        }
        let start = Self::now_us() as u32;
        let mut slept = false;
        // Consume a stale event before deciding to sleep. Recheck both the
        // hardware deadline and ISR handoff afterward; SEVONPEND closes the
        // remaining check-to-WFE race.
        cortex_m::asm::sev();
        cortex_m::asm::wfe();
        cortex_m::asm::dsb();
        let now = Self::now_us() as u32;
        if self
            .wake_at
            .is_some_and(|wake| wake.wrapping_sub(now) < 0x8000_0000 && wake != now)
            && PENDING_READY.load(Ordering::Acquire) == 0
            && PENDING_DEADLINE.load(Ordering::Acquire) == 0
        {
            slept = true;
            cortex_m::asm::dsb();
            cortex_m::asm::wfe();
        }
        let end = Self::now_us() as u32;
        unsafe {
            (*TIMER0::ptr()).intenclr.write(|w| w.compare3().set_bit());
        }
        // A compare that became stale without firing must never release tasks
        // on a later interrupt. A fired compare already moved these bits.
        ARMED_READY.store(0, Ordering::Release);
        ARMED_DEADLINE.store(0, Ordering::Release);
        self.wake_at = None;
        if slept {
            self.residency_us = self
                .residency_us
                .wrapping_add(u64::from(end.wrapping_sub(start)));
            self.entries = self.entries.wrapping_add(1);
        }
        Ok(())
    }

    fn suspend(&mut self, _task_id: u16) -> Result<(), PowerHookError> {
        Ok(())
    }

    fn resume(&mut self, _task_id: u16) -> Result<(), PowerHookError> {
        Ok(())
    }
}

#[no_mangle]
#[allow(non_snake_case)]
unsafe extern "C" fn TIMER0() {
    NrfTimerPower::on_interrupt();
}
