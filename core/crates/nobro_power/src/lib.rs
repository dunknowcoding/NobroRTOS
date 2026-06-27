//! No-heap power management policy (M62): pick a sleep mode from activity + a deadline,
//! and track an active-time duty budget. Pure policy; the HAL applies the mode.
#![cfg_attr(not(test), no_std)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerMode {
    Active,    // CPU running
    Idle,      // WFE/WFI, peripherals on
    LowPower,  // peripherals gated, RTC wake
    Off,       // deepest sleep until external wake
}

/// Chooses a power mode and enforces an active-time duty budget over a window.
pub struct PowerManager {
    active_us: u64,
    window_us: u64,
    budget_us: u64,
}

impl PowerManager {
    /// `budget_us` of active time allowed per `window_us`.
    pub const fn new(window_us: u64, budget_us: u64) -> Self {
        Self { active_us: 0, window_us, budget_us }
    }

    /// Pick a mode: if work is pending choose Active; else sleep as deeply as the next
    /// deadline allows (short -> Idle, longer -> LowPower, none -> Off).
    pub fn select(&self, work_pending: bool, next_deadline_us: Option<u64>) -> PowerMode {
        if work_pending {
            return PowerMode::Active;
        }
        match next_deadline_us {
            None => PowerMode::Off,
            Some(d) if d < 2_000 => PowerMode::Idle,
            Some(_) => PowerMode::LowPower,
        }
    }

    /// Account active time; returns true if the duty budget for the window is exceeded
    /// (caller should back off / shed work).
    pub fn account_active(&mut self, dt_us: u64) -> bool {
        self.active_us = self.active_us.saturating_add(dt_us);
        self.active_us > self.budget_us
    }

    pub fn end_window(&mut self) {
        self.active_us = 0;
    }

    pub fn duty_milli(&self) -> u32 {
        if self.window_us == 0 {
            0
        } else {
            (self.active_us * 1000 / self.window_us) as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_mode_by_activity_and_deadline() {
        let pm = PowerManager::new(1_000_000, 100_000);
        assert_eq!(pm.select(true, Some(500)), PowerMode::Active);
        assert_eq!(pm.select(false, Some(500)), PowerMode::Idle);
        assert_eq!(pm.select(false, Some(50_000)), PowerMode::LowPower);
        assert_eq!(pm.select(false, None), PowerMode::Off);
    }

    #[test]
    fn enforces_duty_budget() {
        let mut pm = PowerManager::new(1_000_000, 100_000); // 10% duty
        assert!(!pm.account_active(80_000));
        assert!(pm.account_active(30_000)); // 110k > 100k budget -> exceeded
        assert_eq!(pm.duty_milli(), 110); // 11.0%
    }
}
