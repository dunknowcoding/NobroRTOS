//! Liveness task supervision (M166): the escalation layer above [`crate::Watchdog`].
//!
//! The error-driven [`crate::Supervisor`] reacts to *reported* failures; this one reacts
//! to *silence*. Each poll in which a registered task stays past its check-in deadline
//! adds a strike, strikes escalate the response (Restart -> Degrade -> Reboot), and a
//! task that checks in again while below the reboot threshold recovers its strike
//! count. Fixed capacity, no heap.

use crate::watchdog::{Watchdog, WatchdogError};
use crate::ModuleId;

/// Escalating responses to repeated missed check-ins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupervisionAction {
    /// All registered tasks are checking in.
    Healthy,
    /// First miss threshold: restart the module.
    Restart(ModuleId),
    /// Repeated misses: run the module degraded.
    Degrade(ModuleId),
    /// Persistent misses: reboot the module (last resort).
    Reboot(ModuleId),
}

impl SupervisionAction {
    fn severity(self) -> u8 {
        match self {
            SupervisionAction::Healthy => 0,
            SupervisionAction::Restart(_) => 1,
            SupervisionAction::Degrade(_) => 2,
            SupervisionAction::Reboot(_) => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Strikes {
    module: Option<ModuleId>,
    count: u8,
}

/// Watchdog + escalation policy: `restart_at`/`degrade_at`/`reboot_at` are strike
/// thresholds in consecutive missed polls.
pub struct TaskSupervisor<const N: usize> {
    watchdog: Watchdog<N>,
    strikes: [Strikes; N],
    pub restart_at: u8,
    pub degrade_at: u8,
    pub reboot_at: u8,
}

impl<const N: usize> TaskSupervisor<N> {
    pub const fn new(restart_at: u8, degrade_at: u8, reboot_at: u8) -> Self {
        Self {
            watchdog: Watchdog::new(),
            strikes: [Strikes {
                module: None,
                count: 0,
            }; N],
            restart_at,
            degrade_at,
            reboot_at,
        }
    }

    pub fn register(
        &mut self,
        module: ModuleId,
        interval_us: u64,
        now_us: u64,
    ) -> Result<(), WatchdogError> {
        self.watchdog.register(module, interval_us, now_us)?;
        if let Some(slot) = self.strikes.iter_mut().find(|s| s.module.is_none()) {
            *slot = Strikes {
                module: Some(module),
                count: 0,
            };
        }
        Ok(())
    }

    /// A task checks in: feeds its watchdog entry and (below the reboot threshold)
    /// clears its strikes - the task has recovered.
    pub fn checkin(&mut self, module: ModuleId, now_us: u64) -> Result<(), WatchdogError> {
        self.watchdog.beat(module, now_us)?;
        if let Some(s) = self.strikes.iter_mut().find(|s| s.module == Some(module)) {
            if s.count < self.reboot_at {
                s.count = 0;
            }
        }
        Ok(())
    }

    pub fn strikes(&self, module: ModuleId) -> u8 {
        self.strikes
            .iter()
            .find(|s| s.module == Some(module))
            .map(|s| s.count)
            .unwrap_or(0)
    }

    /// Poll: every currently-expired module gains a strike; the most severe indicated
    /// action across modules is returned (Reboot > Degrade > Restart > Healthy).
    pub fn poll(&mut self, now_us: u64) -> SupervisionAction {
        let mut expired = [ModuleId::Kernel; N];
        let n = self.watchdog.expired(now_us, &mut expired);
        let mut worst = SupervisionAction::Healthy;
        for &module in expired.iter().take(n) {
            let count = {
                let Some(s) = self.strikes.iter_mut().find(|s| s.module == Some(module)) else {
                    continue;
                };
                s.count = s.count.saturating_add(1);
                s.count
            };
            let action = if count >= self.reboot_at {
                SupervisionAction::Reboot(module)
            } else if count >= self.degrade_at {
                SupervisionAction::Degrade(module)
            } else if count >= self.restart_at {
                SupervisionAction::Restart(module)
            } else {
                SupervisionAction::Healthy
            };
            if action.severity() > worst.severity() {
                worst = action;
            }
        }
        worst
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000;

    #[test]
    fn healthy_while_tasks_check_in() {
        let mut s = TaskSupervisor::<4>::new(1, 3, 5);
        s.register(ModuleId::Sensor, 10 * MS, 0).unwrap();
        s.register(ModuleId::Radio, 10 * MS, 0).unwrap();
        for t in 1..=10u64 {
            s.checkin(ModuleId::Sensor, t * 5 * MS).unwrap();
            s.checkin(ModuleId::Radio, t * 5 * MS).unwrap();
            assert_eq!(s.poll(t * 5 * MS), SupervisionAction::Healthy);
        }
    }

    #[test]
    fn misses_escalate_restart_degrade_reboot() {
        let mut s = TaskSupervisor::<2>::new(1, 3, 5);
        s.register(ModuleId::Sensor, 10 * MS, 0).unwrap();
        // The sensor goes silent: each poll past its deadline escalates.
        assert_eq!(s.poll(11 * MS), SupervisionAction::Restart(ModuleId::Sensor));
        assert_eq!(s.poll(12 * MS), SupervisionAction::Restart(ModuleId::Sensor));
        assert_eq!(s.poll(13 * MS), SupervisionAction::Degrade(ModuleId::Sensor));
        assert_eq!(s.poll(14 * MS), SupervisionAction::Degrade(ModuleId::Sensor));
        assert_eq!(s.poll(15 * MS), SupervisionAction::Reboot(ModuleId::Sensor));
        assert_eq!(s.strikes(ModuleId::Sensor), 5);
    }

    #[test]
    fn checkin_recovers_strikes() {
        let mut s = TaskSupervisor::<2>::new(1, 3, 5);
        s.register(ModuleId::Sensor, 10 * MS, 0).unwrap();
        assert_eq!(s.poll(11 * MS), SupervisionAction::Restart(ModuleId::Sensor));
        assert_eq!(s.strikes(ModuleId::Sensor), 1);
        s.checkin(ModuleId::Sensor, 12 * MS).unwrap(); // it comes back
        assert_eq!(s.strikes(ModuleId::Sensor), 0);
        assert_eq!(s.poll(13 * MS), SupervisionAction::Healthy);
    }

    #[test]
    fn worst_action_wins_across_modules() {
        let mut s = TaskSupervisor::<4>::new(1, 3, 5);
        s.register(ModuleId::Sensor, 10 * MS, 0).unwrap();
        s.register(ModuleId::Radio, 10 * MS, 0).unwrap();
        // Both miss once; the radio then recovers while the sensor stays silent.
        let _ = s.poll(11 * MS);
        s.checkin(ModuleId::Radio, 12 * MS).unwrap();
        let _ = s.poll(13 * MS); // sensor strike 2
        let _ = s.poll(14 * MS); // sensor strike 3 -> Degrade territory
        // At 23 ms the radio (silent since 12 ms) has expired again with 1 strike
        // (Restart) while the sensor is at 4 strikes (Degrade): Degrade must win.
        assert_eq!(s.poll(23 * MS), SupervisionAction::Degrade(ModuleId::Sensor));
        assert_eq!(s.strikes(ModuleId::Radio), 1);
    }
}
