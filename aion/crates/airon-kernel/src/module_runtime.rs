//! Fixed-slot module runtime state tracking for recovery and low-power policy.

use crate::{Action, ModuleId, RecoveryOutcome, StartupPlan};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleRunState {
    Registered,
    Active,
    Suspended,
    Faulted,
    Recovering,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleRuntimeEntry {
    pub module: ModuleId,
    pub state: ModuleRunState,
    pub fault_count: u32,
    pub recovery_count: u32,
    pub last_change_us: u64,
}

impl ModuleRuntimeEntry {
    pub const fn new(module: ModuleId, now_us: u64) -> Self {
        Self {
            module,
            state: ModuleRunState::Registered,
            fault_count: 0,
            recovery_count: 0,
            last_change_us: now_us,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleRuntimeError {
    Full,
    Duplicate(ModuleId),
    Missing(ModuleId),
    Disabled(ModuleId),
    InvalidTransition {
        module: ModuleId,
        from: ModuleRunState,
        to: ModuleRunState,
    },
}

pub struct ModuleRuntimeGuard<const N: usize> {
    entries: [Option<ModuleRuntimeEntry>; N],
}

impl<const N: usize> ModuleRuntimeGuard<N> {
    pub const fn new() -> Self {
        Self { entries: [None; N] }
    }

    pub fn try_from_startup_plan<const STARTUP: usize>(
        plan: &StartupPlan<STARTUP>,
    ) -> Result<Self, ModuleRuntimeError> {
        let mut guard = Self::new();
        guard.register_startup_plan(plan, 0)?;
        Ok(guard)
    }

    pub fn register(&mut self, module: ModuleId, now_us: u64) -> Result<(), ModuleRuntimeError> {
        if self.entry(module).is_some() {
            return Err(ModuleRuntimeError::Duplicate(module));
        }

        let Some(slot) = self.entries.iter_mut().find(|slot| slot.is_none()) else {
            return Err(ModuleRuntimeError::Full);
        };
        *slot = Some(ModuleRuntimeEntry::new(module, now_us));
        Ok(())
    }

    pub fn register_startup_plan<const STARTUP: usize>(
        &mut self,
        plan: &StartupPlan<STARTUP>,
        now_us: u64,
    ) -> Result<(), ModuleRuntimeError> {
        for module in plan.order.iter().copied().take(plan.len).flatten() {
            self.register(module, now_us)?;
        }
        Ok(())
    }

    pub fn activate_all(&mut self, now_us: u64) -> Result<(), ModuleRuntimeError> {
        for idx in 0..N {
            if let Some(entry) = self.entries[idx] {
                self.transition(entry.module, ModuleRunState::Active, now_us)?;
            }
        }
        Ok(())
    }

    pub fn suspend(&mut self, module: ModuleId, now_us: u64) -> Result<(), ModuleRuntimeError> {
        self.transition(module, ModuleRunState::Suspended, now_us)
    }

    pub fn resume(&mut self, module: ModuleId, now_us: u64) -> Result<(), ModuleRuntimeError> {
        self.transition(module, ModuleRunState::Active, now_us)
    }

    pub fn disable(&mut self, module: ModuleId, now_us: u64) -> Result<(), ModuleRuntimeError> {
        self.transition(module, ModuleRunState::Disabled, now_us)
    }

    pub fn note_recovery_outcome(
        &mut self,
        outcome: RecoveryOutcome,
        now_us: u64,
    ) -> Result<(), ModuleRuntimeError> {
        match outcome.action {
            Action::Ignore => self.transition(outcome.module, ModuleRunState::Active, now_us),
            Action::RetryNow | Action::RetryDelay(_) | Action::RebootModule => {
                self.mark_recovering(outcome.module, now_us)
            }
            Action::NotifyUserTask => self.mark_faulted(outcome.module, now_us),
        }
    }

    pub fn complete_recovery(
        &mut self,
        module: ModuleId,
        now_us: u64,
    ) -> Result<(), ModuleRuntimeError> {
        self.transition(module, ModuleRunState::Active, now_us)
    }

    pub fn state(&self, module: ModuleId) -> Option<ModuleRunState> {
        self.entry(module).map(|entry| entry.state)
    }

    pub fn count_state(&self, state: ModuleRunState) -> usize {
        self.entries
            .iter()
            .flatten()
            .filter(|entry| entry.state == state)
            .count()
    }

    pub fn latest_changed(&self) -> Option<ModuleRuntimeEntry> {
        let mut latest = None;
        for entry in self.entries.iter().flatten() {
            if latest
                .map(|current: ModuleRuntimeEntry| entry.last_change_us >= current.last_change_us)
                .unwrap_or(true)
            {
                latest = Some(*entry);
            }
        }
        latest
    }

    pub fn entry(&self, module: ModuleId) -> Option<ModuleRuntimeEntry> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.module == module)
            .copied()
    }

    pub fn len(&self) -> usize {
        self.entries.iter().flatten().count()
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn mark_faulted(&mut self, module: ModuleId, now_us: u64) -> Result<(), ModuleRuntimeError> {
        let entry = self.entry_mut(module)?;
        if entry.state == ModuleRunState::Disabled {
            return Err(ModuleRuntimeError::InvalidTransition {
                module,
                from: entry.state,
                to: ModuleRunState::Faulted,
            });
        }
        entry.state = ModuleRunState::Faulted;
        entry.fault_count = entry.fault_count.saturating_add(1);
        entry.last_change_us = now_us;
        Ok(())
    }

    fn mark_recovering(&mut self, module: ModuleId, now_us: u64) -> Result<(), ModuleRuntimeError> {
        let entry = self.entry_mut(module)?;
        if entry.state == ModuleRunState::Disabled {
            return Err(ModuleRuntimeError::InvalidTransition {
                module,
                from: entry.state,
                to: ModuleRunState::Recovering,
            });
        }
        entry.state = ModuleRunState::Recovering;
        entry.fault_count = entry.fault_count.saturating_add(1);
        entry.recovery_count = entry.recovery_count.saturating_add(1);
        entry.last_change_us = now_us;
        Ok(())
    }

    fn transition(
        &mut self,
        module: ModuleId,
        to: ModuleRunState,
        now_us: u64,
    ) -> Result<(), ModuleRuntimeError> {
        let entry = self.entry_mut(module)?;
        if !Self::is_valid_transition(entry.state, to) {
            return Err(ModuleRuntimeError::InvalidTransition {
                module,
                from: entry.state,
                to,
            });
        }
        entry.state = to;
        entry.last_change_us = now_us;
        Ok(())
    }

    fn entry_mut(
        &mut self,
        module: ModuleId,
    ) -> Result<&mut ModuleRuntimeEntry, ModuleRuntimeError> {
        self.entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.module == module)
            .ok_or(ModuleRuntimeError::Missing(module))
    }

    const fn is_valid_transition(from: ModuleRunState, to: ModuleRunState) -> bool {
        use ModuleRunState::*;
        matches!(
            (from, to),
            (Registered, Active)
                | (Registered, Disabled)
                | (Active, Active)
                | (Active, Suspended)
                | (Active, Disabled)
                | (Suspended, Active)
                | (Suspended, Disabled)
                | (Faulted, Active)
                | (Faulted, Recovering)
                | (Faulted, Disabled)
                | (Recovering, Active)
                | (Recovering, Faulted)
                | (Recovering, Disabled)
        )
    }
}

impl<const N: usize> Default for ModuleRuntimeGuard<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KernelError, RecoveryOutcome, SystemState};

    #[test]
    fn guard_registers_and_tracks_power_style_states() {
        let mut guard = ModuleRuntimeGuard::<2>::new();
        guard.register(ModuleId::Sensor, 10).unwrap();

        assert_eq!(
            guard.state(ModuleId::Sensor),
            Some(ModuleRunState::Registered)
        );
        guard.resume(ModuleId::Sensor, 20).unwrap();
        guard.suspend(ModuleId::Sensor, 30).unwrap();
        assert_eq!(
            guard.state(ModuleId::Sensor),
            Some(ModuleRunState::Suspended)
        );
        guard.resume(ModuleId::Sensor, 40).unwrap();
        assert_eq!(guard.state(ModuleId::Sensor), Some(ModuleRunState::Active));
    }

    #[test]
    fn guard_counts_faults_and_recoveries_from_outcomes() {
        let mut guard = ModuleRuntimeGuard::<1>::new();
        guard.register(ModuleId::Radio, 0).unwrap();
        guard.resume(ModuleId::Radio, 1).unwrap();

        guard
            .note_recovery_outcome(
                RecoveryOutcome {
                    module: ModuleId::Radio,
                    error: KernelError::RadioTxFail,
                    action: Action::RetryDelay(1000),
                    state: SystemState::Running,
                },
                10,
            )
            .unwrap();

        let entry = guard.entry(ModuleId::Radio).unwrap();
        assert_eq!(entry.state, ModuleRunState::Recovering);
        assert_eq!(entry.fault_count, 1);
        assert_eq!(entry.recovery_count, 1);
        assert_eq!(guard.count_state(ModuleRunState::Recovering), 1);
        assert_eq!(guard.latest_changed(), Some(entry));

        guard.complete_recovery(ModuleId::Radio, 20).unwrap();
        assert_eq!(guard.state(ModuleId::Radio), Some(ModuleRunState::Active));
    }

    #[test]
    fn disabled_modules_reject_new_fault_state() {
        let mut guard = ModuleRuntimeGuard::<1>::new();
        guard.register(ModuleId::Bus, 0).unwrap();
        guard.disable(ModuleId::Bus, 1).unwrap();

        assert_eq!(
            guard.note_recovery_outcome(
                RecoveryOutcome {
                    module: ModuleId::Bus,
                    error: KernelError::BusTimeout,
                    action: Action::NotifyUserTask,
                    state: SystemState::Degraded,
                },
                2,
            ),
            Err(ModuleRuntimeError::InvalidTransition {
                module: ModuleId::Bus,
                from: ModuleRunState::Disabled,
                to: ModuleRunState::Faulted,
            })
        );
    }

    #[test]
    fn startup_plan_registration_reports_capacity_errors() {
        let plan = StartupPlan::<2> {
            order: [Some(ModuleId::Kernel), Some(ModuleId::Sensor)],
            len: 2,
        };

        assert_eq!(
            ModuleRuntimeGuard::<1>::try_from_startup_plan(&plan).map(|guard| guard.len()),
            Err(ModuleRuntimeError::Full)
        );
    }

    #[test]
    fn startup_plan_registration_reports_duplicate_modules() {
        let plan = StartupPlan::<2> {
            order: [Some(ModuleId::Kernel), Some(ModuleId::Kernel)],
            len: 2,
        };

        assert_eq!(
            ModuleRuntimeGuard::<2>::try_from_startup_plan(&plan).map(|guard| guard.len()),
            Err(ModuleRuntimeError::Duplicate(ModuleId::Kernel))
        );
    }
}
