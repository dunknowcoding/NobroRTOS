//! Minimal cooperative executor for Phase 1 (no heap, no async/await yet).

use crate::scheduler::Timer;
use crate::{Criticality, ModuleId};

const READY_NONE: u8 = u8::MAX;
const CRITICALITY_LEVELS: usize = 5;

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
    /// Measured upper bound for lower-priority non-preemptible/critical-section delay.
    pub blocking_us: u32,
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
            blocking_us: 0,
        }
    }

    pub const fn with_blocking_us(mut self, blocking_us: u32) -> Self {
        self.blocking_us = blocking_us;
        self
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
    /// Width of the release group that most recently made this task ready.
    pub release_group_width: u8,
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
            release_group_width: 0,
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
    /// The O(1) dispatcher uses one native `u32` ready word.
    ReadyMaskCapacity,
    DuplicateTask(ModuleId),
    InvalidPeriod(ModuleId),
    InvalidBudget(ModuleId),
    InvalidBlocking(ModuleId),
}

pub struct TaskTable<const N: usize> {
    slots: [Option<TaskSlot>; N],
    len: u8,
    /// Intrusive list ordered by next release, then fixed priority. The head is
    /// the next compare in O(1); reinsertion happens after poll bookkeeping.
    release_head: u8,
    release_next: [u8; N],
    /// Ready membership is one bit per task slot. Dispatch first scans the
    /// five-level criticality bitmap, then consumes that level's FIFO head.
    /// The FIFO is required so a fast peer cannot starve an older release.
    ready_members: u32,
    /// Non-empty criticality queues; the highest set bit wins in O(1).
    ready_criticalities: u8,
    ready_head: [u8; CRITICALITY_LEVELS],
    ready_tail: [u8; CRITICALITY_LEVELS],
    ready_next: [u8; N],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DueSelection {
    pub index: usize,
    pub release_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DueSweep {
    pub selected: Option<DueSelection>,
    /// Array entries inspected by the same single pass used for selection.
    pub inspected_slots: u32,
    pub due_tasks: u32,
    pub simultaneous_width: u32,
    pub peer_inspected_slots: u32,
    /// Earliest phase-anchored release strictly after this sweep's snapshot.
    /// The diagnostic path can compare later clock samples in O(1) without
    /// adding another task-table scan after the poll-start timestamp.
    pub next_release_us: Option<u64>,
}

/// Earliest compare deadline and the admitted task-slot bits released by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeadlineReleaseArm {
    pub deadline_us: u64,
    pub ready_mask: u32,
}

/// Result of transferring ISR-marked task bits into the executor ready queues.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IsrReleaseReceipt {
    pub accepted: u32,
    pub rejected: u32,
}

impl<const N: usize> TaskTable<N> {
    pub const fn new() -> Self {
        Self {
            slots: [None; N],
            len: 0,
            release_head: READY_NONE,
            release_next: [READY_NONE; N],
            ready_members: 0,
            ready_criticalities: 0,
            ready_head: [READY_NONE; CRITICALITY_LEVELS],
            ready_tail: [READY_NONE; CRITICALITY_LEVELS],
            ready_next: [READY_NONE; N],
        }
    }

    pub fn add(&mut self, meta: TaskMeta, now_us: u64) -> Result<(), TaskTableError> {
        if N > u32::BITS as usize || usize::from(self.len) >= u32::BITS as usize {
            return Err(TaskTableError::ReadyMaskCapacity);
        }
        if meta.period_us == 0 {
            return Err(TaskTableError::InvalidPeriod(meta.module));
        }
        if meta.budget_us == 0 || meta.budget_us > meta.period_us {
            return Err(TaskTableError::InvalidBudget(meta.module));
        }
        if meta.blocking_us > meta.period_us.saturating_sub(meta.budget_us) {
            return Err(TaskTableError::InvalidBlocking(meta.module));
        }
        if self
            .slots
            .iter()
            .flatten()
            .any(|slot| slot.meta.module == meta.module)
        {
            return Err(TaskTableError::DuplicateTask(meta.module));
        }

        let Some(index) = self.slots.iter().position(|slot| slot.is_none()) else {
            return Err(TaskTableError::Full);
        };
        self.slots[index] = Some(TaskSlot {
            meta,
            stats: TaskStats {
                next_due_us: now_us,
                ..TaskStats::zeroed()
            },
        });
        self.len = self.len.saturating_add(1);
        self.insert_release(index);
        Ok(())
    }

