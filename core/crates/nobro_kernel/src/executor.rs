//! Minimal cooperative executor for Phase 1 (no heap, no async/await yet).

use crate::scheduler::Timer;
use crate::{Criticality, ModuleId};

const READY_NONE: u8 = u8::MAX;
const CRITICALITY_LEVELS: usize = 5;

/// Deterministic ordering among ready tasks in the same safety criticality.
///
/// The criticality bitmap always decides the safety class first. This policy
/// only breaks ties inside that class and therefore cannot let lower-class
/// work overtake higher-class work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum IntraClassOrder {
    /// Oldest ready transition first. This is the nano-compatible default.
    #[default]
    Fifo = 0,
    /// Shorter declared relative deadline first; registration order breaks ties.
    ShorterDeadlineFirst = 1,
    /// Shorter declared period first; registration order breaks ties.
    ShorterPeriodFirst = 2,
    /// Stable task-table registration order.
    RegistrationOrder = 3,
}

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
    /// Offset of the first release from the executor epoch.
    pub phase_us: u32,
    pub period_us: u32,
    /// Relative deadline from each release.
    pub deadline_us: u32,
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
            phase_us: 0,
            period_us,
            deadline_us: period_us,
            budget_us,
            blocking_us: 0,
        }
    }

    pub const fn with_phase_us(mut self, phase_us: u32) -> Self {
        self.phase_us = phase_us;
        self
    }

    pub const fn with_deadline_us(mut self, deadline_us: u32) -> Self {
        self.deadline_us = deadline_us;
        self
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
    InvalidPhase(ModuleId),
    InvalidDeadline(ModuleId),
    InvalidBudget(ModuleId),
    InvalidBlocking(ModuleId),
    UnknownTask(ModuleId),
}

/// Bounded task table. `READY_WORDS=1` is the nano/default layout; larger
/// values are an explicit scalable-dispatch opt in. Link indices reserve
/// `u8::MAX` as their sentinel, so the architectural maximum is 255 slots.
pub struct TaskTable<const N: usize, const READY_WORDS: usize = 1> {
    slots: [Option<TaskSlot>; N],
    len: u8,
    /// Intrusive list ordered by next release, then fixed priority. The head is
    /// the next compare in O(1); reinsertion happens after poll bookkeeping.
    release_head: u8,
    release_next: [u8; N],
    /// Ready membership is one bit per task slot. Dispatch first scans the
    /// five-level criticality bitmap, then consumes that level's FIFO head.
    /// The FIFO is required so a fast peer cannot starve an older release.
    ready_members: [u32; READY_WORDS],
    /// Ready members whose phase-anchored periodic release was consumed.
    /// A bit absent here was woken by an external event and must not advance
    /// the task's periodic schedule when that event-driven poll completes.
    periodic_ready_members: [u32; READY_WORDS],
    /// Non-empty criticality queues; the highest set bit wins in O(1).
    ready_criticalities: u8,
    intra_class_order: IntraClassOrder,
    ready_head: [u8; CRITICALITY_LEVELS],
    ready_tail: [u8; CRITICALITY_LEVELS],
    ready_next: [u8; N],
}

/// Complete scheduler state carried with a task when its owning core changes.
///
/// The record is crate-private because moving a task without the executor's
/// admission and runtime checks would break the scheduler invariants.
#[derive(Clone, Copy)]
pub(crate) struct TaskTransferState {
    slot: TaskSlot,
    ready: bool,
    periodic_ready: bool,
    release_linked: bool,
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

/// Exact earliest release group for an opt-in multiword task table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeadlineReleaseArmWords<const WORDS: usize> {
    pub deadline_us: u64,
    pub ready_words: [u32; WORDS],
}

/// Result of transferring ISR-marked task bits into the executor ready queues.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IsrReleaseReceipt {
    pub accepted: u32,
    pub rejected: u32,
}

impl<const N: usize, const READY_WORDS: usize> TaskTable<N, READY_WORDS> {
    pub const fn new() -> Self {
        Self::new_with_order(IntraClassOrder::Fifo)
    }

    pub const fn new_with_order(intra_class_order: IntraClassOrder) -> Self {
        Self {
            slots: [None; N],
            len: 0,
            release_head: READY_NONE,
            release_next: [READY_NONE; N],
            ready_members: [0; READY_WORDS],
            periodic_ready_members: [0; READY_WORDS],
            ready_criticalities: 0,
            intra_class_order,
            ready_head: [READY_NONE; CRITICALITY_LEVELS],
            ready_tail: [READY_NONE; CRITICALITY_LEVELS],
            ready_next: [READY_NONE; N],
        }
    }

    const fn word_and_bit(index: usize) -> (usize, u32) {
        (
            index / u32::BITS as usize,
            1u32 << (index % u32::BITS as usize),
        )
    }

    fn contains(words: &[u32; READY_WORDS], index: usize) -> bool {
        let (word, bit) = Self::word_and_bit(index);
        words.get(word).is_some_and(|value| value & bit != 0)
    }

    fn insert(words: &mut [u32; READY_WORDS], index: usize) {
        let (word, bit) = Self::word_and_bit(index);
        if let Some(value) = words.get_mut(word) {
            *value |= bit;
        }
    }

    fn remove(words: &mut [u32; READY_WORDS], index: usize) {
        let (word, bit) = Self::word_and_bit(index);
        if let Some(value) = words.get_mut(word) {
            *value &= !bit;
        }
    }

