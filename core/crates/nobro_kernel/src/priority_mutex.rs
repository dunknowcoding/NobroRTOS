//! Bounded priority-inheritance/ceiling mutex scheduling state.
//!
//! This type is owned by one scheduler/critical section; it is not itself a
//! Rust data lock. It prevents unbounded priority inversion by promoting the
//! current owner to the resource ceiling or highest waiting priority, and it
//! transfers ownership to the most urgent waiter with FIFO ties. All storage is
//! fixed, and the associated hold-time contract is charged into [`TaskMeta`]
//! before admission.

use crate::TaskMeta;

/// Higher numeric values are more urgent, matching [`crate::Criticality`].
pub type MutexPriority = u8;
pub type MutexTaskId = u16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriorityMutexError {
    ZeroHoldBound,
    BlockingOverflow,
    BlockingExceedsDeadline,
    Reentrant,
    DuplicateWaiter,
    WaitQueueFull,
    NotOwner,
}

/// Admission-side contract for a shared resource.
///
/// `max_hold_us` includes any nested resource hold time. Under the bounded
/// ceiling protocol a task is blocked by at most the largest applicable
/// lower-priority critical section, so charging takes the maximum of the
/// task's existing bound and this resource bound rather than summing unrelated
/// resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PriorityMutexContract {
    pub ceiling: MutexPriority,
    pub max_hold_us: u32,
}

impl PriorityMutexContract {
    pub const fn new(ceiling: MutexPriority, max_hold_us: u32) -> Result<Self, PriorityMutexError> {
        if max_hold_us == 0 {
            return Err(PriorityMutexError::ZeroHoldBound);
        }
        Ok(Self {
            ceiling,
            max_hold_us,
        })
    }

