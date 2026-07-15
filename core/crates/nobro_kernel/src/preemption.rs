//! Optional bounded preemption contracts.
//!
//! P-ISR permits only constant-time acknowledgement/timestamp/ready/event
//! handoff. It never invokes arbitrary application callbacks. P-SLICE owns a
//! separate PSP stack per task and asks a platform port to pend a context
//! switch when the lock-free execution sentinel reports a budget overrun.
//! Neither profile is linked by default and neither is implied on non-Cortex-M
//! targets.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::{module_code, Criticality, ExecutionSentinel, ModuleId};

/// Lock-free ISR-to-executor publication. Saturated event bits remain set until
/// drained; exact ready bits are idempotent.
pub struct InterruptHandoff {
    ready: AtomicU32,
    events: AtomicU32,
    overflows: AtomicU32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InterruptReceipt {
    pub ready_mask: u32,
    pub event_mask: u32,
    pub overflows: u32,
}

impl InterruptHandoff {
    pub const fn new() -> Self {
        Self {
            ready: AtomicU32::new(0),
            events: AtomicU32::new(0),
            overflows: AtomicU32::new(0),
        }
    }

    /// ISR-safe bounded publication. A repeated event is not lost silently:
    /// the sticky overflow counter records that its one-bit mailbox was full.
    pub fn publish(&self, ready_mask: u32, event_mask: u32) {
        self.ready.fetch_or(ready_mask, Ordering::Release);
        let previous = self.events.fetch_or(event_mask, Ordering::AcqRel);
        if previous & event_mask != 0 {
            let _ = self
                .overflows
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    Some(value.saturating_add(1))
                });
        }
    }

    pub fn drain(&self) -> InterruptReceipt {
        InterruptReceipt {
            ready_mask: self.ready.swap(0, Ordering::AcqRel),
            event_mask: self.events.swap(0, Ordering::AcqRel),
            overflows: self.overflows.swap(0, Ordering::AcqRel),
        }
    }
}

impl Default for InterruptHandoff {
    fn default() -> Self {
        Self::new()
    }
}

/// One task-owned PSP stack. The platform port owns the saved PSP value; the
/// kernel owns bounds, attribution, and scheduling state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliceContext {
    pub module: ModuleId,
    pub stack_base: usize,
    pub stack_len: usize,
    pub saved_psp: usize,
    pub allows_fpu: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliceTask {
    pub module: ModuleId,
    pub criticality: Criticality,
    pub budget_us: u32,
    pub context: SliceContext,
}

impl SliceTask {
    pub const fn new(
        module: ModuleId,
        criticality: Criticality,
        budget_us: u32,
        stack_base: usize,
        stack_len: usize,
    ) -> Self {
        Self {
            module,
            criticality,
            budget_us,
            context: SliceContext {
                module,
                stack_base,
                stack_len,
                saved_psp: stack_base.saturating_add(stack_len),
                allows_fpu: false,
            },
        }
    }

    pub const fn allows_fpu(mut self, allows_fpu: bool) -> Self {
        self.context.allows_fpu = allows_fpu;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliceError {
    Full,
    Duplicate(ModuleId),
    InvalidBudget(ModuleId),
    InvalidStack(ModuleId),
    AliasedStack(ModuleId),
    DeadlineOverflow(ModuleId),
    AlreadyRunning,
    NoReadyTask,
    NoPendingSwitch,
    Port,
}

/// What the budget ISR asks the platform to do. The controller never claims
/// isolation: privilege/MPU switching remains a separate lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliceDecision {
    None,
    Pending {
        from: SliceContext,
        to: SliceContext,
        forced: bool,
    },
    Switch {
        from: SliceContext,
        to: SliceContext,
        forced: bool,
    },
}

/// Target port for PendSV/PSP switching. Implementations must preserve R4-R11,
/// 8-byte alignment, EXC_RETURN, and (when enabled) the extended/lazy FPU frame.
pub trait SlicePort {
    type Error;

