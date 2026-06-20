//! Fixed-capacity software alarm queue.

use crate::ModuleId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlarmId(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alarm {
    pub id: AlarmId,
    pub module: ModuleId,
    pub due_us: u64,
    pub period_us: Option<u32>,
}

impl Alarm {
    pub const fn once(id: AlarmId, module: ModuleId, due_us: u64) -> Self {
        Self {
            id,
            module,
            due_us,
            period_us: None,
        }
    }

    pub const fn periodic(id: AlarmId, module: ModuleId, due_us: u64, period_us: u32) -> Self {
        Self {
            id,
            module,
            due_us,
            period_us: Some(period_us),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlarmError {
    Full,
    Duplicate(AlarmId),
    InvalidPeriod(AlarmId),
    Missing(AlarmId),
}

pub struct AlarmQueue<const N: usize> {
    alarms: [Option<Alarm>; N],
}

impl<const N: usize> AlarmQueue<N> {
    pub const fn new() -> Self {
        Self { alarms: [None; N] }
    }

    pub fn schedule_once(
        &mut self,
        id: AlarmId,
        module: ModuleId,
        delay_us: u64,
        now_us: u64,
    ) -> Result<(), AlarmError> {
        self.insert(Alarm::once(id, module, now_us.saturating_add(delay_us)))
    }

    pub fn schedule_periodic(
        &mut self,
        id: AlarmId,
        module: ModuleId,
        period_us: u32,
        now_us: u64,
    ) -> Result<(), AlarmError> {
        if period_us == 0 {
            return Err(AlarmError::InvalidPeriod(id));
        }
        self.insert(Alarm::periodic(
            id,
            module,
            now_us.saturating_add(u64::from(period_us)),
            period_us,
        ))
    }

    pub fn cancel(&mut self, id: AlarmId) -> Result<Alarm, AlarmError> {
        let Some(idx) = self.index_of(id) else {
            return Err(AlarmError::Missing(id));
        };
        self.alarms[idx].take().ok_or(AlarmError::Missing(id))
    }

    pub fn remove_for(&mut self, module: ModuleId) -> usize {
        let mut removed = 0;
        for slot in self.alarms.iter_mut() {
            if slot.map(|alarm| alarm.module == module).unwrap_or(false) {
                *slot = None;
                removed += 1;
            }
        }
        removed
    }

    pub fn pop_due(&mut self, now_us: u64) -> Option<Alarm> {
        let idx = self.next_due_index(now_us)?;
        let mut alarm = self.alarms[idx].take()?;
        if let Some(period) = alarm.period_us {
            let fired = alarm;
            alarm.due_us = next_periodic_due(alarm.due_us, period, now_us);
            self.alarms[idx] = Some(alarm);
            Some(fired)
        } else {
            Some(alarm)
        }
    }

    pub fn next_due(&self, now_us: u64) -> Option<Alarm> {
        self.next_due_index(now_us).and_then(|idx| self.alarms[idx])
    }

    pub fn next_due_us(&self) -> Option<u64> {
        self.alarms.iter().flatten().map(|alarm| alarm.due_us).min()
    }

    pub fn len(&self) -> usize {
        self.alarms.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn insert(&mut self, alarm: Alarm) -> Result<(), AlarmError> {
        if self.index_of(alarm.id).is_some() {
            return Err(AlarmError::Duplicate(alarm.id));
        }
        let Some(slot) = self.alarms.iter_mut().find(|slot| slot.is_none()) else {
            return Err(AlarmError::Full);
        };
        *slot = Some(alarm);
        Ok(())
    }

    fn index_of(&self, id: AlarmId) -> Option<usize> {
        self.alarms
            .iter()
            .position(|slot| slot.map(|alarm| alarm.id == id).unwrap_or(false))
    }

    fn next_due_index(&self, now_us: u64) -> Option<usize> {
        let mut selected = None;
        for (idx, alarm) in self.alarms.iter().enumerate() {
            let Some(alarm) = alarm else {
                continue;
            };
            if alarm.due_us > now_us {
                continue;
            }
            selected = match selected {
                None => Some(idx),
                Some(prev_idx) => {
                    let prev = self.alarms[prev_idx].expect("selected alarm");
                    if alarm.due_us < prev.due_us {
                        Some(idx)
                    } else {
                        Some(prev_idx)
                    }
                }
            };
        }
        selected
    }
}

impl<const N: usize> Default for AlarmQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

fn next_periodic_due(mut due_us: u64, period_us: u32, now_us: u64) -> u64 {
    let period = u64::from(period_us);
    while due_us <= now_us {
        due_us = due_us.saturating_add(period);
    }
    due_us
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alarm_queue_pops_earliest_due_alarm() {
        let mut queue = AlarmQueue::<3>::new();
        queue
            .schedule_once(AlarmId(1), ModuleId::Sensor, 100, 0)
            .unwrap();
        queue
            .schedule_once(AlarmId(2), ModuleId::Radio, 50, 0)
            .unwrap();

        assert_eq!(queue.next_due_us(), Some(50));
        assert_eq!(
            queue.next_due(50),
            Some(Alarm::once(AlarmId(2), ModuleId::Radio, 50))
        );
        assert_eq!(
            queue.pop_due(50),
            Some(Alarm::once(AlarmId(2), ModuleId::Radio, 50))
        );
        assert_eq!(
            queue.pop_due(100),
            Some(Alarm::once(AlarmId(1), ModuleId::Sensor, 100))
        );
        assert!(queue.is_empty());
    }

    #[test]
    fn periodic_alarm_is_rescheduled_after_fire() {
        let mut queue = AlarmQueue::<1>::new();
        queue
            .schedule_periodic(AlarmId(7), ModuleId::Actuator, 20, 0)
            .unwrap();

        assert_eq!(
            queue.pop_due(20),
            Some(Alarm::periodic(AlarmId(7), ModuleId::Actuator, 20, 20))
        );
        assert_eq!(queue.next_due_us(), Some(40));
        assert_eq!(
            queue.pop_due(95),
            Some(Alarm::periodic(AlarmId(7), ModuleId::Actuator, 40, 20))
        );
        assert_eq!(queue.next_due_us(), Some(100));
    }

    #[test]
    fn cancel_removes_alarm() {
        let mut queue = AlarmQueue::<1>::new();
        queue
            .schedule_once(AlarmId(3), ModuleId::App(1), 10, 0)
            .unwrap();

        assert_eq!(
            queue.cancel(AlarmId(3)),
            Ok(Alarm::once(AlarmId(3), ModuleId::App(1), 10))
        );
        assert_eq!(queue.pop_due(10), None);
    }

    #[test]
    fn alarm_queue_rejects_duplicate_and_invalid_period() {
        let mut queue = AlarmQueue::<1>::new();
        queue
            .schedule_once(AlarmId(1), ModuleId::Sensor, 10, 0)
            .unwrap();

        assert_eq!(
            queue.schedule_once(AlarmId(1), ModuleId::Radio, 20, 0),
            Err(AlarmError::Duplicate(AlarmId(1)))
        );
        assert_eq!(
            queue.schedule_periodic(AlarmId(2), ModuleId::Radio, 0, 0),
            Err(AlarmError::InvalidPeriod(AlarmId(2)))
        );
    }

    #[test]
    fn remove_for_clears_module_alarms() {
        let mut queue = AlarmQueue::<3>::new();
        queue
            .schedule_once(AlarmId(1), ModuleId::Sensor, 10, 0)
            .unwrap();
        queue
            .schedule_periodic(AlarmId(2), ModuleId::Sensor, 20, 0)
            .unwrap();
        queue
            .schedule_once(AlarmId(3), ModuleId::Kernel, 30, 0)
            .unwrap();

        assert_eq!(queue.remove_for(ModuleId::Sensor), 2);
        assert_eq!(queue.len(), 1);
        assert_eq!(
            queue.pop_due(30),
            Some(Alarm::once(AlarmId(3), ModuleId::Kernel, 30))
        );
        assert!(queue.is_empty());
    }
}