    fn any(words: &[u32; READY_WORDS]) -> bool {
        words.iter().any(|word| *word != 0)
    }

    fn count(words: &[u32; READY_WORDS]) -> u32 {
        words
            .iter()
            .fold(0u32, |total, word| total.saturating_add(word.count_ones()))
    }

    fn for_each_set(mut words: [u32; READY_WORDS], mut visit: impl FnMut(usize)) {
        for (word_index, word) in words.iter_mut().enumerate() {
            while *word != 0 {
                let bit = word.trailing_zeros() as usize;
                *word &= *word - 1;
                visit(word_index * u32::BITS as usize + bit);
            }
        }
    }

    /// Number of ready-mask words retained by this exact table layout.
    pub const fn ready_word_count(&self) -> usize {
        READY_WORDS
    }

    /// Copy the complete bounded ready membership for diagnostics or a
    /// multiword ISR handoff owned by an exact large-profile port.
    pub const fn ready_words(&self) -> [u32; READY_WORDS] {
        self.ready_members
    }

    /// Initialize caller-owned storage without capacity-sized array copies.
    ///
    /// # Safety
    ///
    /// `destination` must be valid, aligned, writable storage for one
    /// uninitialized `TaskTable<N, READY_WORDS>`.
    pub(crate) unsafe fn init_in_place(destination: *mut Self) {
        let slots = core::ptr::addr_of_mut!((*destination).slots).cast::<Option<TaskSlot>>();
        let release_next = core::ptr::addr_of_mut!((*destination).release_next).cast::<u8>();
        let ready_next = core::ptr::addr_of_mut!((*destination).ready_next).cast::<u8>();
        for index in 0..N {
            slots.add(index).write(None);
            release_next.add(index).write(READY_NONE);
            ready_next.add(index).write(READY_NONE);
        }
        core::ptr::addr_of_mut!((*destination).len).write(0);
        core::ptr::addr_of_mut!((*destination).release_head).write(READY_NONE);
        core::ptr::addr_of_mut!((*destination).ready_members).write([0; READY_WORDS]);
        core::ptr::addr_of_mut!((*destination).periodic_ready_members).write([0; READY_WORDS]);
        core::ptr::addr_of_mut!((*destination).ready_criticalities).write(0);
        core::ptr::addr_of_mut!((*destination).intra_class_order).write(IntraClassOrder::Fifo);
        core::ptr::addr_of_mut!((*destination).ready_head).write([READY_NONE; CRITICALITY_LEVELS]);
        core::ptr::addr_of_mut!((*destination).ready_tail).write([READY_NONE; CRITICALITY_LEVELS]);
    }

