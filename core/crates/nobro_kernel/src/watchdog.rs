//! Software heartbeat tracker for module liveness.

use crate::ModuleId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchdogEntry {
    pub module: ModuleId,
    pub timeout_us: u64,
    pub last_beat_us: u64,
    pub missed: u32,
    pub expired_reported: bool,
}

impl WatchdogEntry {
    pub fn age_us(self, now_us: u64) -> u64 {
        now_us.saturating_sub(self.last_beat_us)
    }

    pub fn overdue_us(self, now_us: u64) -> u64 {
        self.age_us(now_us).saturating_sub(self.timeout_us)
    }

    pub fn is_expired(self, now_us: u64) -> bool {
        self.age_us(now_us) > self.timeout_us
    }
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

    /// Initialize caller-owned storage without a capacity-sized array copy.
    ///
    /// # Safety
    ///
    /// `destination` must be valid, aligned, writable storage for one
    /// uninitialized `Watchdog<N>`.
    pub(crate) unsafe fn init_in_place(destination: *mut Self) {
        let entries =
            core::ptr::addr_of_mut!((*destination).entries).cast::<Option<WatchdogEntry>>();
        for index in 0..N {
            entries.add(index).write(None);
        }
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
            expired_reported: false,
        });
        Ok(())
    }

    pub fn beat(&mut self, module: ModuleId, now_us: u64) -> Result<(), WatchdogError> {
        let Some(entry) = self.entry_mut(module) else {
            return Err(WatchdogError::Missing(module));
        };
        entry.last_beat_us = now_us;
        entry.missed = 0;
        entry.expired_reported = false;
        Ok(())
    }

    pub fn expired(&mut self, now_us: u64, out: &mut [ModuleId]) -> usize {
        let mut count = 0;
        for entry in self.entries.iter_mut().flatten() {
            if !entry.is_expired(now_us) {
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

    /// Report only the transition into an expired state. Repeated sweeps during
    /// one uninterrupted outage return no additional event until a heartbeat.
    pub fn expired_edges(&mut self, now_us: u64, out: &mut [ModuleId]) -> usize {
        let mut count = 0;
        for entry in self.entries.iter_mut().flatten() {
            if !entry.is_expired(now_us) || entry.expired_reported {
                continue;
            }
            entry.expired_reported = true;
            entry.missed = entry.missed.saturating_add(1);
            if count < out.len() {
                out[count] = entry.module;
                count += 1;
            }
        }
        count
    }

    pub fn expired_count(&self, now_us: u64) -> usize {
        self.entries
            .iter()
            .flatten()
            .filter(|entry| entry.is_expired(now_us))
            .count()
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
    fn in_place_initialization_matches_const_constructor() {
        let expected = Watchdog::<6>::new();
        let mut storage = core::mem::MaybeUninit::<Watchdog<6>>::uninit();

        unsafe {
            Watchdog::init_in_place(storage.as_mut_ptr());
        }
        let actual = unsafe { storage.assume_init_ref() };

        assert_eq!(actual.entries, expected.entries);
    }

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
        assert_eq!(watchdog.expired(160, &mut expired), 1);
        assert_eq!(watchdog.get(ModuleId::Sensor).expect("sensor").missed, 2);
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
    fn expired_count_does_not_mutate_missed_count() {
        let mut watchdog = Watchdog::<2>::new();
        watchdog.register(ModuleId::Sensor, 100, 0).unwrap();
        watchdog.register(ModuleId::Radio, 200, 0).unwrap();

        assert_eq!(watchdog.expired_count(150), 1);
        assert_eq!(watchdog.get(ModuleId::Sensor).expect("sensor").missed, 0);

        let mut expired = [ModuleId::Kernel; 2];
        assert_eq!(watchdog.expired(150, &mut expired), 1);
        assert_eq!(watchdog.get(ModuleId::Sensor).expect("sensor").missed, 1);
    }

    #[test]
    fn entry_reports_age_and_overdue_time() {
        let entry = WatchdogEntry {
            module: ModuleId::Sensor,
            timeout_us: 100,
            last_beat_us: 20,
            missed: 0,
            expired_reported: false,
        };

        assert_eq!(entry.age_us(90), 70);
        assert_eq!(entry.overdue_us(90), 0);
        assert!(!entry.is_expired(120));
        assert_eq!(entry.overdue_us(121), 1);
        assert!(entry.is_expired(121));
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
                expired_reported: false,
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
                expired_reported: false,
            })
        );
    }
}