    fn pend_switch(
        &mut self,
        from: SliceContext,
        to: SliceContext,
        forced: bool,
    ) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SliceSlot {
    task: SliceTask,
    ready: bool,
    suspended: bool,
    forced_suspends: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingSwitch {
    current: usize,
    next: usize,
    from: SliceContext,
    to: SliceContext,
    forced: bool,
}

pub struct SliceController<const N: usize> {
    slots: [Option<SliceSlot>; N],
    len: usize,
    current: Option<usize>,
    cursor: usize,
    pending: Option<PendingSwitch>,
}

impl<const N: usize> SliceController<N> {
    /// Cortex-M basic frame (32) + R4-R11 (32) + 16-byte canary + 32-byte
    /// minimum handler/task margin. FPU tasks also reserve the 72-byte hardware
    /// extended frame plus software-saved S16-S31 (64 bytes). These are
    /// admission floors, not measured stack claims.
    const BASIC_STACK_FLOOR: usize = 112;
    const FPU_EXTRA: usize = 136;

    pub const fn new() -> Self {
        Self {
            slots: [None; N],
            len: 0,
            current: None,
            cursor: 0,
            pending: None,
        }
    }

    pub fn add(&mut self, task: SliceTask) -> Result<usize, SliceError> {
        if self.len == N {
            return Err(SliceError::Full);
        }
        if task.budget_us == 0 {
            return Err(SliceError::InvalidBudget(task.module));
        }
        let required = Self::BASIC_STACK_FLOOR
            + if task.context.allows_fpu {
                Self::FPU_EXTRA
            } else {
                0
            };
        if task.context.stack_base & 7 != 0
            || task.context.stack_len < required
            || task.context.stack_len & 7 != 0
        {
            return Err(SliceError::InvalidStack(task.module));
        }
        if self
            .slots
            .iter()
            .flatten()
            .any(|slot| slot.task.module == task.module)
        {
            return Err(SliceError::Duplicate(task.module));
        }
        let task_end = task
            .context
            .stack_base
            .checked_add(task.context.stack_len)
            .ok_or(SliceError::InvalidStack(task.module))?;
        if self.slots.iter().flatten().any(|slot| {
            let other = slot.task.context;
            let other_end = other.stack_base.saturating_add(other.stack_len);
            task.context.stack_base < other_end && other.stack_base < task_end
        }) {
            return Err(SliceError::AliasedStack(task.module));
        }
        let index = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or(SliceError::Full)?;
        self.slots[index] = Some(SliceSlot {
            task,
            ready: false,
            suspended: false,
            forced_suspends: 0,
        });
        self.len += 1;
        Ok(index)
    }

    pub fn mark_ready(&mut self, module: ModuleId) -> bool {
        let Some(slot) = self
            .slots
            .iter_mut()
            .flatten()
            .find(|slot| slot.task.module == module)
        else {
            return false;
        };
        slot.ready = true;
        slot.suspended = false;
        true
    }

    fn choose(&self, exclude: Option<usize>) -> Option<usize> {
        let mut selected: Option<usize> = None;
        for offset in 0..N {
            let index = (self.cursor + offset) % N.max(1);
            if Some(index) == exclude {
                continue;
            }
            let Some(slot) = self.slots[index] else {
                continue;
            };
            if !slot.ready || slot.suspended {
                continue;
            }
            selected = match selected {
                None => Some(index),
                Some(previous) => {
                    let previous_slot = self.slots[previous].expect("selected slice slot");
                    if slot.task.criticality > previous_slot.task.criticality {
                        Some(index)
                    } else {
                        Some(previous)
                    }
                }
            };
        }
        selected
    }

    /// Select and arm the first runnable context. The lock-free sentinel is
    /// armed before the port starts thread mode, so a task that never yields is
    /// still observable from the admitted budget interrupt. No unmonitored
    /// start API is provided.
    pub fn start_next_at(
        &mut self,
        now_us: u64,
        sentinel: &ExecutionSentinel,
    ) -> Result<SliceContext, SliceError> {
        if self.current.is_some() {
            return Err(SliceError::AlreadyRunning);
        }
        let next = self.choose(None).ok_or(SliceError::NoReadyTask)?;
        let slot = self.slots[next].expect("selected slice slot");
        let deadline = now_us
            .checked_add(u64::from(slot.task.budget_us))
            .ok_or(SliceError::DeadlineOverflow(slot.task.module))?;
        self.current = Some(next);
        self.cursor = (next + 1) % N.max(1);
        sentinel.arm(slot.task.module, deadline);
        Ok(slot.task.context)
    }

    /// Called from the admitted budget ISR. A non-yielding current task is
    /// suspended and the platform receives one bounded PendSV request.
    pub fn on_budget_interrupt(
        &mut self,
        now_us: u64,
        sentinel: &ExecutionSentinel,
        port: &mut impl SlicePort,
    ) -> Result<SliceDecision, SliceError> {
        if let Some(pending) = self.pending {
            return Ok(SliceDecision::Pending {
                from: pending.from,
                to: pending.to,
                forced: pending.forced,
            });
        }
        let Some(stuck) = sentinel.check(now_us) else {
            return Ok(SliceDecision::None);
        };
        let current = self.current.ok_or(SliceError::NoReadyTask)?;
        let from = self.slots[current].expect("current slice slot");
        if module_code(from.task.module) != stuck.module_code {
            return Ok(SliceDecision::None);
        }
        self.force_current_after_budget_fault(port)
    }

    fn force_current_after_budget_fault(
        &mut self,
        port: &mut impl SlicePort,
    ) -> Result<SliceDecision, SliceError> {
        let current = self.current.ok_or(SliceError::NoReadyTask)?;
        let from = self.slots[current].expect("current slice slot");
        let next = self.choose(Some(current)).ok_or(SliceError::NoReadyTask)?;
        let to = self.slots[next].expect("next slice slot");
        // Queue first. A failed port request leaves scheduling state and the
        // current sentinel unchanged, so callers can retry or escalate safely.
        // A successful request is still only pending: PendSV is deliberately
        // configured at or below the BASEPRI ceiling, so a critical-section
        // overrun may defer the actual architectural switch until the section
        // exits. Scheduler state and the budget sentinel therefore commit only
        // from `commit_pending_switch_at`, after the port has completed the
        // switch. This keeps watchdog/recovery attribution on the old task if
        // it never releases the ceiling.
        port.pend_switch(from.task.context, to.task.context, true)
            .map_err(|_| SliceError::Port)?;
        self.pending = Some(PendingSwitch {
            current,
            next,
            from: from.task.context,
            to: to.task.context,
            forced: true,
        });
        Ok(SliceDecision::Switch {
            from: from.task.context,
            to: to.task.context,
            forced: true,
        })
    }

    /// Commit a previously queued context switch after the platform PendSV
    /// path has actually installed the next PSP context. Until this method is
    /// called, the old task remains current and its sentinel remains armed.
    pub fn commit_pending_switch_at(
        &mut self,
        now_us: u64,
        sentinel: &ExecutionSentinel,
    ) -> Result<SliceDecision, SliceError> {
        let pending = self.pending.ok_or(SliceError::NoPendingSwitch)?;
        let to = self.slots[pending.next].expect("next slice slot");
        let deadline = now_us
            .checked_add(u64::from(to.task.budget_us))
            .ok_or(SliceError::DeadlineOverflow(to.task.module))?;
        self.pending = None;
        let current_slot = self.slots[pending.current]
            .as_mut()
            .expect("current slice slot");
        current_slot.suspended = true;
        current_slot.ready = false;
        current_slot.forced_suspends = current_slot.forced_suspends.saturating_add(1);
        self.current = Some(pending.next);
        self.cursor = (pending.next + 1) % N.max(1);
        sentinel.disarm();
        sentinel.arm(to.task.module, deadline);
        Ok(SliceDecision::Switch {
            from: pending.from,
            to: pending.to,
            forced: pending.forced,
        })
    }

    pub fn forced_suspends(&self, module: ModuleId) -> Option<u32> {
        self.slots
            .iter()
            .flatten()
            .find(|slot| slot.task.module == module)
            .map(|slot| slot.forced_suspends)
    }
}

impl<const N: usize> Default for SliceController<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Port {
        switches: u32,
        fail: bool,
    }

    impl SlicePort for Port {
        type Error = ();

        fn pend_switch(
            &mut self,
            _from: SliceContext,
            _to: SliceContext,
            forced: bool,
        ) -> Result<(), Self::Error> {
            assert!(forced);
            if self.fail {
                Err(())
            } else {
                self.switches += 1;
                Ok(())
            }
        }
    }

    #[test]
    fn isr_handoff_is_bounded_and_reports_repeated_events() {
        let handoff = InterruptHandoff::new();
        handoff.publish(0b001, 0b010);
        handoff.publish(0b100, 0b010);
        assert_eq!(
            handoff.drain(),
            InterruptReceipt {
                ready_mask: 0b101,
                event_mask: 0b010,
                overflows: 1,
            }
        );
        assert_eq!(handoff.drain(), InterruptReceipt::default());
    }

    #[test]
    fn controller_validates_owned_psp_stacks_and_prefers_criticality() {
        let mut storage = [0u64; 64];
        let base = storage.as_mut_ptr() as usize;
        let mut controller = SliceController::<2>::new();
        controller
            .add(SliceTask::new(
                ModuleId::Sensor,
                Criticality::Driver,
                100,
                base,
                256,
            ))
            .unwrap();
        controller
            .add(SliceTask::new(
                ModuleId::Actuator,
                Criticality::HardRealtime,
                100,
                base + 256,
                256,
            ))
            .unwrap();
        assert!(controller.mark_ready(ModuleId::Sensor));
        assert!(controller.mark_ready(ModuleId::Actuator));
        let sentinel = ExecutionSentinel::new();
        assert_eq!(
            controller.start_next_at(0, &sentinel).unwrap().module,
            ModuleId::Actuator
        );
        let mut port = Port::default();
        let decision = controller
            .on_budget_interrupt(101, &sentinel, &mut port)
            .unwrap();
        assert!(matches!(
            decision,
            SliceDecision::Switch {
                from: SliceContext {
                    module: ModuleId::Actuator,
                    ..
                },
                to: SliceContext {
                    module: ModuleId::Sensor,
                    ..
                },
                forced: true,
            }
        ));
        assert_eq!(port.switches, 1);
        assert_eq!(controller.forced_suspends(ModuleId::Actuator), Some(0));
        assert_eq!(controller.current, Some(1));
        assert!(matches!(
            controller.on_budget_interrupt(150, &sentinel, &mut port),
            Ok(SliceDecision::Pending {
                from: SliceContext {
                    module: ModuleId::Actuator,
                    ..
                },
                to: SliceContext {
                    module: ModuleId::Sensor,
                    ..
                },
                forced: true,
            })
        ));
        assert_eq!(
            sentinel.check(150).map(|stuck| stuck.module_code),
            Some(module_code(ModuleId::Actuator))
        );
        let committed = controller
            .commit_pending_switch_at(150, &sentinel)
            .expect("pending switch commits");
        assert!(matches!(
            committed,
            SliceDecision::Switch {
                from: SliceContext {
                    module: ModuleId::Actuator,
                    ..
                },
                to: SliceContext {
                    module: ModuleId::Sensor,
                    ..
                },
                forced: true,
            }
        ));
        assert_eq!(controller.forced_suspends(ModuleId::Actuator), Some(1));
        assert_eq!(controller.current, Some(0));
        assert_eq!(sentinel.check(249), None);
        assert_eq!(
            sentinel.check(251).map(|stuck| stuck.module_code),
            Some(module_code(ModuleId::Sensor))
        );
    }

    #[test]
    fn controller_rejects_stack_aliases_and_rolls_back_port_failure() {
        let mut storage = [0u64; 64];
        let base = storage.as_mut_ptr() as usize;
        let mut controller = SliceController::<2>::new();
        controller
            .add(SliceTask::new(
                ModuleId::Sensor,
                Criticality::System,
                100,
                base,
                256,
            ))
            .unwrap();
        assert_eq!(
            controller.add(SliceTask::new(
                ModuleId::Radio,
                Criticality::Driver,
                100,
                base + 128,
                256,
            )),
            Err(SliceError::AliasedStack(ModuleId::Radio))
        );
        controller
            .add(SliceTask::new(
                ModuleId::Radio,
                Criticality::Driver,
                100,
                base + 256,
                256,
            ))
            .unwrap();
        controller.mark_ready(ModuleId::Sensor);
        controller.mark_ready(ModuleId::Radio);
        let sentinel = ExecutionSentinel::new();
        controller.start_next_at(0, &sentinel).unwrap();
        let mut failing = Port {
            switches: 0,
            fail: true,
        };
        assert_eq!(
            controller.on_budget_interrupt(101, &sentinel, &mut failing),
            Err(SliceError::Port)
        );
        assert_eq!(controller.forced_suspends(ModuleId::Sensor), Some(0));
        assert_eq!(controller.current, Some(0));
    }

    #[test]
    fn timed_start_arms_the_budget_sentinel_before_thread_mode_runs() {
        let mut storage = [0u64; 32];
        let base = storage.as_mut_ptr() as usize;
        let mut controller = SliceController::<1>::new();
        controller
            .add(SliceTask::new(
                ModuleId::Sensor,
                Criticality::System,
                100,
                base,
                256,
            ))
            .unwrap();
        controller.mark_ready(ModuleId::Sensor);
        let sentinel = ExecutionSentinel::new();
        controller.start_next_at(1_000, &sentinel).unwrap();
        assert_eq!(
            controller.start_next_at(1_001, &sentinel),
            Err(SliceError::AlreadyRunning)
        );
        assert_eq!(sentinel.check(1_100), None);
        assert_eq!(
            sentinel.check(1_101).map(|stuck| stuck.module_code),
            Some(module_code(ModuleId::Sensor))
        );
    }

    #[test]
    fn fpu_slice_reserves_both_hardware_and_software_floating_point_frames() {
        let mut storage = [0u64; 64];
        let base = storage.as_mut_ptr() as usize;
        let mut controller = SliceController::<1>::new();
        assert_eq!(
            controller.add(
                SliceTask::new(ModuleId::Sensor, Criticality::System, 100, base, 192)
                    .allows_fpu(true)
            ),
            Err(SliceError::InvalidStack(ModuleId::Sensor))
        );
        assert!(controller
            .add(
                SliceTask::new(ModuleId::Sensor, Criticality::System, 100, base, 256)
                    .allows_fpu(true)
            )
            .is_ok());
    }
}