    fn fixed_higher_priority(&self, left: usize, right: usize) -> bool {
        let left_meta = self.slots[left].expect("priority task slot").meta;
        let right_meta = self.slots[right].expect("priority task slot").meta;
        left_meta.criticality > right_meta.criticality
            || (left_meta.criticality == right_meta.criticality
                && (left_meta.period_us < right_meta.period_us
                    || (left_meta.period_us == right_meta.period_us && left < right)))
    }

    fn release_precedes(&self, left: usize, right: usize) -> bool {
        let left_due = self.slots[left]
            .expect("release task slot")
            .stats
            .next_due_us;
        let right_due = self.slots[right]
            .expect("release task slot")
            .stats
            .next_due_us;
        left_due < right_due || (left_due == right_due && self.fixed_higher_priority(left, right))
    }

    fn insert_release(&mut self, task_index: usize) {
        self.release_next[task_index] = READY_NONE;
        if self.release_head == READY_NONE
            || self.release_precedes(task_index, usize::from(self.release_head))
        {
            self.release_next[task_index] = self.release_head;
            self.release_head = task_index as u8;
            return;
        }
        let mut cursor = usize::from(self.release_head);
        loop {
            let next = self.release_next[cursor];
            if next == READY_NONE || self.release_precedes(task_index, usize::from(next)) {
                self.release_next[task_index] = next;
                self.release_next[cursor] = task_index as u8;
                return;
            }
            cursor = usize::from(next);
        }
    }

    fn release_root(&self) -> Option<usize> {
        (self.release_head != READY_NONE).then(|| usize::from(self.release_head))
    }

    fn pop_release_root(&mut self) -> Option<usize> {
        let root = self.release_root()?;
        self.release_head = self.release_next[root];
        self.release_next[root] = READY_NONE;
        Some(root)
    }

    /// Compatibility fallback for callers that record a task without first
    /// selecting it through the ready word. The executor hot path never enters
    /// this capacity-bounded search.
    fn remove_release(&mut self, task_index: usize) {
        if self.release_head == READY_NONE {
            return;
        }
        if self.release_head == task_index as u8 {
            let _ = self.pop_release_root();
            return;
        }
        let mut cursor = usize::from(self.release_head);
        while self.release_next[cursor] != READY_NONE {
            let next = usize::from(self.release_next[cursor]);
            if next == task_index {
                self.release_next[cursor] = self.release_next[task_index];
                self.release_next[task_index] = READY_NONE;
                return;
            }
            cursor = next;
        }
    }

    /// Incrementally transfer every elapsed phase release into the ready word.
    /// Work is proportional only to tasks actually released; there is no table scan.
    pub fn mark_due_releases(&mut self, now_us: u64) -> u32 {
        let mut released = 0u32;
        while let Some(root) = self.release_root() {
            let release_us = self.slots[root]
                .expect("release heap task slot")
                .stats
                .next_due_us;
            if release_us > now_us {
                break;
            }

            let mut group_members = 0u32;
            while let Some(group_root) = self.release_root() {
                let group_due = self.slots[group_root]
                    .expect("release heap task slot")
                    .stats
                    .next_due_us;
                if group_due != release_us {
                    break;
                }
                let task_index = self.pop_release_root().expect("nonempty release list");
                group_members |= 1u32 << task_index;
                self.enqueue_ready(task_index);
                released = released.saturating_add(1);
            }
            self.ready_members |= group_members;
            let group_width = group_members.count_ones().min(u32::from(u8::MAX)) as u8;
            let mut members = group_members;
            while members != 0 {
                let task_index = members.trailing_zeros() as usize;
                members &= members - 1;
                self.slots[task_index]
                    .as_mut()
                    .expect("ready task slot")
                    .stats
                    .release_group_width = group_width;
            }
        }
        released
    }

    /// Describe the exact earliest release group to arm in a compare provider.
    /// Heap pruning visits only members of that group and its immediate frontier.
    pub fn next_release_arm(&self) -> Option<DeadlineReleaseArm> {
        let root = self.release_root()?;
        let deadline_us = self.slots[root]
            .expect("release heap task slot")
            .stats
            .next_due_us;
        let mut ready_mask = 0u32;
        let mut cursor = self.release_head;
        while cursor != READY_NONE {
            let index = usize::from(cursor);
            if self.slots[index]
                .expect("release list task slot")
                .stats
                .next_due_us
                != deadline_us
            {
                break;
            }
            ready_mask |= 1u32 << index;
            cursor = self.release_next[index];
        }
        Some(DeadlineReleaseArm {
            deadline_us,
            ready_mask,
        })
    }