    /// Charge this resource's blocking bound into the ordinary response-time
    /// analysis input. Admission still performs the authoritative full-set
    /// schedulability check.
    pub const fn charge(self, mut task: TaskMeta) -> Result<TaskMeta, PriorityMutexError> {
        let blocking = if task.blocking_us > self.max_hold_us {
            task.blocking_us
        } else {
            self.max_hold_us
        };
        let Some(total) = task.budget_us.checked_add(blocking) else {
            return Err(PriorityMutexError::BlockingOverflow);
        };
        if total > task.deadline_us {
            return Err(PriorityMutexError::BlockingExceedsDeadline);
        }
        task.blocking_us = blocking;
        Ok(task)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Owner {
    task: MutexTaskId,
    base_priority: MutexPriority,
    effective_priority: MutexPriority,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Waiter {
    task: MutexTaskId,
    priority: MutexPriority,
    sequence: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutexAcquire {
    Acquired {
        effective_priority: MutexPriority,
    },
    Queued {
        owner: MutexTaskId,
        owner_effective_priority: MutexPriority,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MutexRelease {
    pub released: MutexTaskId,
    pub restored_priority: MutexPriority,
    pub next_owner: Option<MutexTaskId>,
    pub next_effective_priority: Option<MutexPriority>,
}

/// Scheduler-owned bounded mutex state.
pub struct BoundedPriorityMutex<const WAITERS: usize> {
    contract: PriorityMutexContract,
    owner: Option<Owner>,
    waiters: [Option<Waiter>; WAITERS],
    next_sequence: u32,
}

impl<const WAITERS: usize> BoundedPriorityMutex<WAITERS> {
    pub const fn new(contract: PriorityMutexContract) -> Self {
        Self {
            contract,
            owner: None,
            waiters: [None; WAITERS],
            next_sequence: 0,
        }
    }

    pub const fn contract(&self) -> PriorityMutexContract {
        self.contract
    }

    pub fn owner(&self) -> Option<MutexTaskId> {
        self.owner.map(|owner| owner.task)
    }

    pub fn owner_effective_priority(&self) -> Option<MutexPriority> {
        self.owner.map(|owner| owner.effective_priority)
    }

    pub fn waiter_count(&self) -> usize {
        self.waiters.iter().flatten().count()
    }

    /// Acquire immediately or enter the bounded wait set. The caller uses the
    /// returned effective priority to update its portable scheduler/port state.
    pub fn acquire(
        &mut self,
        task: MutexTaskId,
        base_priority: MutexPriority,
    ) -> Result<MutexAcquire, PriorityMutexError> {
        let Some(mut owner) = self.owner else {
            let effective_priority = base_priority.max(self.contract.ceiling);
            self.owner = Some(Owner {
                task,
                base_priority,
                effective_priority,
            });
            return Ok(MutexAcquire::Acquired { effective_priority });
        };
        if owner.task == task {
            return Err(PriorityMutexError::Reentrant);
        }
        if self
            .waiters
            .iter()
            .flatten()
            .any(|waiter| waiter.task == task)
        {
            return Err(PriorityMutexError::DuplicateWaiter);
        }
        let slot = self
            .waiters
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(PriorityMutexError::WaitQueueFull)?;
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        *slot = Some(Waiter {
            task,
            priority: base_priority,
            sequence,
        });
        owner.effective_priority = owner
            .effective_priority
            .max(base_priority)
            .max(self.contract.ceiling);
        self.owner = Some(owner);
        Ok(MutexAcquire::Queued {
            owner: owner.task,
            owner_effective_priority: owner.effective_priority,
        })
    }

    /// Release and transfer to the most urgent waiter. The former owner's base
    /// priority is returned so the scheduler can remove inheritance.
    pub fn release(&mut self, task: MutexTaskId) -> Result<MutexRelease, PriorityMutexError> {
        let owner = self.owner.ok_or(PriorityMutexError::NotOwner)?;
        if owner.task != task {
            return Err(PriorityMutexError::NotOwner);
        }
        let selected = self
            .waiters
            .iter()
            .enumerate()
            .filter_map(|(index, waiter)| waiter.map(|waiter| (index, waiter)))
            .max_by(|(_, left), (_, right)| {
                left.priority
                    .cmp(&right.priority)
                    .then_with(|| right.sequence.cmp(&left.sequence))
            });
        let Some((selected_index, next)) = selected else {
            self.owner = None;
            return Ok(MutexRelease {
                released: owner.task,
                restored_priority: owner.base_priority,
                next_owner: None,
                next_effective_priority: None,
            });
        };
        self.waiters[selected_index] = None;
        let inherited = self
            .waiters
            .iter()
            .flatten()
            .map(|waiter| waiter.priority)
            .max()
            .unwrap_or(0);
        let effective_priority = next.priority.max(self.contract.ceiling).max(inherited);
        self.owner = Some(Owner {
            task: next.task,
            base_priority: next.priority,
            effective_priority,
        });
        Ok(MutexRelease {
            released: owner.task,
            restored_priority: owner.base_priority,
            next_owner: Some(next.task),
            next_effective_priority: Some(effective_priority),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Criticality, ModuleId};

    #[test]
    fn higher_waiter_inherits_then_ownership_transfers() {
        let contract = PriorityMutexContract::new(3, 20).unwrap();
        let mut mutex = BoundedPriorityMutex::<3>::new(contract);
        assert_eq!(
            mutex.acquire(1, 1),
            Ok(MutexAcquire::Acquired {
                effective_priority: 3
            })
        );
        assert_eq!(
            mutex.acquire(2, 7),
            Ok(MutexAcquire::Queued {
                owner: 1,
                owner_effective_priority: 7
            })
        );
        assert_eq!(mutex.owner_effective_priority(), Some(7));
        let release = mutex.release(1).unwrap();
        assert_eq!(release.restored_priority, 1);
        assert_eq!(release.next_owner, Some(2));
        assert_eq!(release.next_effective_priority, Some(7));
    }

    #[test]
    fn equal_priority_waiters_transfer_fifo_and_capacity_fails_closed() {
        let contract = PriorityMutexContract::new(2, 10).unwrap();
        let mut mutex = BoundedPriorityMutex::<2>::new(contract);
        mutex.acquire(1, 2).unwrap();
        mutex.acquire(2, 5).unwrap();
        mutex.acquire(3, 5).unwrap();
        assert_eq!(mutex.acquire(4, 9), Err(PriorityMutexError::WaitQueueFull));
        assert_eq!(mutex.release(1).unwrap().next_owner, Some(2));
        assert_eq!(mutex.release(2).unwrap().next_owner, Some(3));
    }

    #[test]
    fn hold_bound_is_charged_into_normal_task_admission_input() {
        let task = TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 100, 60);
        let contract = PriorityMutexContract::new(3, 25).unwrap();
        assert_eq!(contract.charge(task).unwrap().blocking_us, 25);
        let too_large = PriorityMutexContract::new(3, 41).unwrap();
        assert_eq!(
            too_large.charge(task),
            Err(PriorityMutexError::BlockingExceedsDeadline)
        );
    }
}