    pub fn add(&mut self, meta: TaskMeta, now_us: u64) -> Result<(), TaskTableError> {
        if READY_WORDS == 0
            || N > READY_WORDS.saturating_mul(u32::BITS as usize)
            || N > usize::from(READY_NONE)
            || usize::from(self.len) >= N
        {
            return Err(TaskTableError::ReadyMaskCapacity);
        }
        if meta.period_us == 0 || meta.period_us > nobro_admission::MAX_WRAP_SAFE_INTERVAL_US {
            return Err(TaskTableError::InvalidPeriod(meta.module));
        }
        if meta.phase_us >= meta.period_us {
            return Err(TaskTableError::InvalidPhase(meta.module));
        }
        if meta.deadline_us == 0 || meta.deadline_us > meta.period_us {
            return Err(TaskTableError::InvalidDeadline(meta.module));
        }
        if meta.budget_us == 0 || meta.budget_us > meta.deadline_us {
            return Err(TaskTableError::InvalidBudget(meta.module));
        }
        if meta.blocking_us > meta.deadline_us.saturating_sub(meta.budget_us) {
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
                next_due_us: now_us.saturating_add(u64::from(meta.phase_us)),
                ..TaskStats::zeroed()
            },
        });
        self.len = self.len.saturating_add(1);
        self.insert_release(index);
        Ok(())
    }

    pub(crate) fn rebase_unstarted_epoch(&mut self, now_us: u64) -> bool {
        if Self::any(&self.ready_members)
            || self
                .slots
                .iter()
                .flatten()
                .any(|slot| slot.stats.polls != 0)
        {
            return false;
        }
        self.release_head = READY_NONE;
        self.release_next.fill(READY_NONE);
        for slot in self.slots.iter_mut().flatten() {
            slot.stats.next_due_us = now_us.saturating_add(u64::from(slot.meta.phase_us));
        }
        for index in 0..N {
            if self.slots[index].is_some() {
                self.insert_release(index);
            }
        }
        true
    }

    fn release_precedes(&self, left: usize, right: usize) -> bool {
        // Release-list indices normally name registered slots. Borrow the
        // records so ordering never copies a complete TaskSlot through an
        // aggregate-return frame; a broken private link fails closed.
        let Some(left_slot) = self.slots.get(left).and_then(Option::as_ref) else {
            return false;
        };
        let Some(right_slot) = self.slots.get(right).and_then(Option::as_ref) else {
            return true;
        };
        let left_due = left_slot.stats.next_due_us;
        let right_due = right_slot.stats.next_due_us;
        let left_meta = left_slot.meta;
        let right_meta = right_slot.meta;
        left_due < right_due
            || (left_due == right_due
                && (left_meta.criticality > right_meta.criticality
                    || (left_meta.criticality == right_meta.criticality
                        && (left_meta.period_us < right_meta.period_us
                            || (left_meta.period_us == right_meta.period_us && left < right)))))
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
            let Some(release_us) = self
                .slots
                .get(root)
                .and_then(Option::as_ref)
                .map(|slot| slot.stats.next_due_us)
            else {
                break;
            };
            if release_us > now_us {
                break;
            }

            let mut group_members = [0u32; READY_WORDS];
            while let Some(group_root) = self.release_root() {
                let Some(group_due) = self
                    .slots
                    .get(group_root)
                    .and_then(Option::as_ref)
                    .map(|slot| slot.stats.next_due_us)
                else {
                    break;
                };
                if group_due != release_us {
                    break;
                }
                let Some(task_index) = self.pop_release_root() else {
                    break;
                };
                Self::insert(&mut group_members, task_index);
                if !Self::contains(&self.ready_members, task_index) {
                    self.enqueue_ready(task_index);
                }
                released = released.saturating_add(1);
            }
            for ((ready, periodic), group) in self
                .ready_members
                .iter_mut()
                .zip(self.periodic_ready_members.iter_mut())
                .zip(group_members)
            {
                *ready |= group;
                *periodic |= group;
            }
            let group_width = Self::count(&group_members).min(u32::from(u8::MAX)) as u8;
            Self::for_each_set(group_members, |task_index| {
                if let Some(slot) = self.slots.get_mut(task_index).and_then(Option::as_mut) {
                    slot.stats.release_group_width = group_width;
                }
            });
        }
        released
    }

    /// Describe the exact earliest release group to arm in a compare provider.
    /// Heap pruning visits only members of that group and its immediate frontier.
    pub fn next_release_arm(&self) -> Option<DeadlineReleaseArm> {
        let arm = self.next_release_arm_words()?;
        Some(DeadlineReleaseArm {
            deadline_us: arm.deadline_us,
            ready_mask: arm.ready_words.first().copied().unwrap_or(0),
        })
    }

    /// Describe the complete earliest release group for a scalable profile.
    pub fn next_release_arm_words(&self) -> Option<DeadlineReleaseArmWords<READY_WORDS>> {
        let root = self.release_root()?;
        let deadline_us = self.slots.get(root)?.as_ref()?.stats.next_due_us;
        let mut ready_words = [0u32; READY_WORDS];
        let mut cursor = self.release_head;
        while cursor != READY_NONE {
            let index = usize::from(cursor);
            if self.slots.get(index)?.as_ref()?.stats.next_due_us != deadline_us {
                break;
            }
            Self::insert(&mut ready_words, index);
            cursor = self.release_next[index];
        }
        Some(DeadlineReleaseArmWords {
            deadline_us,
            ready_words,
        })
    }

    /// Accept ready bits produced by the bounded compare ISR. Early, stale, or
    /// duplicate bits are rejected and never detach a future release.
    pub fn accept_isr_releases(&mut self, ready_mask: u32, now_us: u64) -> IsrReleaseReceipt {
        let mut ready_words = [0u32; READY_WORDS];
        if let Some(first) = ready_words.first_mut() {
            *first = ready_mask;
        }
        self.accept_isr_release_words(ready_words, now_us)
    }

    /// Multiword counterpart used by explicitly admitted large-profile ports.
    /// A provider must publish the complete earliest release group; missing,
    /// early, stale, duplicate, or out-of-range bits fail closed.
    pub fn accept_isr_release_words(
        &mut self,
        mut candidates: [u32; READY_WORDS],
        now_us: u64,
    ) -> IsrReleaseReceipt {
        let mut receipt = IsrReleaseReceipt::default();
        for (word_index, word) in candidates.iter_mut().enumerate() {
            let first_slot = word_index * u32::BITS as usize;
            let valid = usize::from(self.len)
                .saturating_sub(first_slot)
                .min(u32::BITS as usize);
            let valid_mask = match valid {
                0 => 0,
                32 => u32::MAX,
                bits => (1u32 << bits) - 1,
            };
            receipt.rejected = receipt
                .rejected
                .saturating_add((*word & !valid_mask).count_ones());
            *word &= valid_mask;
        }
        if receipt.rejected != 0 {
            return receipt;
        }
        let Some(expected) = self.next_release_arm_words() else {
            receipt.rejected = receipt.rejected.saturating_add(Self::count(&candidates));
            return receipt;
        };
        if expected.deadline_us > now_us || expected.ready_words != candidates {
            receipt.rejected = receipt.rejected.saturating_add(Self::count(&candidates));
            return receipt;
        }

        let accepted_members = expected.ready_words;
        while let Some(task_index) = self.release_root() {
            if !Self::contains(&accepted_members, task_index) {
                break;
            }
            let _ = self.pop_release_root();
            if !Self::contains(&self.ready_members, task_index) {
                self.enqueue_ready(task_index);
            }
            receipt.accepted = receipt.accepted.saturating_add(1);
        }
        for ((ready, periodic), accepted) in self
            .ready_members
            .iter_mut()
            .zip(self.periodic_ready_members.iter_mut())
            .zip(accepted_members)
        {
            *ready |= accepted;
            *periodic |= accepted;
        }
        let width = receipt.accepted.min(u32::from(u8::MAX)) as u8;
        Self::for_each_set(accepted_members, |task_index| {
            if let Some(slot) = self.slots.get_mut(task_index).and_then(Option::as_mut) {
                slot.stats.release_group_width = width;
            }
        });
        receipt
    }

    fn enqueue_ready(&mut self, task_index: usize) {
        let Some(criticality) = self
            .slots
            .get(task_index)
            .and_then(Option::as_ref)
            .map(|slot| slot.meta.criticality as usize)
        else {
            return;
        };
        self.ready_next[task_index] = READY_NONE;
        let head = self.ready_head[criticality];
        if head == READY_NONE {
            self.ready_head[criticality] = task_index as u8;
            self.ready_tail[criticality] = task_index as u8;
            self.ready_criticalities |= 1u8 << criticality;
            return;
        }
        if self.intra_class_precedes(task_index, usize::from(head)) {
            self.ready_next[task_index] = head;
            self.ready_head[criticality] = task_index as u8;
            self.ready_criticalities |= 1u8 << criticality;
            return;
        }
        let mut cursor = usize::from(head);
        loop {
            let next = self.ready_next[cursor];
            if next == READY_NONE {
                self.ready_next[cursor] = task_index as u8;
                self.ready_tail[criticality] = task_index as u8;
                break;
            }
            if self.intra_class_precedes(task_index, usize::from(next)) {
                self.ready_next[task_index] = next;
                self.ready_next[cursor] = task_index as u8;
                break;
            }
            cursor = usize::from(next);
        }
        self.ready_criticalities |= 1u8 << criticality;
    }

    fn intra_class_precedes(&self, left: usize, right: usize) -> bool {
        let left_index = left;
        let right_index = right;
        let Some(left) = self.slots.get(left_index).and_then(Option::as_ref) else {
            return false;
        };
        let Some(right) = self.slots.get(right_index).and_then(Option::as_ref) else {
            return true;
        };
        match self.intra_class_order {
            IntraClassOrder::Fifo => false,
            IntraClassOrder::ShorterDeadlineFirst => {
                left.meta.deadline_us < right.meta.deadline_us
                    || (left.meta.deadline_us == right.meta.deadline_us && left_index < right_index)
            }
            IntraClassOrder::ShorterPeriodFirst => {
                left.meta.period_us < right.meta.period_us
                    || (left.meta.period_us == right.meta.period_us && left_index < right_index)
            }
            IntraClassOrder::RegistrationOrder => left_index < right_index,
        }
    }

    pub const fn intra_class_order(&self) -> IntraClassOrder {
        self.intra_class_order
    }

    /// Wake a registered task from a bounded external event without consuming
    /// or shifting its phase-anchored periodic release. Repeated wakes dedup
    /// into one ready membership, matching async waker semantics.
    pub fn wake_event(&mut self, module: ModuleId) -> Result<bool, TaskTableError> {
        let Some(task_index) = self
            .slots
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|slot| slot.meta.module == module))
        else {
            return Err(TaskTableError::UnknownTask(module));
        };
        if Self::contains(&self.ready_members, task_index) {
            return Ok(false);
        }
        self.enqueue_ready(task_index);
        Self::insert(&mut self.ready_members, task_index);
        if let Some(slot) = self.slots.get_mut(task_index).and_then(Option::as_mut) {
            slot.stats.release_group_width = 1;
        }
        Ok(true)
    }

    fn ready_selection(&self) -> Option<DueSelection> {
        if !Self::any(&self.ready_members) {
            return None;
        }
        let criticality = (u8::BITS - 1 - self.ready_criticalities.leading_zeros()) as usize;
        let index = usize::from(self.ready_head[criticality]);
        // Queue indices originate only from registered slots. Keep the lookup
        // checked so an inconsistent private queue fails closed without an
        // unsafe access or a panic-formatting path.
        let slot = self.slots.get(index)?.as_ref()?;
        Some(DueSelection {
            index,
            release_us: slot.stats.next_due_us,
        })
    }

    /// Mark elapsed releases and return the O(1) highest-priority ready task.
    pub(crate) fn select_due(&mut self, now_us: u64) -> Option<DueSelection> {
        self.mark_due_releases(now_us);
        let mut selected = self.ready_selection()?;
        if !Self::contains(&self.periodic_ready_members, selected.index) {
            selected.release_us = now_us;
        }
        Some(selected)
    }

    /// Commit one previously selected task. The updated phase is reinserted by
    /// [`record_poll`](Self::record_poll) or [`skip_release`](Self::skip_release).
    pub(crate) fn take_selected(&mut self, index: usize) -> bool {
        let Some(criticality) = self
            .slots
            .get(index)
            .and_then(Option::as_ref)
            .map(|slot| slot.meta.criticality as usize)
        else {
            return false;
        };
        let head = self.ready_head[criticality];
        if head == index as u8 {
            let next = self.ready_next[index];
            self.ready_head[criticality] = next;
            if next == READY_NONE {
                self.ready_tail[criticality] = READY_NONE;
                self.ready_criticalities &= !(1u8 << criticality);
            }
        } else if Self::contains(&self.ready_members, index) {
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
        Self::remove(&mut self.ready_members, index);
        let periodic = Self::contains(&self.periodic_ready_members, index);
        Self::remove(&mut self.periodic_ready_members, index);
        periodic
    }

    pub(crate) fn selected_group_width(&self, index: usize) -> u32 {
        self.slots
            .get(index)
            .and_then(Option::as_ref)
            .map_or(0, |slot| u32::from(slot.stats.release_group_width))
    }

    /// O(1) readiness check used by idle decisions.
    pub fn has_due(&self, now_us: u64) -> bool {
        Self::any(&self.ready_members)
            || self.release_root().is_some_and(|index| {
                self.slots
                    .get(index)
                    .and_then(Option::as_ref)
                    .is_some_and(|slot| slot.stats.next_due_us <= now_us)
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
                    let Some(prev) = self.slots.get(prev_idx).and_then(Option::as_ref) else {
                        selected = Some(idx);
                        continue;
                    };
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
        let selected = self.ready_selection().map(|mut selection| {
            if !Self::contains(&self.periodic_ready_members, selection.index) {
                selection.release_us = now_us;
            }
            selection
        });
        let simultaneous_width = selected.map_or(0, |selection| {
            self.slots
                .get(selection.index)
                .and_then(Option::as_ref)
                .map_or(0, |slot| u32::from(slot.stats.release_group_width))
        });
        DueSweep {
            selected,
            inspected_slots: released,
            due_tasks: Self::count(&self.ready_members),
            simultaneous_width,
            peer_inspected_slots: 0,
            next_release_us: self.release_root().and_then(|index| {
                self.slots
                    .get(index)
                    .and_then(Option::as_ref)
                    .map(|slot| slot.stats.next_due_us)
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
        let _ = self.take_selected(idx);
        self.finish_poll(idx, now_us, duration_us, result, true)
    }

    /// Commit a task already detached by [`take_selected`](Self::take_selected).
    /// This is the executor hot path; it avoids compatibility searches.
    pub(crate) fn record_selected_poll(
        &mut self,
        idx: usize,
        now_us: u64,
        duration_us: u32,
        result: Poll,
        periodic_release: bool,
    ) -> Option<TaskStats> {
        if idx >= usize::from(self.len) {
            return None;
        }
        self.finish_poll(idx, now_us, duration_us, result, periodic_release)
    }

    fn finish_poll(
        &mut self,
        idx: usize,
        now_us: u64,
        duration_us: u32,
        result: Poll,
        periodic_release: bool,
    ) -> Option<TaskStats> {
        let slot = self.slots.get_mut(idx)?.as_mut()?;
        slot.stats.polls = slot.stats.polls.saturating_add(1);
        slot.stats.last_poll_us = now_us;
        if periodic_release {
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
        }
        slot.stats.max_observed_us = slot.stats.max_observed_us.max(duration_us);
        if duration_us > slot.meta.budget_us {
            slot.stats.overruns = slot.stats.overruns.saturating_add(1);
        }
        if result == Poll::Ready {
            slot.stats.ready = slot.stats.ready.saturating_add(1);
        }
        let stats = slot.stats;
        if periodic_release {
            self.insert_release(idx);
        }
        Some(stats)
    }

    pub fn get(&self, module: ModuleId) -> Option<TaskSlot> {
        self.slots
            .iter()
            .flatten()
            .find(|slot| slot.meta.module == module)
            .copied()
    }

    pub(crate) fn metas_with_transferred(
        &self,
        meta: TaskMeta,
    ) -> Result<[Option<TaskMeta>; N], TaskTableError> {
        if N > u32::BITS as usize || usize::from(self.len) >= N {
            return Err(if N > u32::BITS as usize {
                TaskTableError::ReadyMaskCapacity
            } else {
                TaskTableError::Full
            });
        }
        if self
            .slots
            .iter()
            .flatten()
            .any(|slot| slot.meta.module == meta.module)
        {
            return Err(TaskTableError::DuplicateTask(meta.module));
        }
        let mut metas = self.metas();
        metas[usize::from(self.len)] = Some(meta);
        Ok(metas)
    }

    fn release_contains(&self, index: usize) -> bool {
        let mut cursor = self.release_head;
        while cursor != READY_NONE {
            if usize::from(cursor) == index {
                return true;
            }
            cursor = self.release_next[usize::from(cursor)];
        }
        false
    }

    fn collapse_words(mut words: [u32; READY_WORDS], removed: usize) -> [u32; READY_WORDS] {
        for index in removed..N.saturating_sub(1) {
            if Self::contains(&words, index + 1) {
                Self::insert(&mut words, index);
            } else {
                Self::remove(&mut words, index);
            }
        }
        if N != 0 {
            Self::remove(&mut words, N - 1);
        }
        words
    }

    fn collapse_index(index: u8, removed: usize) -> u8 {
        if index == READY_NONE {
            READY_NONE
        } else if usize::from(index) > removed {
            index - 1
        } else if usize::from(index) == removed {
            READY_NONE
        } else {
            index
        }
    }

    pub(crate) fn detach_for_transfer(
        &mut self,
        module: ModuleId,
    ) -> Result<TaskTransferState, TaskTableError> {
        let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|slot| slot.meta.module == module))
        else {
            return Err(TaskTableError::UnknownTask(module));
        };
        let Some(slot) = self.slots[index] else {
            return Err(TaskTableError::UnknownTask(module));
        };
        let state = TaskTransferState {
            slot,
            ready: Self::contains(&self.ready_members, index),
            periodic_ready: Self::contains(&self.periodic_ready_members, index),
            release_linked: self.release_contains(index),
        };

        self.remove_release(index);
        let _ = self.take_selected(index);

        let old_len = usize::from(self.len);
        for cursor in index..old_len.saturating_sub(1) {
            self.slots[cursor] = self.slots[cursor + 1];
            self.release_next[cursor] = self.release_next[cursor + 1];
            self.ready_next[cursor] = self.ready_next[cursor + 1];
        }
        if old_len != 0 {
            let tail = old_len - 1;
            self.slots[tail] = None;
            self.release_next[tail] = READY_NONE;
            self.ready_next[tail] = READY_NONE;
        }
        self.len = self.len.saturating_sub(1);

        self.release_head = Self::collapse_index(self.release_head, index);
        for link in self.release_next.iter_mut().take(usize::from(self.len)) {
            *link = Self::collapse_index(*link, index);
        }
        for head in &mut self.ready_head {
            *head = Self::collapse_index(*head, index);
        }
        for tail in &mut self.ready_tail {
            *tail = Self::collapse_index(*tail, index);
        }
        for link in self.ready_next.iter_mut().take(usize::from(self.len)) {
            *link = Self::collapse_index(*link, index);
        }
        self.ready_members = Self::collapse_words(self.ready_members, index);
        self.periodic_ready_members = Self::collapse_words(self.periodic_ready_members, index);
        Ok(state)
    }

    pub(crate) fn attach_transferred(
        &mut self,
        state: TaskTransferState,
    ) -> Result<(), TaskTableError> {
        let _ = self.metas_with_transferred(state.slot.meta)?;
        let index = usize::from(self.len);
        self.slots[index] = Some(state.slot);
        self.release_next[index] = READY_NONE;
        self.ready_next[index] = READY_NONE;
        self.len = self.len.saturating_add(1);
        if state.release_linked {
            self.insert_release(index);
        }
        if state.ready {
            self.enqueue_ready(index);
            Self::insert(&mut self.ready_members, index);
            if state.periodic_ready {
                Self::insert(&mut self.periodic_ready_members, index);
            }
        }
        Ok(())
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
        let _ = self.take_selected(idx);
        self.finish_skip(idx, now_us);
    }

    pub(crate) fn skip_selected_release(
        &mut self,
        idx: usize,
        now_us: u64,
        periodic_release: bool,
    ) {
        if idx >= usize::from(self.len) {
            return;
        }
        if periodic_release {
            self.finish_skip(idx, now_us);
        }
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
            .map(|selection| {
                if !Self::contains(&self.periodic_ready_members, selection.index) {
                    0
                } else {
                    selection.release_us
                }
            })
            .or_else(|| {
                let index = self.release_root()?;
                self.slots
                    .get(index)
                    .copied()
                    .flatten()
                    .map(|slot| slot.stats.next_due_us)
            })
    }
}

impl<const N: usize, const READY_WORDS: usize> Default for TaskTable<N, READY_WORDS> {
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
    fn in_place_initialization_matches_const_constructor() {
        let expected = TaskTable::<5>::new();
        let mut storage = core::mem::MaybeUninit::<TaskTable<5>>::uninit();

        unsafe {
            TaskTable::init_in_place(storage.as_mut_ptr());
        }
        let actual = unsafe { storage.assume_init_ref() };

        assert_eq!(actual.slots, expected.slots);
        assert_eq!(actual.len, expected.len);
        assert_eq!(actual.release_head, expected.release_head);
        assert_eq!(actual.release_next, expected.release_next);
        assert_eq!(actual.ready_members, expected.ready_members);
        assert_eq!(
            actual.periodic_ready_members,
            expected.periodic_ready_members
        );
        assert_eq!(actual.ready_criticalities, expected.ready_criticalities);
        assert_eq!(actual.intra_class_order, expected.intra_class_order);
        assert_eq!(actual.ready_head, expected.ready_head);
        assert_eq!(actual.ready_tail, expected.ready_tail);
        assert_eq!(actual.ready_next, expected.ready_next);
    }

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
    fn explicit_phase_shapes_first_release_and_preserves_the_periodic_anchor() {
        let mut table = TaskTable::<2>::new();
        table
            .add(
                TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1_000, 100)
                    .with_phase_us(250)
                    .with_deadline_us(700),
                10_000,
            )
            .unwrap();
        table
            .add(
                TaskMeta::new(ModuleId::Actuator, Criticality::System, 1_000, 100)
                    .with_phase_us(600),
                10_000,
            )
            .unwrap();

        assert_eq!(table.next_due_us(), Some(10_250));
        assert!(table.select_due(10_249).is_none());
        let first = table.select_due(10_250).expect("first shaped release");
        assert_eq!(table.meta_at(first.index).unwrap().module, ModuleId::Sensor);
        table
            .record_poll(first.index, 10_260, 10, Poll::Ready)
            .unwrap();
        assert_eq!(table.next_due_us(), Some(10_600));
        let second = table.select_due(10_600).expect("second shaped release");
        assert_eq!(
            table.meta_at(second.index).unwrap().module,
            ModuleId::Actuator
        );
        table
            .record_poll(second.index, 10_610, 10, Poll::Ready)
            .unwrap();
        assert_eq!(table.next_due_us(), Some(11_250));
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

    #[test]
    fn explicit_intra_class_policy_never_overrides_safety_criticality() {
        let mut table = TaskTable::<3>::new_with_order(IntraClassOrder::ShorterDeadlineFirst);
        table
            .add(
                TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1_000, 1)
                    .with_deadline_us(900),
                0,
            )
            .unwrap();
        table
            .add(
                TaskMeta::new(ModuleId::Radio, Criticality::Driver, 1_000, 1).with_deadline_us(100),
                0,
            )
            .unwrap();
        table
            .add(
                TaskMeta::new(ModuleId::Actuator, Criticality::System, 1_000, 1)
                    .with_deadline_us(1_000),
                0,
            )
            .unwrap();

        assert_eq!(
            table.intra_class_order(),
            IntraClassOrder::ShorterDeadlineFirst
        );
        let first = table.select_due(0).unwrap();
        assert_eq!(
            table.meta_at(first.index).unwrap().module,
            ModuleId::Actuator
        );
        table.record_poll(first.index, 0, 1, Poll::Pending).unwrap();

        let second = table.select_due(0).unwrap();
        assert_eq!(table.meta_at(second.index).unwrap().module, ModuleId::Radio);
        table
            .record_poll(second.index, 0, 1, Poll::Pending)
            .unwrap();

        let third = table.select_due(0).unwrap();
        assert_eq!(table.meta_at(third.index).unwrap().module, ModuleId::Sensor);
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
    fn opt_in_multiword_profile_crosses_32_and_64_task_boundaries() {
        let mut thirty_three = TaskTable::<33, 2>::new();
        for index in 0..33u8 {
            thirty_three
                .add(
                    TaskMeta::new(ModuleId::App(index), Criticality::User, 100, 1),
                    0,
                )
                .unwrap();
        }
        let receipt = thirty_three.accept_isr_release_words([u32::MAX, 1], 0);
        assert_eq!(receipt.accepted, 33);
        assert_eq!(receipt.rejected, 0);
        assert_eq!(thirty_three.ready_words(), [u32::MAX, 1]);

        let mut sixty_four = TaskTable::<64, 2>::new();
        for index in 0..64u8 {
            sixty_four
                .add(
                    TaskMeta::new(ModuleId::App(index), Criticality::User, 100, 1),
                    0,
                )
                .unwrap();
        }
        let receipt = sixty_four.accept_isr_release_words([u32::MAX; 2], 0);
        assert_eq!(receipt.accepted, 64);
        assert_eq!(receipt.rejected, 0);
        assert_eq!(sixty_four.ready_words(), [u32::MAX; 2]);

        // FIFO is the explicit nano-compatible within-class policy: all 64
        // simultaneous releases retain registration order across the word
        // boundary instead of making bit position an accidental priority.
        for expected in 0..64usize {
            let selected = sixty_four.select_due(0).unwrap();
            assert_eq!(selected.index, expected);
            let periodic = sixty_four.take_selected(selected.index);
            sixty_four
                .record_selected_poll(selected.index, 0, 1, Poll::Pending, periodic)
                .unwrap();
        }
        assert_eq!(sixty_four.ready_words(), [0; 2]);

        let mut too_narrow = TaskTable::<65, 2>::new();
        assert_eq!(
            too_narrow.add(
                TaskMeta::new(ModuleId::App(0), Criticality::User, 100, 1),
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
    fn compare_isr_handoff_rejects_a_partial_simultaneous_group_atomically() {
        let mut table = TaskTable::<33, 2>::new();
        for index in 0..33u8 {
            table
                .add(
                    TaskMeta::new(ModuleId::App(index), Criticality::User, 100, 1),
                    0,
                )
                .unwrap();
        }
        let arm = table.next_release_arm_words().unwrap();
        assert_eq!(arm.ready_words, [u32::MAX, 1]);
        assert_eq!(
            table.accept_isr_release_words([u32::MAX, 0], 0),
            IsrReleaseReceipt {
                accepted: 0,
                rejected: 32,
            }
        );
        assert_eq!(table.ready_words(), [0; 2]);
        assert_eq!(table.next_release_arm_words(), Some(arm));
    }

    #[test]
    fn compare_isr_handoff_rejects_out_of_range_bits_atomically() {
        let mut table = TaskTable::<33, 2>::new();
        for index in 0..33u8 {
            table
                .add(
                    TaskMeta::new(ModuleId::App(index), Criticality::User, 100, 1),
                    0,
                )
                .unwrap();
        }
        let arm = table.next_release_arm_words().unwrap();
        assert_eq!(arm.ready_words, [u32::MAX, 1]);
        assert_eq!(
            table.accept_isr_release_words([u32::MAX, 3], 0),
            IsrReleaseReceipt {
                accepted: 0,
                rejected: 1,
            }
        );
        assert_eq!(table.ready_words(), [0; 2]);
        assert_eq!(table.next_release_arm_words(), Some(arm));
    }

    #[test]
    fn compare_isr_handoff_fast_path_accepts_only_release_root() {
        let mut table = TaskTable::<2>::new();
        table
            .add(
                TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 100, 1).with_phase_us(20),
                0,
            )
            .unwrap();
        table
            .add(
                TaskMeta::new(ModuleId::Radio, Criticality::System, 100, 1).with_phase_us(10),
                0,
            )
            .unwrap();

        assert_eq!(
            table.accept_isr_releases(1u32 << 0, 20),
            IsrReleaseReceipt {
                accepted: 0,
                rejected: 1
            }
        );
        assert_eq!(
            table.accept_isr_releases(1u32 << 1, 9),
            IsrReleaseReceipt {
                accepted: 0,
                rejected: 1
            }
        );
        assert_eq!(
            table.accept_isr_releases(1u32 << 1, 10),
            IsrReleaseReceipt {
                accepted: 1,
                rejected: 0
            }
        );
        let selected = table
            .select_due(10)
            .expect("single ISR bit made root ready");
        assert_eq!(selected.index, 1);
        assert_eq!(table.selected_group_width(selected.index), 1);
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

    #[test]
    fn event_wakes_dedup_without_shifting_the_periodic_phase() {
        let module = ModuleId::App(0);
        let mut table = TaskTable::<1>::new();
        table
            .add(
                TaskMeta::new(module, Criticality::Driver, 100, 20).with_deadline_us(100),
                0,
            )
            .unwrap();

        let first = table.select_due(0).unwrap();
        assert_eq!(first.release_us, 0);
        let first_periodic = table.take_selected(first.index);
        assert!(first_periodic);
        let first_stats = table
            .record_selected_poll(first.index, 1, 1, Poll::Pending, first_periodic)
            .unwrap();
        assert_eq!(first_stats.next_due_us, 100);

        assert!(table.wake_event(module).unwrap());
        assert!(!table.wake_event(module).unwrap(), "event wakes dedup");
        let event = table.select_due(25).unwrap();
        assert_eq!(event.release_us, 25);
        let event_periodic = table.take_selected(event.index);
        assert!(!event_periodic);
        let event_stats = table
            .record_selected_poll(event.index, 26, 1, Poll::Pending, event_periodic)
            .unwrap();
        assert_eq!(event_stats.next_due_us, 100);
        assert_eq!(event_stats.missed_releases, 0);

        assert!(table.wake_event(module).unwrap());
        let periodic_and_event = table.select_due(100).unwrap();
        assert_eq!(periodic_and_event.release_us, 100);
        let periodic = table.take_selected(periodic_and_event.index);
        assert!(
            periodic,
            "one poll consumes the coincident release and event"
        );
        let final_stats = table
            .record_selected_poll(periodic_and_event.index, 101, 1, Poll::Ready, periodic)
            .unwrap();
        assert_eq!(final_stats.next_due_us, 200);
        assert_eq!(
            table.wake_event(ModuleId::App(7)),
            Err(TaskTableError::UnknownTask(ModuleId::App(7)))
        );
    }

    #[test]
    fn transferred_ready_task_preserves_phase_and_compacts_peer_queues() {
        let mut source = TaskTable::<4>::new();
        for module in 0..4 {
            source
                .add(
                    TaskMeta::new(ModuleId::App(module), Criticality::User, 100, 10),
                    0,
                )
                .unwrap();
        }
        assert_eq!(source.mark_due_releases(0), 4);

        let moved = source.detach_for_transfer(ModuleId::App(1)).unwrap();
        let mut destination = TaskTable::<4>::new();
        destination.attach_transferred(moved).unwrap();

        let mut source_order = [ModuleId::Kernel; 3];
        for module in &mut source_order {
            let selected = source.select_due(0).unwrap();
            *module = source.meta_at(selected.index).unwrap().module;
            let periodic = source.take_selected(selected.index);
            source
                .record_selected_poll(selected.index, 1, 1, Poll::Pending, periodic)
                .unwrap();
        }
        assert_eq!(
            source_order,
            [ModuleId::App(0), ModuleId::App(2), ModuleId::App(3)]
        );

        let selected = destination.select_due(0).unwrap();
        assert_eq!(
            destination.meta_at(selected.index).unwrap().module,
            ModuleId::App(1)
        );
        let periodic = destination.take_selected(selected.index);
        assert!(periodic);
        let stats = destination
            .record_selected_poll(selected.index, 1, 1, Poll::Pending, periodic)
            .unwrap();
        assert_eq!(stats.next_due_us, 100);
    }

    #[test]
    fn transferring_slot_thirty_one_keeps_ready_mask_bounded() {
        let mut source = TaskTable::<32>::new();
        for module in 0..32 {
            source
                .add(
                    TaskMeta::new(ModuleId::App(module), Criticality::User, 100, 1),
                    0,
                )
                .unwrap();
        }
        assert_eq!(source.mark_due_releases(0), 32);
        let moved = source.detach_for_transfer(ModuleId::App(31)).unwrap();
        assert_eq!(TaskTable::<32>::count(&source.ready_members), 31);

        let mut destination = TaskTable::<32>::new();
        destination.attach_transferred(moved).unwrap();
        assert_eq!(destination.ready_members, [1]);
        assert_eq!(destination.meta_at(0).unwrap().module, ModuleId::App(31));
    }

    #[test]
    fn phase_and_relative_deadline_fail_closed() {
        let mut table = TaskTable::<1>::new();
        let module = ModuleId::App(1);
        assert_eq!(
            table.add(
                TaskMeta::new(module, Criticality::User, 100, 10).with_phase_us(100),
                0
            ),
            Err(TaskTableError::InvalidPhase(module))
        );
        assert_eq!(
            table.add(
                TaskMeta::new(module, Criticality::User, 100, 10).with_deadline_us(101),
                0
            ),
            Err(TaskTableError::InvalidDeadline(module))
        );
        assert_eq!(
            table.add(
                TaskMeta::new(
                    module,
                    Criticality::User,
                    nobro_admission::MAX_WRAP_SAFE_INTERVAL_US + 1,
                    10,
                ),
                0,
            ),
            Err(TaskTableError::InvalidPeriod(module))
        );
    }
}
