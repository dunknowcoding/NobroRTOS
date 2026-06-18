//! Deterministic fault injection for host-side recovery tests.

use crate::{KernelError, ModuleId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultMode {
    Once,
    Every(u8),
    Window { start: u32, end: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultRule {
    pub module: ModuleId,
    pub error: KernelError,
    pub mode: FaultMode,
    pub hits: u32,
    pub fired: u32,
}

impl FaultRule {
    pub const fn new(module: ModuleId, error: KernelError, mode: FaultMode) -> Self {
        Self {
            module,
            error,
            mode,
            hits: 0,
            fired: 0,
        }
    }

    fn should_fire(&mut self) -> bool {
        self.hits = self.hits.saturating_add(1);
        let fire = match self.mode {
            FaultMode::Once => self.fired == 0,
            FaultMode::Every(n) => n != 0 && self.hits % u32::from(n) == 0,
            FaultMode::Window { start, end } => self.hits >= start && self.hits <= end,
        };
        if fire {
            self.fired = self.fired.saturating_add(1);
        }
        fire
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultInjectError {
    Full,
}

pub struct FaultInjector<const N: usize> {
    rules: [Option<FaultRule>; N],
}

impl<const N: usize> FaultInjector<N> {
    pub const fn new() -> Self {
        Self { rules: [None; N] }
    }

    pub fn add(&mut self, rule: FaultRule) -> Result<(), FaultInjectError> {
        let Some(slot) = self.rules.iter_mut().find(|slot| slot.is_none()) else {
            return Err(FaultInjectError::Full);
        };
        *slot = Some(rule);
        Ok(())
    }

    pub fn check(&mut self, module: ModuleId) -> Option<KernelError> {
        for rule in self.rules.iter_mut().flatten() {
            if rule.module == module && rule.should_fire() {
                return Some(rule.error);
            }
        }
        None
    }

    pub fn fired_count(&self, module: ModuleId) -> u32 {
        self.rules
            .iter()
            .flatten()
            .filter(|rule| rule.module == module)
            .map(|rule| rule.fired)
            .sum()
    }
}

impl<const N: usize> Default for FaultInjector<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn once_rule_fires_once() {
        let mut injector = FaultInjector::<1>::new();
        injector
            .add(FaultRule::new(
                ModuleId::Sensor,
                KernelError::SensorReadFail,
                FaultMode::Once,
            ))
            .unwrap();

        assert_eq!(
            injector.check(ModuleId::Sensor),
            Some(KernelError::SensorReadFail)
        );
        assert_eq!(injector.check(ModuleId::Sensor), None);
        assert_eq!(injector.fired_count(ModuleId::Sensor), 1);
    }

    #[test]
    fn every_rule_fires_periodically() {
        let mut injector = FaultInjector::<1>::new();
        injector
            .add(FaultRule::new(
                ModuleId::Radio,
                KernelError::RadioTxFail,
                FaultMode::Every(3),
            ))
            .unwrap();

        assert_eq!(injector.check(ModuleId::Radio), None);
        assert_eq!(injector.check(ModuleId::Radio), None);
        assert_eq!(
            injector.check(ModuleId::Radio),
            Some(KernelError::RadioTxFail)
        );
    }

    #[test]
    fn window_rule_fires_inclusive_range() {
        let mut injector = FaultInjector::<1>::new();
        injector
            .add(FaultRule::new(
                ModuleId::Bus,
                KernelError::BusTimeout,
                FaultMode::Window { start: 2, end: 3 },
            ))
            .unwrap();

        assert_eq!(injector.check(ModuleId::Bus), None);
        assert_eq!(injector.check(ModuleId::Bus), Some(KernelError::BusTimeout));
        assert_eq!(injector.check(ModuleId::Bus), Some(KernelError::BusTimeout));
        assert_eq!(injector.check(ModuleId::Bus), None);
    }
}
