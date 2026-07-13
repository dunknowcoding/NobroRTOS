//! Deadline-programmed System-ON sleep using the owned nRF52840 TIMER0 clock.

use cortex_m::peripheral::NVIC;
use nobro_power::{PowerHookError, PowerMode, PowerPlatform};
use nrf52840_pac::TIMER0;

const COMPARE: usize = 3;

pub struct NrfTimerPower {
    residency_us: u64,
    entries: u32,
    wake_at: Option<u32>,
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
        (*timer).tasks_start.write(|w| w.bits(1));
        NVIC::unmask(nrf52840_pac::Interrupt::TIMER0);
        Self {
            residency_us: 0,
            entries: 0,
            wake_at: None,
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
    }
}

impl PowerPlatform for NrfTimerPower {
    fn program_wake(&mut self, deadline_us: Option<u64>) -> Result<(), PowerHookError> {
        let Some(deadline) = deadline_us else {
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
        }
        Ok(())
    }

    fn enter(&mut self, mode: PowerMode) -> Result<(), PowerHookError> {
        if mode == PowerMode::Active {
            return Ok(());
        }
        let start = Self::now_us() as u32;
        let mut slept = false;
        cortex_m::interrupt::free(|_| {
            let now = Self::now_us() as u32;
            if self
                .wake_at
                .is_some_and(|wake| wake.wrapping_sub(now) < 0x8000_0000 && wake != now)
            {
                slept = true;
                cortex_m::asm::wfi();
            }
        });
        let end = Self::now_us() as u32;
        unsafe {
            (*TIMER0::ptr()).intenclr.write(|w| w.compare3().set_bit());
        }
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
