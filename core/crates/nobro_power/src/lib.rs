//! No-heap power management policy (M62): pick a sleep mode from activity + a deadline,
//! and track an active-time duty budget. Pure policy; the HAL applies the mode.
#![cfg_attr(not(test), no_std)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerMode {
    Active,   // CPU running
    Idle,     // WFE/WFI, peripherals on
    LowPower, // peripherals gated, RTC wake
    Off,      // deepest sleep until external wake
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerHookError {
    pub source: u16,
    pub code: u16,
}

/// Fallible board power operations owned by the authoritative executor.
pub trait PowerPlatform {
    fn program_wake(&mut self, deadline_us: Option<u64>) -> Result<(), PowerHookError>;
    fn enter(&mut self, mode: PowerMode) -> Result<(), PowerHookError>;
    fn suspend(&mut self, task_id: u16) -> Result<(), PowerHookError>;
    fn resume(&mut self, task_id: u16) -> Result<(), PowerHookError>;
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
        Self {
            active_us: 0,
            window_us,
            budget_us,
        }
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
        self.active_us
            .saturating_mul(1000)
            .checked_div(self.window_us)
            .unwrap_or(0)
            .min(u64::from(u32::MAX)) as u32
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

/// Per-task energy ledger (M161): charge each task's active time at a measured power
/// draw (uW) and report energy in uJ. Fixed capacity, no heap.
pub struct EnergyLedger<const N: usize> {
    entries: [(u16, u64); N], // (task id, energy uJ)
    len: usize,
}

impl<const N: usize> Default for EnergyLedger<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> EnergyLedger<N> {
    pub const fn new() -> Self {
        Self {
            entries: [(0, 0); N],
            len: 0,
        }
    }

    /// Charge `task` for `active_us` at `power_uw`. Returns false if the ledger is full.
    pub fn charge(&mut self, task: u16, active_us: u64, power_uw: u64) -> bool {
        let energy_uj = active_us.saturating_mul(power_uw) / 1_000_000;
        for e in self.entries[..self.len].iter_mut() {
            if e.0 == task {
                e.1 = e.1.saturating_add(energy_uj);
                return true;
            }
        }
        if self.len >= N {
            return false;
        }
        self.entries[self.len] = (task, energy_uj);
        self.len += 1;
        true
    }

    pub fn energy_uj(&self, task: u16) -> Option<u64> {
        self.entries[..self.len]
            .iter()
            .find(|e| e.0 == task)
            .map(|e| e.1)
    }

    pub fn total_uj(&self) -> u64 {
        self.entries[..self.len].iter().map(|e| e.1).sum()
    }

    /// The hungriest task (id, energy uJ).
    pub fn top(&self) -> Option<(u16, u64)> {
        self.entries[..self.len].iter().copied().max_by_key(|e| e.1)
    }
}

/// Executor-owned power policy, task power profiles, and measured energy ledger.
pub struct ExecutorPower<const N: usize> {
    manager: PowerManager,
    ledger: EnergyLedger<N>,
    profiles: [(u16, u64); N],
    profile_len: usize,
    default_power_uw: u64,
}

impl<const N: usize> ExecutorPower<N> {
    pub const fn new(window_us: u64, budget_us: u64, default_power_uw: u64) -> Self {
        Self {
            manager: PowerManager::new(window_us, budget_us),
            ledger: EnergyLedger::new(),
            profiles: [(0, 0); N],
            profile_len: 0,
            default_power_uw,
        }
    }

    pub fn set_task_power(&mut self, task_id: u16, power_uw: u64) -> bool {
        if let Some(profile) = self.profiles[..self.profile_len]
            .iter_mut()
            .find(|profile| profile.0 == task_id)
        {
            profile.1 = power_uw;
            return true;
        }
        if self.profile_len == N {
            return false;
        }
        self.profiles[self.profile_len] = (task_id, power_uw);
        self.profile_len += 1;
        true
    }

    pub fn account_task(&mut self, task_id: u16, active_us: u64) -> bool {
        let power_uw = self.profiles[..self.profile_len]
            .iter()
            .find(|profile| profile.0 == task_id)
            .map(|profile| profile.1)
            .unwrap_or(self.default_power_uw);
        let _ = self.manager.account_active(active_us);
        self.ledger.charge(task_id, active_us, power_uw)
    }