    /// Accept ready bits produced by the bounded compare ISR. Early, stale, or
    /// duplicate bits are rejected and never detach a future release.
    pub fn accept_isr_releases(&mut self, ready_mask: u32, now_us: u64) -> IsrReleaseReceipt {
        let valid_mask = if self.len == u32::BITS as u8 {
            u32::MAX
        } else {
            (1u32 << self.len) - 1
        };
        let mut receipt = IsrReleaseReceipt {
            rejected: (ready_mask & !valid_mask).count_ones(),
            ..IsrReleaseReceipt::default()
        };
        let mut accepted_members = 0u32;
        let mut candidates = ready_mask & valid_mask;
        while let Some(task_index) = self.release_root() {
            let bit = 1u32 << task_index;
            let due = self.slots[task_index]
                .expect("release task slot")
                .stats
                .next_due_us;
            if due > now_us || candidates & bit == 0 {
                break;
            }
            let _ = self.pop_release_root();
            self.enqueue_ready(task_index);
            accepted_members |= bit;
            candidates &= !bit;
            receipt.accepted = receipt.accepted.saturating_add(1);
        }
        receipt.rejected = receipt.rejected.saturating_add(candidates.count_ones());
        self.ready_members |= accepted_members;
        let width = receipt.accepted.min(u32::from(u8::MAX)) as u8;
        while accepted_members != 0 {
            let task_index = accepted_members.trailing_zeros() as usize;
            accepted_members &= accepted_members - 1;
            self.slots[task_index]
                .as_mut()
                .expect("ready task slot")
                .stats
                .release_group_width = width;
        }
        receipt
    }

    fn enqueue_ready(&mut self, task_index: usize) {
        let criticality = self.slots[task_index]
            .expect("ready task slot")
            .meta
            .criticality as usize;
        self.ready_next[task_index] = READY_NONE;
        let tail = self.ready_tail[criticality];
        if tail == READY_NONE {
            self.ready_head[criticality] = task_index as u8;
        } else {
            self.ready_next[usize::from(tail)] = task_index as u8;
        }
        self.ready_tail[criticality] = task_index as u8;
        self.ready_criticalities |= 1u8 << criticality;
    }

    fn ready_selection(&self) -> Option<DueSelection> {
        if self.ready_members == 0 {
            return None;
        }
        let criticality = (u8::BITS - 1 - self.ready_criticalities.leading_zeros()) as usize;
        let index = usize::from(self.ready_head[criticality]);
        Some(DueSelection {
            index,
            release_us: self.slots[index]
                .expect("ready task slot")
                .stats
                .next_due_us,
        })
    }

    /// Mark elapsed releases and return the O(1) highest-priority ready task.
    pub(crate) fn select_due(&mut self, now_us: u64) -> Option<DueSelection> {
        self.mark_due_releases(now_us);
        self.ready_selection()
    }

    /// Commit one previously selected task. The updated phase is reinserted by
    /// [`record_poll`](Self::record_poll) or [`skip_release`](Self::skip_release).
    pub(crate) fn take_selected(&mut self, index: usize) {
        let criticality = self.slots[index]
            .expect("selected task slot")
            .meta
            .criticality as usize;
        let head = self.ready_head[criticality];
        if head == index as u8 {
            let next = self.ready_next[index];
            self.ready_head[criticality] = next;
            if next == READY_NONE {
                self.ready_tail[criticality] = READY_NONE;
                self.ready_criticalities &= !(1u8 << criticality);
            }
        } else if self.ready_members & (1u32 << index) != 0 {
            // Compatibility fallback for direct `record_poll(index, ..)` calls.
            // The executor always consumes the queue head and never scans here.
            let mut previous = head;
            while previous != READY_NONE {
                let next = self.ready_next[usize::from(previous)];
                if next == index as u8 {
                    let after = self.ready_next[index];
                    self.ready_next[usize::from(previous)] = after;
                    if self.ready_tail[criticality] == index as u8 {
                        self.ready_tail[criticality] = previous;
                    }
                    break;
                }
                previous = next;
            }
        }
        self.ready_next[index] = READY_NONE;
        self.ready_members &= !(1u32 << index);
    }

    pub(crate) fn selected_group_width(&self, index: usize) -> u32 {
        u32::from(
            self.slots[index]
                .expect("selected task slot")
                .stats
                .release_group_width,
        )
    }

