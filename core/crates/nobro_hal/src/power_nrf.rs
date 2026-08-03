//! Deadline-programmed System-ON sleep using the owned nRF52840 TIMER0 clock.

use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::peripheral::NVIC;
use nobro_power::{DeadlineTimingProfile, PowerHookError, PowerMode, PowerPlatform, SleepProfile};
use nrf52840_pac::TIMER0;

const COMPARE: usize = 3;
const SCB_SCR_SEVONPEND: u32 = 1 << 4;
#[cfg(feature = "board-promicro-s140")]
const TIMER0_PRIORITY_RAW: u8 = 2 << 5;
#[cfg(not(feature = "board-promicro-s140"))]
const TIMER0_PRIORITY_RAW: u8 = 0;
static ARMED_READY: AtomicU32 = AtomicU32::new(0);
static PENDING_READY: AtomicU32 = AtomicU32::new(0);
static ARMED_DEADLINE: AtomicU32 = AtomicU32::new(0);
static ARMED_DEADLINE_VALID: AtomicU32 = AtomicU32::new(0);
static PENDING_DEADLINE: AtomicU32 = AtomicU32::new(0);
static PENDING_DEADLINE_VALID: AtomicU32 = AtomicU32::new(0);

pub struct NrfTimerPower {
    residency_us: u64,
    entries: u32,
    wake_at: Option<u32>,
    wake_latency_max_us: u32,
    deadline_timing: Option<DeadlineTimingProfile>,
}

impl NrfTimerPower {
    pub const DEADLINE_PROVIDER_ID: u16 = 0x4e54;
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
        ARMED_DEADLINE_VALID.store(0, Ordering::Release);
        PENDING_DEADLINE.store(0, Ordering::Release);
        PENDING_DEADLINE_VALID.store(0, Ordering::Release);
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
            deadline_timing: None,
        }
    }

    /// Install an exact, externally qualified timing bound. Qualification is
    /// deliberately separate from register initialization: builds cannot
    /// inherit a high-resolution claim merely by selecting this peripheral.
    pub fn qualify_deadline_timing(
        &mut self,
        profile: DeadlineTimingProfile,
    ) -> Result<(), PowerHookError> {
        if !profile.is_valid()
            || profile.provider_id != Self::DEADLINE_PROVIDER_ID
            || profile.minimum_period_us < 2
            || profile.resolution_us != 1
        {
            return Err(PowerHookError {
                source: Self::DEADLINE_PROVIDER_ID,
                code: 1,
            });
        }
        self.deadline_timing = Some(profile);
        Ok(())
    }

    pub fn clear_deadline_timing_qualification(&mut self) {
        self.deadline_timing = None;
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
        if ARMED_DEADLINE_VALID.swap(0, Ordering::AcqRel) != 0 {
            PENDING_DEADLINE.store(ARMED_DEADLINE.load(Ordering::Acquire), Ordering::Release);
            PENDING_DEADLINE_VALID.store(1, Ordering::Release);
        }
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
            ARMED_DEADLINE_VALID.store(0, Ordering::Release);
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
            ARMED_DEADLINE_VALID.store(1, Ordering::Release);
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
        let deadline_valid = PENDING_DEADLINE_VALID.swap(0, Ordering::AcqRel) != 0;
        let deadline = PENDING_DEADLINE.load(Ordering::Acquire);
        if deadline_valid {
            self.wake_latency_max_us = self
                .wake_latency_max_us
                .max((now_us as u32).wrapping_sub(deadline));
            if self
                .deadline_timing
                .is_some_and(|profile| self.wake_latency_max_us > profile.wake_latency_us)
            {
                // A measured bound violation withdraws qualification instead
                // of letting later admissions reuse evidence that is false.
                self.deadline_timing = None;
            }
        }
        ready
    }

    fn observed_wake_latency_us(&self) -> u32 {
        self.wake_latency_max_us
    }

    fn deadline_timing_profile(&self) -> Option<DeadlineTimingProfile> {
        self.deadline_timing
    }

    fn sleep_profile(&self, mode: PowerMode) -> Option<SleepProfile> {
        let timing = self.deadline_timing?;
        (mode == PowerMode::Idle && timing.wake_latency_us != 0).then_some(SleepProfile {
            provider_id: timing.provider_id,
            generation: timing.generation,
            deepest_mode: PowerMode::Idle,
            wake_latency_us: timing.wake_latency_us,
            wake_sources: 1 << 0,                // owned TIMER0 compare
            retained_state: 1 << 0,              // System-ON RAM
            retained_clock_domains: 1 << 0,      // TIMER0 1 MHz timebase
            retained_peripheral_domains: 1 << 0, // TIMER0 compare/IRQ route
        })
    }

    fn constrain_mode(&self, requested: PowerMode) -> PowerMode {
        // TIMER0 continues running only in System-ON sleep. This backend must
        // never advertise LowPower/SYSTEMOFF merely because policy requested
        // it; board-specific retained wake and restoration are separate
        // providers that can opt into those modes.
        requested.shallower(PowerMode::Idle)
    }

    fn enter(&mut self, mode: PowerMode) -> Result<PowerMode, PowerHookError> {
        if mode == PowerMode::Active {
            return Ok(PowerMode::Active);
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
            && PENDING_DEADLINE_VALID.load(Ordering::Acquire) == 0
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
        ARMED_DEADLINE_VALID.store(0, Ordering::Release);
        self.wake_at = None;
        if slept {
            self.residency_us = self
                .residency_us
                .saturating_add(u64::from(end.wrapping_sub(start)));
            self.entries = self.entries.saturating_add(1);
        }
        Ok(PowerMode::Idle)
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
