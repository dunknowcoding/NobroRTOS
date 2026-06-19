//! Software heartbeat tracker for module liveness.

use crate::ModuleId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchdogEntry {
    pub module: ModuleId,
    pub timeout_us: u64,
    pub last_beat_us: u64,
    pub missed: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchdogError {
    Full,
    Duplicate(ModuleId),
    Missing(ModuleId),
    InvalidTimeout(ModuleId),
}

pub struct Watchdog<const N: usize> {
    entries: [Option<WatchdogEntry>; N],
}

impl<const N: usize> Watchdog<N> {
    pub const fn new() -> Self {
        Self { entries: [None; N] }
    }

    pub fn register(
        &mut self,
        module: ModuleId,
        timeout_us: u64,
        now_us: u64,
    ) -> Result<(), WatchdogError> {
        if timeout_us == 0 {
            return Err(WatchdogError::InvalidTimeout(module));
        }
        if self
            .entries
            .iter()
            .flatten()
            .any(|entry| entry.module == module)
        {
            return Err(WatchdogError::Duplicate(module));
        }

        let Some(slot) = self.entries.iter_mut().find(|entry| entry.is_none()) else {
            return Err(WatchdogError::Full);
        };
        *slot = Some(WatchdogEntry {
            module,
            timeout_us,
            last_beat_us: now_us,
            missed: 0,
        });
        Ok(())
    }

    pub fn beat(&mut self, module: ModuleId, now_us: u64) -> Result<(), WatchdogError> {
        let Some(entry) = self.entry_mut(module) else {
            return Err(WatchdogError::Missing(module));
        };
        entry.last_beat_us = now_us;
        entry.missed = 0;
        Ok(())
    }

    pub fn expired(&mut self, now_us: u64, out: &mut [ModuleId]) -> usize {
        let mut count = 0;
        for entry in self.entries.iter_mut().flatten() {
            if now_us.saturating_sub(entry.last_beat_us) <= entry.timeout_us {
                continue;
            }
            entry.missed = entry.missed.saturating_add(1);
            if count < out.len() {
                out[count] = entry.module;
                count += 1;
            }
        }
        count
    }

    pub fn remove(&mut self, module: ModuleId) -> Option<WatchdogEntry> {
        for slot in self.entries.iter_mut() {
            if slot.map(|entry| entry.module == module).unwrap_or(false) {
                return slot.take();
            }
        }
        None
    }

    pub fn get(&self, module: ModuleId) -> Option<WatchdogEntry> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.module == module)
            .copied()
    }

    fn entry_mut(&mut self, module: ModuleId) -> Option<&mut WatchdogEntry> {
        self.entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.module == module)
    }
}

impl<const N: usize> Default for Watchdog<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_reports_expired_modules() {
        let mut watchdog = Watchdog::<2>::new();
        watchdog.register(ModuleId::Sensor, 100, 0).unwrap();
        watchdog.register(ModuleId::Radio, 500, 0).unwrap();

        let mut expired = [ModuleId::Kernel; 2];
        let count = watchdog.expired(150, &mut expired);

        assert_eq!(count, 1);
        assert_eq!(expired[0], ModuleId::Sensor);
        assert_eq!(watchdog.get(ModuleId::Sensor).expect("sensor").missed, 1);
    }

    #[test]
    fn heartbeat_resets_missed_count() {
        let mut watchdog = Watchdog::<1>::new();
        watchdog.register(ModuleId::Bus, 100, 0).unwrap();

        let mut expired = [ModuleId::Kernel; 1];
        assert_eq!(watchdog.expired(150, &mut expired), 1);
        watchdog.beat(ModuleId::Bus, 160).unwrap();

        let entry = watchdog.get(ModuleId::Bus).expect("bus");
        assert_eq!(entry.missed, 0);
        assert_eq!(entry.last_beat_us, 160);
        assert_eq!(watchdog.expired(200, &mut expired), 0);
    }

    #[test]
    fn duplicate_registration_is_rejected() {
        let mut watchdog = Watchdog::<1>::new();
        watchdog.register(ModuleId::Actuator, 100, 0).unwrap();
        assert_eq!(
            watchdog.register(ModuleId::Actuator, 100, 0),
            Err(WatchdogError::Duplicate(ModuleId::Actuator))
        );
    }

    #[test]
    fn remove_clears_module_watchdog() {
        let mut watchdog = Watchdog::<2>::new();
        watchdog.register(ModuleId::Sensor, 100, 0).unwrap();
        watchdog.register(ModuleId::Radio, 200, 0).unwrap();

        assert_eq!(
            watchdog.remove(ModuleId::Sensor),
            Some(WatchdogEntry {
                module: ModuleId::Sensor,
                timeout_us: 100,
                last_beat_us: 0,
                missed: 0,
            })
        );
        assert_eq!(watchdog.get(ModuleId::Sensor), None);
        assert_eq!(
            watchdog.get(ModuleId::Radio),
            Some(WatchdogEntry {
                module: ModuleId::Radio,
                timeout_us: 200,
                last_beat_us: 0,
                missed: 0,
            })
        );
    }
}
