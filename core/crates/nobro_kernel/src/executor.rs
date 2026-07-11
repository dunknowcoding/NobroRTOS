//! Minimal cooperative executor for Phase 1 (no heap, no async/await yet).

use crate::scheduler::Timer;
use crate::{Criticality, ModuleId};

pub trait Task {
    fn poll(&mut self, now_us: u64) -> Poll;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Poll {
    Pending,
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskMeta {
    pub module: ModuleId,
    pub criticality: Criticality,
    pub period_us: u32,
    pub budget_us: u32,
}

impl TaskMeta {
    pub const fn new(
        module: ModuleId,
        criticality: Criticality,
        period_us: u32,
        budget_us: u32,
    ) -> Self {
        Self {
            module,
            criticality,
            period_us,
            budget_us,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskStats {
    pub polls: u32,
    pub ready: u32,
    pub overruns: u32,
    pub missed_releases: u32,
    pub last_poll_us: u64,
    pub next_due_us: u64,
    pub max_observed_us: u32,
}

impl TaskStats {
    pub const fn zeroed() -> Self {
        Self {
            polls: 0,
            ready: 0,
            overruns: 0,
            missed_releases: 0,
            last_poll_us: 0,
            next_due_us: 0,
            max_observed_us: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskSlot {
    pub meta: TaskMeta,
    pub stats: TaskStats,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskTableError {
    Full,
    DuplicateTask(ModuleId),
    InvalidPeriod(ModuleId),
    InvalidBudget(ModuleId),
}

pub struct TaskTable<const N: usize> {
    slots: [Option<TaskSlot>; N],
}

impl<const N: usize> TaskTable<N> {
    pub const fn new() -> Self {
        Self { slots: [None; N] }
    }

    pub fn add(&mut self, meta: TaskMeta, now_us: u64) -> Result<(), TaskTableError> {
        if meta.period_us == 0 {
            return Err(TaskTableError::InvalidPeriod(meta.module));
        }
        if meta.budget_us == 0 || meta.budget_us > meta.period_us {
            return Err(TaskTableError::InvalidBudget(meta.module));
        }
        if self
            .slots
            .iter()
            .flatten()
            .any(|slot| slot.meta.module == meta.module)
        {
            return Err(TaskTableError::DuplicateTask(meta.module));
        }

        let Some(slot) = self.slots.iter_mut().find(|slot| slot.is_none()) else {
            return Err(TaskTableError::Full);
        };
        *slot = Some(TaskSlot {
            meta,
            stats: TaskStats {
                next_due_us: now_us,
                ..TaskStats::zeroed()
            },
        });
        Ok(())
    }

    pub fn due_index(&self, now_us: u64) -> Option<usize> {
        let mut selected = None;
        for (idx, slot) in self.slots.iter().enumerate() {
            let Some(slot) = slot else {
                continue;
            };
            if now_us < slot.stats.next_due_us {
                continue;
            }

            selected = match selected {
                None => Some(idx),
                Some(prev_idx) => {
                    let prev = self.slots[prev_idx].expect("selected task slot");
                    if slot.meta.criticality > prev.meta.criticality
                        || (slot.meta.criticality == prev.meta.criticality
                            && slot.stats.next_due_us < prev.stats.next_due_us)
                    {
                        Some(idx)
                    } else {
                        Some(prev_idx)
                    }
                }
            };
        }
        selected
    }

    pub fn record_poll(
        &mut self,
        idx: usize,
        now_us: u64,
        duration_us: u32,
        result: Poll,
    ) -> Option<TaskStats> {
        let slot = self.slots.get_mut(idx)?.as_mut()?;
        slot.stats.polls = slot.stats.polls.saturating_add(1);
        slot.stats.last_poll_us = now_us;
        let period = u64::from(slot.meta.period_us);
        let releases_elapsed = now_us.saturating_sub(slot.stats.next_due_us) / period;
        slot.stats.missed_releases = slot
            .stats
            .missed_releases
            .saturating_add(releases_elapsed.min(u64::from(u32::MAX)) as u32);
        slot.stats.next_due_us = slot
            .stats
            .next_due_us
            .saturating_add(releases_elapsed.saturating_add(1).saturating_mul(period));
        slot.stats.max_observed_us = slot.stats.max_observed_us.max(duration_us);
        if duration_us > slot.meta.budget_us {
            slot.stats.overruns = slot.stats.overruns.saturating_add(1);
        }
        if result == Poll::Ready {
            slot.stats.ready = slot.stats.ready.saturating_add(1);
        }
        Some(slot.stats)
    }

    pub fn get(&self, module: ModuleId) -> Option<TaskSlot> {
        self.slots
            .iter()
            .flatten()
            .find(|slot| slot.meta.module == module)
            .copied()
    }

    pub fn meta_at(&self, idx: usize) -> Option<TaskMeta> {
        self.slots.get(idx)?.as_ref().map(|slot| slot.meta)
    }

    /// All registered task contracts (schedulability-analysis input).
    pub fn metas(&self) -> [Option<TaskMeta>; N] {
        let mut metas = [None; N];
        for (out, slot) in metas.iter_mut().zip(self.slots.iter()) {
            *out = slot.as_ref().map(|slot| slot.meta);
        }
        metas
    }

    /// Skip one release without executing it (module not runnable): the release
    /// is counted as missed and the phase-anchored next due advances.
    pub fn skip_release(&mut self, idx: usize, now_us: u64) {
        let Some(Some(slot)) = self.slots.get_mut(idx) else {
            return;
        };
        let period = u64::from(slot.meta.period_us);
        let releases_elapsed = now_us.saturating_sub(slot.stats.next_due_us) / period;
        let skipped = releases_elapsed.saturating_add(1);
        slot.stats.missed_releases = slot
            .stats
            .missed_releases
            .saturating_add(skipped.min(u64::from(u32::MAX)) as u32);
        slot.stats.next_due_us = slot
            .stats
            .next_due_us
            .saturating_add(skipped.saturating_mul(period));
    }

    /// Earliest phase-anchored release over the whole set.
    pub fn next_due_us(&self) -> Option<u64> {
        self.slots
            .iter()
            .flatten()
            .map(|slot| slot.stats.next_due_us)
            .min()
    }
}

impl<const N: usize> Default for TaskTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple I2C poll task stub.
pub struct I2cPollTask {
    timer: Timer,
    owner: u8,
    pub reads: u32,
}

impl I2cPollTask {
    pub fn new(owner: u8, now_us: u64) -> Self {
        Self {
            timer: Timer::after_ms(100, now_us),
            owner,
            reads: 0,
        }
    }
}

impl Task for I2cPollTask {
    fn poll(&mut self, now_us: u64) -> Poll {
        if !self.timer.is_ready(now_us) {
            return Poll::Pending;
        }
        self.reads += 1;
        self.timer = Timer::after_ms(100, now_us);
        Poll::Ready
    }
}

impl I2cPollTask {
    pub fn owner(&self) -> u8 {
        self.owner
    }
}

/// Heartbeat / stats reporter.
pub struct StatsTask {
    timer: Timer,
}

impl StatsTask {
    pub fn new(now_us: u64) -> Self {
        Self {
            timer: Timer::after_ms(2000, now_us),
        }
    }
}

impl Task for StatsTask {
    fn poll(&mut self, now_us: u64) -> Poll {
        if self.timer.is_ready(now_us) {
            self.timer = Timer::after_ms(2000, now_us);
            Poll::Ready
        } else {
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_index_prefers_higher_criticality() {
        let mut table = TaskTable::<3>::new();
        table
            .add(
                TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1000, 100),
                0,
            )
            .unwrap();
        table
            .add(
                TaskMeta::new(ModuleId::Actuator, Criticality::HardRealtime, 20_000, 200),
                0,
            )
            .unwrap();

        let idx = table.due_index(0).expect("due task");
        let selected = table.record_poll(idx, 0, 50, Poll::Ready).unwrap();
        assert_eq!(selected.ready, 1);
        assert_eq!(
            table.get(ModuleId::Actuator).expect("actuator").stats.polls,
            1
        );
    }

    #[test]
    fn task_table_tracks_budget_overruns() {
        let mut table = TaskTable::<1>::new();
        table
            .add(
                TaskMeta::new(ModuleId::Radio, Criticality::Driver, 1000, 100),
                0,
            )
            .unwrap();

        let idx = table.due_index(0).expect("due task");
        let stats = table.record_poll(idx, 0, 250, Poll::Pending).unwrap();

        assert_eq!(stats.overruns, 1);
        assert_eq!(stats.max_observed_us, 250);
        assert_eq!(stats.next_due_us, 1000);
    }

    #[test]
    fn late_poll_preserves_phase_and_counts_missed_releases() {
        let mut table = TaskTable::<1>::new();
        table
            .add(
                TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1000, 100),
                0,
            )
            .unwrap();
        let stats = table.record_poll(0, 2_500, 50, Poll::Ready).unwrap();
        assert_eq!(stats.missed_releases, 2);
        assert_eq!(stats.next_due_us, 3_000);
    }

    #[test]
    fn invalid_task_budget_is_rejected() {
        let mut table = TaskTable::<1>::new();
        let err = table
            .add(
                TaskMeta::new(ModuleId::App(1), Criticality::User, 100, 200),
                0,
            )
            .unwrap_err();
        assert_eq!(err, TaskTableError::InvalidBudget(ModuleId::App(1)));
    }
}
