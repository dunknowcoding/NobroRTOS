//! Bounded, no-allocation cooperative async executor.
//!
//! NobroRTOS is poll-first: modules implement [`crate::executor::Task`] and are dispatched
//! by the deadline [`crate::scheduler`]. This module offers an `async fn` wrapper without
//! giving up the guarantees that
//! make the RTOS auditable:
//!
//! * **Bounded** - a fixed-capacity table (`const N`); [`BoundedExecutor::spawn`] returns an
//!   error instead of ever allocating, so the worst-case task count is known at build time.
//! * **No heap** - futures are supplied by the caller (static or stack-pinned via
//!   [`core::pin::pin!`]); the executor only stores a `Pin<&mut dyn Future>`.
//! * **Cooperative** - the waker is a no-op, so every incomplete task is polled once per
//!   [`BoundedExecutor::run_once`] pass. There is no timer wheel or priority inheritance
//!   here; for hard-real-time dispatch keep using `scheduler` + `executor`. Use this only
//!   where an `async` wrapper around otherwise-cooperative work is worth the ergonomics.
//!
//! [`BoundedExecutor::run_to_idle`] drives tasks to completion but is itself bounded: if a
//! task never resolves it returns [`AsyncError::Stalled`] rather than looping forever.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, RawWaker, RawWakerVTable, Waker};

/// A caller-owned future erased behind a pinned trait object. The `'a` lifetime ties every
/// spawned task to its backing storage, so the executor never needs to allocate or own it.
pub type SpawnedTask<'a> = Pin<&'a mut (dyn Future<Output = ()> + 'a)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsyncError {
    /// The task table is full (`spawn` past capacity `N`).
    Full,
    /// `run_to_idle` hit its round budget with tasks still pending (a task never resolved).
    Stalled,
}

/// Outcome of a single [`BoundedExecutor::run_once`] pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunStats {
    /// Tasks polled this pass (i.e. those that were still pending at the start).
    pub polled: u32,
    /// Tasks that resolved to `Ready` during this pass.
    pub completed: u32,
    /// Tasks still pending after this pass.
    pub pending: u32,
}

struct Slot<'a> {
    future: SpawnedTask<'a>,
    done: bool,
}

/// A fixed-capacity cooperative async executor over `N` task slots.
pub struct BoundedExecutor<'a, const N: usize> {
    slots: [Option<Slot<'a>>; N],
    count: usize,
}

// A no-op waker: cooperative scheduling re-polls every pending task each pass, so waking is
// implicit and the waker carries no state (null data pointer).
static NOOP_VTABLE: RawWakerVTable = RawWakerVTable::new(
    |_| RawWaker::new(core::ptr::null(), &NOOP_VTABLE), // clone
    |_| {},                                             // wake
    |_| {},                                             // wake_by_ref
    |_| {},                                             // drop
);

fn noop_waker() -> Waker {
    // SAFETY: the vtable's clone/wake/drop are all no-ops and ignore the (null) data pointer.
    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &NOOP_VTABLE)) }
}

impl<'a, const N: usize> BoundedExecutor<'a, N> {
    pub const fn new() -> Self {
        Self {
            slots: [const { None }; N],
            count: 0,
        }
    }

