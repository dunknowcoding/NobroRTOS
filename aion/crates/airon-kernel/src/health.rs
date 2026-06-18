//! Lightweight module health tracking and recovery decisions.

use crate::{Action, KernelError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleId {
    Kernel,
    Hal,
    Bus,
    Radio,
    Sensor,
    Actuator,
    Stream,
    Crypto,
    App(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HealthCounters {
    pub total_errors: u32,
    pub consecutive_errors: u16,
    pub last_error: Option<KernelError>,
    pub last_action: Action,
    pub last_seen_us: u64,
    pub last_recovery_us: u64,
}

impl HealthCounters {
    pub const fn zeroed() -> Self {
        Self {
            total_errors: 0,
            consecutive_errors: 0,
            last_error: None,
            last_action: Action::Ignore,
            last_seen_us: 0,
            last_recovery_us: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HealthSlot {
    pub module: ModuleId,
    pub counters: HealthCounters,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultThresholds {
    pub notify_after: u16,
    pub reboot_after: u16,
}

impl FaultThresholds {
    pub const DEFAULT: Self = Self {
        notify_after: 3,
        reboot_after: 8,
    };
}

pub struct HealthMonitor<const N: usize> {
    slots: [Option<HealthSlot>; N],
}

impl<const N: usize> HealthMonitor<N> {
    pub const fn new() -> Self {
        Self { slots: [None; N] }
    }

    pub fn record_ok(&mut self, module: ModuleId, now_us: u64) {
        let Some(slot) = self.find_or_insert(module) else {
            return;
        };
        slot.counters.consecutive_errors = 0;
        slot.counters.last_seen_us = now_us;
    }

    pub fn record_error(
        &mut self,
        module: ModuleId,
        error: KernelError,
        now_us: u64,
        thresholds: FaultThresholds,
        policy: fn(&KernelError) -> Action,
    ) -> Action {
        let Some(slot) = self.find_or_insert(module) else {
            return Action::NotifyUserTask;
        };

        let consecutive = slot.counters.consecutive_errors.saturating_add(1);
        slot.counters.total_errors = slot.counters.total_errors.saturating_add(1);
        slot.counters.consecutive_errors = consecutive;
        slot.counters.last_error = Some(error);
        slot.counters.last_seen_us = now_us;

        let action = if consecutive >= thresholds.reboot_after {
            slot.counters.last_recovery_us = now_us;
            Action::RebootModule
        } else if consecutive >= thresholds.notify_after {
            Action::NotifyUserTask
        } else {
            policy(&error)
        };

        slot.counters.last_action = action;
        action
    }

    pub fn get(&self, module: ModuleId) -> Option<HealthCounters> {
        self.slots
            .iter()
            .flatten()
            .find(|slot| slot.module == module)
            .map(|slot| slot.counters)
    }

    fn find_or_insert(&mut self, module: ModuleId) -> Option<&mut HealthSlot> {
        if let Some(idx) = self
            .slots
            .iter()
            .position(|slot| slot.map(|s| s.module == module).unwrap_or(false))
        {
            return self.slots[idx].as_mut();
        }

        if let Some(idx) = self.slots.iter().position(Option::is_none) {
            self.slots[idx] = Some(HealthSlot {
                module,
                counters: HealthCounters::zeroed(),
            });
            return self.slots[idx].as_mut();
        }

        None
    }
}

impl<const N: usize> Default for HealthMonitor<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::default_action;

    #[test]
    fn ok_resets_consecutive_errors() {
        let mut monitor = HealthMonitor::<2>::new();
        let thresholds = FaultThresholds::DEFAULT;

        monitor.record_error(
            ModuleId::Bus,
            KernelError::BusTimeout,
            10,
            thresholds,
            default_action,
        );
        monitor.record_ok(ModuleId::Bus, 20);

        let counters = monitor.get(ModuleId::Bus).expect("bus slot");
        assert_eq!(counters.total_errors, 1);
        assert_eq!(counters.consecutive_errors, 0);
        assert_eq!(counters.last_seen_us, 20);
    }

    #[test]
    fn repeated_errors_escalate_to_reboot() {
        let mut monitor = HealthMonitor::<1>::new();
        let thresholds = FaultThresholds {
            notify_after: 2,
            reboot_after: 3,
        };

        let first = monitor.record_error(
            ModuleId::Sensor,
            KernelError::SensorReadFail,
            10,
            thresholds,
            default_action,
        );
        let second = monitor.record_error(
            ModuleId::Sensor,
            KernelError::SensorReadFail,
            20,
            thresholds,
            default_action,
        );
        let third = monitor.record_error(
            ModuleId::Sensor,
            KernelError::SensorReadFail,
            30,
            thresholds,
            default_action,
        );

        assert_eq!(first, Action::Ignore);
        assert_eq!(second, Action::NotifyUserTask);
        assert_eq!(third, Action::RebootModule);
        assert_eq!(
            monitor
                .get(ModuleId::Sensor)
                .expect("sensor slot")
                .last_recovery_us,
            30
        );
    }

    #[test]
    fn full_monitor_reports_user_notification() {
        let mut monitor = HealthMonitor::<1>::new();
        monitor.record_ok(ModuleId::Bus, 1);

        let action = monitor.record_error(
            ModuleId::Radio,
            KernelError::RadioTxFail,
            2,
            FaultThresholds::DEFAULT,
            default_action,
        );

        assert_eq!(action, Action::NotifyUserTask);
        assert!(monitor.get(ModuleId::Radio).is_none());
    }
}