    pub fn apply_idle(
        &self,
        now_us: u64,
        work_pending: bool,
        deadline_us: Option<u64>,
        platform: &mut impl PowerPlatform,
    ) -> Result<PowerMode, PowerHookError> {
        let relative = deadline_us.map(|deadline| deadline.saturating_sub(now_us));
        let mode = self.manager.select(work_pending, relative);
        if mode != PowerMode::Active {
            platform.program_wake(deadline_us)?;
            platform.enter(mode)?;
        }
        Ok(mode)
    }

    pub const fn ledger(&self) -> &EnergyLedger<N> {
        &self.ledger
    }

    pub const fn manager(&self) -> &PowerManager {
        &self.manager
    }
}

#[cfg(test)]
mod energy_tests {
    use super::*;

    #[derive(Default)]
    struct Hooks {
        wake: Option<u64>,
        mode: Option<PowerMode>,
        suspended: Option<u16>,
    }

    impl PowerPlatform for Hooks {
        fn program_wake(&mut self, deadline_us: Option<u64>) -> Result<(), PowerHookError> {
            self.wake = deadline_us;
            Ok(())
        }
        fn enter(&mut self, mode: PowerMode) -> Result<(), PowerHookError> {
            self.mode = Some(mode);
            Ok(())
        }
        fn suspend(&mut self, task_id: u16) -> Result<(), PowerHookError> {
            self.suspended = Some(task_id);
            Ok(())
        }
        fn resume(&mut self, task_id: u16) -> Result<(), PowerHookError> {
            (self.suspended == Some(task_id))
                .then(|| self.suspended = None)
                .ok_or(PowerHookError { source: 1, code: 2 })
        }
    }

    #[test]
    fn ledger_charges_and_ranks_tasks() {
        let mut led = EnergyLedger::<4>::new();
        // sensor task: 200 ms at 5 mW -> 1000 uJ; radio: 50 ms at 40 mW -> 2000 uJ
        assert!(led.charge(1, 200_000, 5_000));
        assert!(led.charge(2, 50_000, 40_000));
        assert!(led.charge(1, 200_000, 5_000)); // accumulates
        assert_eq!(led.energy_uj(1), Some(2_000));
        assert_eq!(led.energy_uj(2), Some(2_000));
        assert_eq!(led.total_uj(), 4_000);
        assert!(led.charge(2, 1_000_000, 40_000)); // radio burns 40 mJ more
        assert_eq!(led.top(), Some((2, 42_000)));
    }

    #[test]
    fn executor_power_accounts_and_programs_wake_before_sleep() {
        let mut power = ExecutorPower::<2>::new(1_000_000, 100_000, 1_000);
        assert!(power.set_task_power(7, 5_000));
        assert!(power.account_task(7, 200_000));
        assert_eq!(power.ledger().energy_uj(7), Some(1_000));

        let mut hooks = Hooks::default();
        assert_eq!(
            power.apply_idle(10_000, false, Some(20_000), &mut hooks),
            Ok(PowerMode::LowPower)
        );
        assert_eq!(hooks.wake, Some(20_000));
        assert_eq!(hooks.mode, Some(PowerMode::LowPower));
    }
}

/// Adaptive sampling by battery level (M162): map a state-of-charge (percent) to a
/// decimation factor for the sensor pipeline - full rate when charged, progressively
/// heavier downsampling as the battery drains, minimum duty when critical.
///
/// Pairs with `nobro-sensor`'s `Decimator`: `Decimator::new(sampling_divisor(soc))`.
pub fn sampling_divisor(soc_percent: u8) -> u16 {
    match soc_percent {
        60..=u8::MAX => 1, // full rate
        30..=59 => 2,      // half rate
        15..=29 => 4,      // quarter rate
        5..=14 => 8,       // eighth rate
        _ => 16,           // critical: minimum duty
    }
}

#[cfg(test)]
mod adaptive_sampling_tests {
    use super::*;