    /// Number of live (spawned, not-yet-reclaimed) tasks.
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    /// Register a future. Returns its slot index, or [`AsyncError::Full`] at capacity.
    pub fn spawn(&mut self, future: SpawnedTask<'a>) -> Result<usize, AsyncError> {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(Slot {
                    future,
                    done: false,
                });
                self.count += 1;
                return Ok(i);
            }
        }
        Err(AsyncError::Full)
    }

    /// Tasks that have not yet resolved.
    pub fn pending(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| matches!(s, Some(slot) if !slot.done))
            .count()
    }

    /// Poll every pending task exactly once. Completed tasks are dropped, freeing their slot.
    pub fn run_once(&mut self) -> RunStats {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut stats = RunStats::default();
        for slot in self.slots.iter_mut() {
            if let Some(task) = slot {
                if task.done {
                    continue;
                }
                stats.polled += 1;
                if task.future.as_mut().poll(&mut cx).is_ready() {
                    stats.completed += 1;
                    *slot = None; // reclaim capacity immediately
                    self.count -= 1;
                }
            }
        }
        stats.pending = self.pending() as u32;
        stats
    }

    /// Drive tasks to completion, bounded by `max_rounds` passes. Returns the number of
    /// passes used, or [`AsyncError::Stalled`] if tasks remain pending after the budget.
    pub fn run_to_idle(&mut self, max_rounds: u32) -> Result<u32, AsyncError> {
        for round in 1..=max_rounds {
            if self.run_once().pending == 0 {
                return Ok(round);
            }
        }
        if self.pending() == 0 {
            Ok(max_rounds)
        } else {
            Err(AsyncError::Stalled)
        }
    }
}

impl<'a, const N: usize> Default for BoundedExecutor<'a, N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::pin::pin;
    use core::sync::atomic::{AtomicU32, Ordering};
    use core::task::Poll;

    /// A future that yields `Pending` `rounds` times, then resolves, bumping `done` once.
    struct Countdown<'c> {
        left: u32,
        done: &'c AtomicU32,
    }

    impl<'c> Future for Countdown<'c> {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.left == 0 {
                self.done.fetch_add(1, Ordering::Relaxed);
                Poll::Ready(())
            } else {
                self.left -= 1;
                cx.waker().wake_by_ref(); // no-op, but exercises the waker path
                Poll::Pending
            }
        }
    }

    #[test]
    fn spawns_and_completes_within_bound() {
        let done = AtomicU32::new(0);
        let f1 = pin!(Countdown {
            left: 3,
            done: &done
        });
        let f2 = pin!(Countdown {
            left: 1,
            done: &done
        });
        let mut exec = BoundedExecutor::<4>::new();
        exec.spawn(f1).unwrap();
        exec.spawn(f2).unwrap();
        assert_eq!(exec.len(), 2);

        let rounds = exec.run_to_idle(16).expect("both tasks resolve");
        // f1 needs 4 polls (3 pending + 1 ready) => 4 passes drives the slower one.
        assert_eq!(rounds, 4);
        assert_eq!(done.load(Ordering::Relaxed), 2);
        assert_eq!(exec.pending(), 0);
        assert_eq!(exec.len(), 0); // slots reclaimed on completion
    }

    #[test]
    fn spawn_past_capacity_is_rejected_not_allocated() {
        let done = AtomicU32::new(0);
        let f1 = pin!(Countdown {
            left: 0,
            done: &done
        });
        let f2 = pin!(Countdown {
            left: 0,
            done: &done
        });
        let mut exec = BoundedExecutor::<1>::new();
        assert_eq!(exec.spawn(f1), Ok(0));
        assert_eq!(exec.spawn(f2), Err(AsyncError::Full));
    }

    #[test]
    fn run_once_reports_progress() {
        let done = AtomicU32::new(0);
        let f = pin!(Countdown {
            left: 2,
            done: &done
        });
        let mut exec = BoundedExecutor::<2>::new();
        exec.spawn(f).unwrap();

        let s0 = exec.run_once(); // 2 -> 1 pending
        assert_eq!((s0.polled, s0.completed, s0.pending), (1, 0, 1));
        exec.run_once(); // 1 -> 0
        let s2 = exec.run_once(); // ready
        assert_eq!(s2.completed, 1);
        assert_eq!(exec.pending(), 0);
    }

    #[test]
    fn stalls_are_bounded_not_infinite() {
        // A future that never resolves must make run_to_idle return Stalled, not hang.
        struct Never;
        impl Future for Never {
            type Output = ();
            fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
                Poll::Pending
            }
        }
        let n = pin!(Never);
        let mut exec = BoundedExecutor::<1>::new();
        exec.spawn(n).unwrap();
        assert_eq!(exec.run_to_idle(8), Err(AsyncError::Stalled));
    }
}