    /// O(1) readiness check used by idle decisions.
    pub fn has_due(&self, now_us: u64) -> bool {
        self.ready_members != 0
            || self.release_root().is_some_and(|index| {
                self.slots[index]
                    .expect("release heap task slot")
                    .stats
                    .next_due_us
                    <= now_us
            })
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

    /// Instrumented O(1)-selection form. `inspected_slots` now reports tasks
    /// released from the incremental heap, never capacity-wide table scans.
    pub(crate) fn due_sweep(&mut self, now_us: u64) -> DueSweep {
        let released = self.mark_due_releases(now_us);
        let selected = self.ready_selection();
        let simultaneous_width = selected.map_or(0, |selection| {
            u32::from(
                self.slots[selection.index]
                    .expect("selected task slot")
                    .stats
                    .release_group_width,
            )
        });
        DueSweep {
            selected,
            inspected_slots: released,
            due_tasks: self.ready_members.count_ones(),
            simultaneous_width,
            peer_inspected_slots: 0,
            next_release_us: self.release_root().map(|index| {
                self.slots[index]
                    .expect("release heap task slot")
                    .stats
                    .next_due_us
            }),
        }
    }

    pub fn record_poll(
        &mut self,
        idx: usize,
        now_us: u64,
        duration_us: u32,
        result: Poll,
    ) -> Option<TaskStats> {
        if idx >= usize::from(self.len) {
            return None;
        }
        self.remove_release(idx);
        self.take_selected(idx);
        self.finish_poll(idx, now_us, duration_us, result)
    }

    /// Commit a task already detached by [`take_selected`](Self::take_selected).
    /// This is the executor hot path; it avoids compatibility searches.
    pub(crate) fn record_selected_poll(
        &mut self,
        idx: usize,
        now_us: u64,
        duration_us: u32,
        result: Poll,
    ) -> Option<TaskStats> {
        if idx >= usize::from(self.len) {
            return None;
        }
        self.finish_poll(idx, now_us, duration_us, result)
    }

    fn finish_poll(
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
        let stats = slot.stats;
        self.insert_release(idx);
        Some(stats)
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
        if idx >= usize::from(self.len) {
            return;
        }
        self.remove_release(idx);
        self.take_selected(idx);
        self.finish_skip(idx, now_us);
    }

    pub(crate) fn skip_selected_release(&mut self, idx: usize, now_us: u64) {
        if idx >= usize::from(self.len) {
            return;
        }
        self.finish_skip(idx, now_us);
    }

    fn finish_skip(&mut self, idx: usize, now_us: u64) {
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
        self.insert_release(idx);
    }

    /// Earliest phase-anchored release over the whole set.
    pub fn next_due_us(&self) -> Option<u64> {
        self.ready_selection()
            .map(|selection| selection.release_us)
            .or_else(|| {
                self.release_root().map(|index| {
                    self.slots[index]
                        .expect("release heap task slot")
                        .stats
                        .next_due_us
                })
            })
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
    fn due_sweep_preserves_selection_without_capacity_scan() {
        let mut table = TaskTable::<4>::new();
        table
            .add(
                TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1000, 100),
                0,
            )
            .unwrap();
        table
            .add(
                TaskMeta::new(ModuleId::Actuator, Criticality::HardRealtime, 2000, 100),
                0,
            )
            .unwrap();
        let sweep = table.due_sweep(0);
        assert_eq!(
            sweep.selected.map(|selected| selected.index),
            table.due_index(0)
        );
        assert_eq!(sweep.selected.unwrap().release_us, 0);
        assert_eq!(sweep.inspected_slots, 2);
        assert_eq!(sweep.due_tasks, 2);
        assert_eq!(sweep.peer_inspected_slots, 0);
    }

    #[test]
    fn equal_criticality_fifo_prevents_new_release_overtaking() {
        let mut table = TaskTable::<2>::new();
        table
            .add(
                TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 10, 1),
                0,
            )
            .unwrap();
        table
            .add(
                TaskMeta::new(ModuleId::Radio, Criticality::Driver, 20, 1),
                0,
            )
            .unwrap();

        let first = table.select_due(0).expect("initial release");
        assert_eq!(first.index, 0);
        table.record_poll(first.index, 0, 1, Poll::Ready).unwrap();

        // Task 0 releases again at t=10, but task 1's t=0 release is older and
        // must run first. A fixed-rank-only mask would starve task 1 here.
        let second = table.select_due(10).expect("older peer remains ready");
        assert_eq!(second.index, 1);
        assert_eq!(second.release_us, 0);
    }