    #[test]
    fn divisor_scales_with_soc_and_is_monotonic() {
        assert_eq!(sampling_divisor(100), 1);
        assert_eq!(sampling_divisor(60), 1);
        assert_eq!(sampling_divisor(45), 2);
        assert_eq!(sampling_divisor(20), 4);
        assert_eq!(sampling_divisor(10), 8);
        assert_eq!(sampling_divisor(2), 16);
        // monotonic: lower charge never samples faster
        let mut last = 0u16;
        for soc in (0..=100u8).rev() {
            let d = sampling_divisor(soc);
            assert!(d >= last, "soc {soc}: divisor {d} < {last}");
            last = d;
        }
    }
}

/// Energy-harvest-aware scheduling (M163): decide the work budget for the next window
/// from harvested income vs. battery reserve. Energy-neutral operation: spend at most
/// (harvest income + an affordable battery draw that keeps SoC above the reserve floor).
pub fn harvest_work_budget_uj(
    harvest_uw: u32,
    window_ms: u32,
    soc_percent: u8,
    reserve_floor_percent: u8,
    battery_capacity_uj: u64,
) -> u64 {
    let income_uj = u64::from(harvest_uw) * u64::from(window_ms) / 1000;
    if soc_percent <= reserve_floor_percent {
        // At/below the reserve: strictly energy-neutral (spend only what is harvested).
        return income_uj;
    }
    // Above the reserve: may additionally draw down toward the floor, rate-limited to
    // 1% of capacity per window so a burst cannot crater the battery.
    let above = u64::from(soc_percent - reserve_floor_percent);
    let draw_cap = battery_capacity_uj / 100;
    let affordable = (battery_capacity_uj * above / 100).min(draw_cap);
    income_uj + affordable
}

#[cfg(test)]
mod harvest_tests {
    use super::*;

    #[test]
    fn harvest_budget_is_neutral_at_floor_and_generous_above() {
        let cap = 10_000_000u64; // 10 J battery
        assert_eq!(harvest_work_budget_uj(5_000, 1_000, 20, 20, cap), 5_000);
        let b = harvest_work_budget_uj(5_000, 1_000, 80, 20, cap);
        assert_eq!(b, 5_000 + cap / 100);
        assert_eq!(harvest_work_budget_uj(0, 1_000, 15, 20, cap), 0);
    }
}

/// Duty-cycle scheduler (M159): drive a periodic task toward a target active fraction.
/// Each tick reports whether to run (active) or sleep, keeping the long-run active time
/// within the target duty using a leaky accumulator - robust to jittery tick spacing.
#[derive(Clone, Copy, Debug)]
pub struct DutyScheduler {
    target_milli: u32, // target duty in 1/1000
    credit: i64,       // accumulated "owed" active micros (signed)
    window_us: u64,
}

impl DutyScheduler {
    /// `target_duty_milli` in [0,1000]; `window_us` is the averaging horizon.
    pub const fn new(target_duty_milli: u32, window_us: u64) -> Self {
        Self {
            target_milli: target_duty_milli,
            credit: 0,
            window_us,
        }
    }

    /// Advance by `dt_us`; returns true if the task should be ACTIVE this interval.
    /// Accrues target active-time as credit, spends it when active, and leaks toward 0
    /// over the window so transient bursts do not bias the long-run duty.
    pub fn tick(&mut self, dt_us: u64, was_active: bool) -> bool {
        // accrue the target share of this interval
        self.credit += (dt_us as i64) * (self.target_milli as i64) / 1000;
        if was_active {
            self.credit -= dt_us as i64;
        }
        // leak toward zero across the window
        if self.window_us > 0 {
            self.credit -= self.credit * (dt_us as i64) / (self.window_us as i64) / 4;
        }
        // run when we owe active time
        self.credit > 0
    }
}

#[cfg(test)]
mod duty_tests {
    use super::*;

    #[test]
    fn duty_scheduler_converges_to_target() {
        // target 25% duty, 1 s window, 10 ms ticks over 10 s
        let mut ds = DutyScheduler::new(250, 1_000_000);
        let mut active_ticks = 0u32;
        let mut was_active = false;
        let total = 1000;
        for _ in 0..total {
            was_active = ds.tick(10_000, was_active);
            if was_active {
                active_ticks += 1;
            }
        }
        let duty = active_ticks * 1000 / total; // in milli
        assert!((200..=300).contains(&duty), "duty {duty} not near 250");
    }

    #[test]
    fn duty_zero_and_full() {
        let mut off = DutyScheduler::new(0, 1_000_000);
        let mut a = false;
        for _ in 0..100 {
            a = off.tick(10_000, a);
            assert!(!a);
        }
        let mut on = DutyScheduler::new(1000, 1_000_000);
        let mut a2 = false;
        let mut hi = 0;
        for _ in 0..100 {
            a2 = on.tick(10_000, a2);
            if a2 {
                hi += 1;
            }
        }
        assert!(hi > 90, "full-duty scheduler mostly active: {hi}/100");
    }
}
