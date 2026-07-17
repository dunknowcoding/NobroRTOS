//! Lightweight module health tracking and recovery decisions.

use crate::{Action, HealthFault, KernelError};

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
    Ai,
    App(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HealthCounters {
    pub total_errors: u32,
    pub consecutive_errors: u16,
    pub last_error: Option<KernelError>,
    pub last_fault: Option<HealthFault>,
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
            last_fault: None,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultThresholdError {
    NotifyZero,
    RebootBeforeNotify,
}

impl FaultThresholds {
    pub const DEFAULT: Self = Self {
        notify_after: 3,
        reboot_after: 8,
    };

    pub const fn validate(self) -> Result<(), FaultThresholdError> {
        if self.notify_after == 0 {
            Err(FaultThresholdError::NotifyZero)
        } else if self.reboot_after < self.notify_after {
            Err(FaultThresholdError::RebootBeforeNotify)
        } else {
            Ok(())
        }
    }
}

pub struct HealthMonitor<const N: usize> {
    // Keep the small occupancy key separate from the aligned counters so each
    // configured slot does not retain aggregate tail padding.
    modules: [Option<ModuleId>; N],
    counters: [HealthCounters; N],
}

/// Stateful, module-aware fault policy. Implementations may retain backoff/history and
/// inspect the complete source context plus updated counters.
pub trait FaultPolicy {
    fn decide(
        &mut self,
        module: ModuleId,
        fault: &HealthFault,
        counters: &HealthCounters,
    ) -> Action;
}

struct FunctionPolicy(fn(&KernelError) -> Action);

impl FaultPolicy for FunctionPolicy {
    fn decide(
        &mut self,
        _module: ModuleId,
        fault: &HealthFault,
        _counters: &HealthCounters,
    ) -> Action {
        (self.0)(&fault.error)
    }
}

impl<const N: usize> HealthMonitor<N> {
    pub const fn new() -> Self {
        Self {
            modules: [None; N],
            counters: [HealthCounters::zeroed(); N],
        }
    }

    /// Initialize caller-owned storage without materializing the complete slot
    /// array as a stack temporary.
    ///
    /// # Safety
    ///
    /// `destination` must be valid, aligned, writable storage for one
    /// uninitialized `HealthMonitor<N>`.
    pub(crate) unsafe fn init_in_place(destination: *mut Self) {
        let modules = core::ptr::addr_of_mut!((*destination).modules).cast::<Option<ModuleId>>();
        let counters = core::ptr::addr_of_mut!((*destination).counters).cast::<HealthCounters>();
        for index in 0..N {
            modules.add(index).write(None);
            counters.add(index).write(HealthCounters::zeroed());
        }
    }

    pub fn record_ok(&mut self, module: ModuleId, now_us: u64) {
        let Some(counters) = self.find_or_insert(module) else {
            return;
        };
        counters.consecutive_errors = 0;
        counters.last_seen_us = now_us;
    }

    pub fn record_error(
        &mut self,
        module: ModuleId,
        error: KernelError,
        now_us: u64,
        thresholds: FaultThresholds,
        policy: fn(&KernelError) -> Action,
    ) -> Action {
        self.record_fault(
            module,
            HealthFault::from_error(error),
            now_us,
            thresholds,
            &mut FunctionPolicy(policy),
        )
    }

    pub fn record_fault(
        &mut self,
        module: ModuleId,
        fault: HealthFault,
        now_us: u64,
        thresholds: FaultThresholds,
        policy: &mut impl FaultPolicy,
    ) -> Action {
        let Some(counters) = self.find_or_insert(module) else {
            return Action::NotifyUserTask;
        };

        let consecutive = counters.consecutive_errors.saturating_add(1);
        counters.total_errors = counters.total_errors.saturating_add(1);
        counters.consecutive_errors = consecutive;
        counters.last_error = Some(fault.error);
        counters.last_fault = Some(fault);
        counters.last_seen_us = now_us;

        let action = if consecutive >= thresholds.reboot_after {
            counters.last_recovery_us = now_us;
            Action::RebootModule
        } else if consecutive >= thresholds.notify_after {
            Action::NotifyUserTask
        } else {
            policy.decide(module, &fault, counters)
        };

        counters.last_action = action;
        action
    }

    pub fn get(&self, module: ModuleId) -> Option<HealthCounters> {
        self.modules
            .iter()
            .position(|candidate| *candidate == Some(module))
            .map(|index| self.counters[index])
    }

    fn find_or_insert(&mut self, module: ModuleId) -> Option<&mut HealthCounters> {
        if let Some(idx) = self
            .modules
            .iter()
            .position(|candidate| *candidate == Some(module))
        {
            return Some(&mut self.counters[idx]);
        }

        if let Some(idx) = self.modules.iter().position(Option::is_none) {
            self.modules[idx] = Some(module);
            return Some(&mut self.counters[idx]);
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
    use core::mem::MaybeUninit;

    struct StatefulPolicy {
        calls: u16,
    }

    impl FaultPolicy for StatefulPolicy {
        fn decide(
            &mut self,
            module: ModuleId,
            fault: &HealthFault,
            _counters: &HealthCounters,
        ) -> Action {
            self.calls += 1;
            if module == ModuleId::Bus && fault.context.detail0 == 0x52 {
                Action::RetryDelay(u32::from(self.calls) * 100)
            } else {
                Action::NotifyUserTask
            }
        }
    }

    #[test]
    fn fault_thresholds_reject_incoherent_escalation() {
        assert_eq!(
            FaultThresholds {
                notify_after: 0,
                reboot_after: 1,
            }
            .validate(),
            Err(FaultThresholdError::NotifyZero)
        );
        assert_eq!(
            FaultThresholds {
                notify_after: 3,
                reboot_after: 2,
            }
            .validate(),
            Err(FaultThresholdError::RebootBeforeNotify)
        );
        assert!(FaultThresholds::DEFAULT.validate().is_ok());
    }

    #[test]
    fn in_place_initialization_matches_const_constructor() {
        let mut storage = MaybeUninit::<HealthMonitor<2>>::uninit();
        unsafe {
            HealthMonitor::init_in_place(storage.as_mut_ptr());
        }
        let mut in_place = unsafe { storage.assume_init() };
        let mut by_value = HealthMonitor::<2>::new();

        for monitor in [&mut in_place, &mut by_value] {
            monitor.record_ok(ModuleId::Bus, 10);
            assert_eq!(
                monitor.record_error(
                    ModuleId::Sensor,
                    KernelError::SensorReadFail,
                    20,
                    FaultThresholds::DEFAULT,
                    default_action,
                ),
                Action::Ignore
            );
        }

        assert_eq!(in_place.get(ModuleId::Bus), by_value.get(ModuleId::Bus));
        assert_eq!(
            in_place.get(ModuleId::Sensor),
            by_value.get(ModuleId::Sensor)
        );
        assert_eq!(in_place.get(ModuleId::Radio), None);
    }

    #[test]
    fn structured_fault_preserves_context_and_uses_stateful_module_policy() {
        let mut monitor = HealthMonitor::<1>::new();
        let mut policy = StatefulPolicy { calls: 0 };
        let fault = HealthFault::new(
            KernelError::BusTimeout,
            crate::FaultContext::new(crate::FaultSource::Bus, 7, 0x52, 400),
        );
        assert_eq!(
            monitor.record_fault(
                ModuleId::Bus,
                fault,
                10,
                FaultThresholds {
                    notify_after: 3,
                    reboot_after: 5,
                },
                &mut policy,
            ),
            Action::RetryDelay(100)
        );
        assert_eq!(monitor.get(ModuleId::Bus).unwrap().last_fault, Some(fault));
    }

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