    fn assert_single_release_work_is_capacity_independent<const N: usize>() {
        let mut table = TaskTable::<N>::new();
        for index in 0..N {
            let phase = if index == N / 2 {
                10
            } else {
                1_000 + index as u64
            };
            table
                .add(
                    TaskMeta::new(ModuleId::App(index as u8), Criticality::User, 10_000, 1),
                    phase,
                )
                .unwrap();
        }
        let sweep = table.due_sweep(10);
        assert_eq!(sweep.inspected_slots, 1);
        assert_eq!(sweep.due_tasks, 1);
        assert_eq!(sweep.peer_inspected_slots, 0);
        assert_eq!(
            table.meta_at(sweep.selected.unwrap().index).unwrap().module,
            ModuleId::App((N / 2) as u8)
        );
    }

    #[test]
    fn ten_and_sixteen_task_variants_keep_release_work_flat() {
        assert_single_release_work_is_capacity_independent::<10>();
        assert_single_release_work_is_capacity_independent::<16>();
    }

    #[test]
    fn ready_word_accepts_exactly_thirty_two_tasks() {
        let mut table = TaskTable::<32>::new();
        for index in 0..32u8 {
            table
                .add(
                    TaskMeta::new(ModuleId::App(index), Criticality::User, 1_000, 1),
                    0,
                )
                .unwrap();
        }
        assert_eq!(table.mark_due_releases(0), 32);
        let mut selected = 0u32;
        while let Some(next) = table.select_due(0) {
            table.record_poll(next.index, 0, 1, Poll::Ready).unwrap();
            selected += 1;
        }
        assert_eq!(selected, 32);

        let mut too_wide = TaskTable::<33>::new();
        assert_eq!(
            too_wide.add(
                TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1_000, 1),
                0,
            ),
            Err(TaskTableError::ReadyMaskCapacity)
        );
    }

    #[test]
    fn compare_isr_handoff_accepts_exact_group_and_rejects_early_bits() {
        let mut table = TaskTable::<2>::new();
        table
            .add(
                TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 10, 1),
                10,
            )
            .unwrap();
        table
            .add(
                TaskMeta::new(ModuleId::Actuator, Criticality::System, 20, 1),
                10,
            )
            .unwrap();
        let arm = table.next_release_arm().expect("earliest group");
        assert_eq!(arm.deadline_us, 10);
        assert_eq!(arm.ready_mask.count_ones(), 2);
        assert_eq!(
            table.accept_isr_releases(arm.ready_mask, 9),
            IsrReleaseReceipt {
                accepted: 0,
                rejected: 2
            }
        );
        assert_eq!(
            table.accept_isr_releases(arm.ready_mask, 10),
            IsrReleaseReceipt {
                accepted: 2,
                rejected: 0
            }
        );
        let selected = table.select_due(10).expect("ISR made tasks ready");
        assert_eq!(
            table.meta_at(selected.index).unwrap().module,
            ModuleId::Actuator
        );
        assert_eq!(table.selected_group_width(selected.index), 2);
    }

    #[test]
    fn release_queue_retains_phase_after_large_lateness_and_counter_saturation() {
        let mut table = TaskTable::<1>::new();
        table
            .add(
                TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 10, 1),
                5,
            )
            .unwrap();
        let due = table.select_due(35).expect("late task is released");
        let stats = table
            .record_poll(due.index, 35, 1, Poll::Ready)
            .expect("task remains registered");
        assert_eq!(stats.missed_releases, 3);
        assert_eq!(table.next_due_us(), Some(45));

        table.slots[0].as_mut().unwrap().stats.missed_releases = u32::MAX;
        let due = table.select_due(45).expect("next phase release");
        let stats = table.record_poll(due.index, 45, 1, Poll::Ready).unwrap();
        assert_eq!(stats.missed_releases, u32::MAX);
        assert_eq!(table.next_due_us(), Some(55));
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

    #[test]
    fn blocking_term_must_fit_beside_execution_budget() {
        let mut table = TaskTable::<1>::new();
        let err = table
            .add(
                TaskMeta::new(ModuleId::App(1), Criticality::User, 100, 60).with_blocking_us(41),
                0,
            )
            .unwrap_err();
        assert_eq!(err, TaskTableError::InvalidBlocking(ModuleId::App(1)));
    }
}
