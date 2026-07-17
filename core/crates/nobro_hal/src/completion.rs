//! Reusable, allocation-free handoff from a hardware-completion ISR to a future.
//!
//! The ISR that calls [`CompletionCell::complete_from_isr`] must be masked by
//! the target's process-wide critical-section ceiling. High-priority deadline
//! and watchdog ISRs may only use their lock-free handoff paths.

use core::cell::RefCell;
use core::sync::atomic::{AtomicU8, Ordering};
use core::task::{Context, Waker};

use critical_section::Mutex;

const IDLE: u8 = 0;
const ARMED: u8 = 1;
const COMPLETE: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionError {
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagedTransferError {
    Empty,
    LengthMismatch,
    TooLong,
}

/// Validated size contract for a DMA provider that copies through fixed
/// staging instead of exposing caller memory to hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StagedTransferPlan {
    words: usize,
}

impl StagedTransferPlan {
    pub const fn new(
        source_words: usize,
        destination_words: usize,
        capacity_words: usize,
    ) -> Result<Self, StagedTransferError> {
        if source_words == 0 {
            return Err(StagedTransferError::Empty);
        }
        if source_words != destination_words {
            return Err(StagedTransferError::LengthMismatch);
        }
        if source_words > capacity_words {
            return Err(StagedTransferError::TooLong);
        }
        Ok(Self {
            words: source_words,
        })
    }

    pub const fn words(self) -> usize {
        self.words
    }
}

/// A single reusable completion slot. It stores at most one task waker and is
/// safe to place in static storage.
pub struct CompletionCell {
    state: AtomicU8,
    waker: Mutex<RefCell<Option<Waker>>>,
}

impl CompletionCell {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(IDLE),
            waker: Mutex::new(RefCell::new(None)),
        }
    }

    /// Prepare one operation before its interrupt source is enabled.
    pub fn arm(&self, waker: &Waker) -> Result<(), CompletionError> {
        critical_section::with(|cs| {
            if self.state.load(Ordering::Acquire) != IDLE {
                return Err(CompletionError::Busy);
            }
            *self.waker.borrow(cs).borrow_mut() = Some(waker.clone());
            self.state.store(ARMED, Ordering::Release);
            Ok(())
        })
    }

    /// Refresh the registered waker and consume a completed operation.
    pub fn poll_complete(&self, cx: &Context<'_>) -> bool {
        critical_section::with(|cs| {
            if self.state.load(Ordering::Acquire) == COMPLETE {
                self.state.store(IDLE, Ordering::Release);
                self.waker.borrow(cs).borrow_mut().take();
                return true;
            }
            if self.state.load(Ordering::Acquire) == ARMED {
                let mut slot = self.waker.borrow(cs).borrow_mut();
                if slot
                    .as_ref()
                    .is_none_or(|registered| !registered.will_wake(cx.waker()))
                {
                    *slot = Some(cx.waker().clone());
                }
            }
            false
        })
    }

    /// Publish completion and wake the registered task.
    ///
    /// Call from a completion ISR whose priority obeys the module-level
    /// critical-section requirement. Stray or cancelled interrupts are ignored.
    pub fn complete_from_isr(&self) -> bool {
        let (completed, wake) = critical_section::with(|cs| {
            if self.state.load(Ordering::Acquire) != ARMED {
                return (false, None);
            }
            self.state.store(COMPLETE, Ordering::Release);
            (true, self.waker.borrow(cs).borrow_mut().take())
        });
        if !completed {
            return false;
        }
        if let Some(waker) = wake {
            waker.wake();
        }
        true
    }

    /// Disarm a pending operation. A later interrupt for it becomes a no-op.
    pub fn cancel(&self) -> bool {
        critical_section::with(|cs| {
            let was_armed = self.state.load(Ordering::Acquire) != IDLE;
            self.state.store(IDLE, Ordering::Release);
            self.waker.borrow(cs).borrow_mut().take();
            was_armed
        })
    }

    pub fn is_armed(&self) -> bool {
        self.state.load(Ordering::Acquire) == ARMED
    }

    /// True from arming until completion is consumed or cancelled.
    pub fn is_busy(&self) -> bool {
        self.state.load(Ordering::Acquire) != IDLE
    }
}

impl Default for CompletionCell {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    extern crate std;
    use self::std::sync::Arc;
    use self::std::task::{Wake, Waker};

    struct CountWake(AtomicUsize);

    impl Wake for CountWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }

    #[test]
    fn completion_wakes_once_and_slot_is_reusable() {
        let cell = CompletionCell::new();
        let count = Arc::new(CountWake(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&count));
        let cx = Context::from_waker(&waker);

        cell.arm(&waker).unwrap();
        assert!(cell.is_busy());
        assert_eq!(cell.arm(&waker), Err(CompletionError::Busy));
        assert!(cell.complete_from_isr());
        assert!(cell.is_busy());
        assert!(!cell.complete_from_isr());
        assert_eq!(count.0.load(Ordering::Acquire), 1);
        assert!(cell.poll_complete(&cx));
        assert!(!cell.is_busy());
        assert!(!cell.poll_complete(&cx));

        cell.arm(&waker).unwrap();
        assert!(cell.cancel());
        assert!(!cell.complete_from_isr());
        assert_eq!(count.0.load(Ordering::Acquire), 1);
        cell.arm(&waker).unwrap();
    }

    #[test]
    fn staged_transfer_plan_matches_shadow_model() {
        for capacity in 0..=16 {
            for source in 0..=17 {
                for destination in 0..=17 {
                    let expected = if source == 0 {
                        Err(StagedTransferError::Empty)
                    } else if source != destination {
                        Err(StagedTransferError::LengthMismatch)
                    } else if source > capacity {
                        Err(StagedTransferError::TooLong)
                    } else {
                        Ok(source)
                    };
                    assert_eq!(
                        StagedTransferPlan::new(source, destination, capacity)
                            .map(StagedTransferPlan::words),
                        expected
                    );
                }
            }
        }
    }
}
