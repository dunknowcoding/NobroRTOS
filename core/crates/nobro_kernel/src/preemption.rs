//! Optional bounded preemption contracts.
//!
//! P-ISR permits only constant-time acknowledgement/timestamp/ready/event
//! handoff. It never invokes arbitrary application callbacks. P-SLICE owns a
//! separate PSP stack per task and asks a platform port to pend a context
//! switch when the lock-free execution sentinel reports a budget overrun.
//! Neither profile is linked by default and neither is implied on non-Cortex-M
//! targets.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::{
    module_code, module_from_code, Action, Criticality, ExecutionSentinel, FaultContext,
    FaultPolicy, FaultSource, HealthCounters, HealthFault, ModuleId, RecoveryOutcome, Runtime,
    RuntimeError,
};

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

const FORCED_MODULE_MASK: u32 = 0x1ff;
const FORCED_CONFLICT_MODULE_SHIFT: u32 = 9;
const FORCED_COUNT_SHIFT: u32 = 18;
const FORCED_COUNT_MASK: u32 = 0x1fff;
const FORCED_IDENTITY_CONFLICT: u32 = 1 << 31;

/// Largest exact occurrence count retained by one forced-suspension handoff.
/// Further publications remain represented by this saturated value until the
/// privileged dispatcher drains the handoff.
pub const FORCED_SUSPEND_MAX_OCCURRENCES: u32 = FORCED_COUNT_MASK;

/// One allocation-free forced-suspension publication from PendSV to the
/// privileged recovery dispatcher.
///
/// The first module identity is retained until drained. Repeated publications
/// saturate a count; a second identity sets a sticky conflict bit so recovery
/// fails closed instead of acting on the wrong module.
pub struct ForcedSuspendHandoff {
    state: AtomicU32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForcedSuspendReceipt {
    pub module: ModuleId,
    pub occurrences: u32,
    pub identity_conflict: bool,
    /// First different identity observed before the handoff was drained.
    pub conflicting_module: Option<ModuleId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForcedSuspendHandoffError {
    CorruptModuleCode(u32),
    CorruptConflictingModuleCode(u32),
}

impl ForcedSuspendHandoff {
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
        }
    }

    /// ISR-safe bounded publication after the architectural switch commits.
    pub fn publish(&self, module: ModuleId) {
        let code = module_code(module);
        debug_assert!(code != 0 && code <= FORCED_MODULE_MASK);
        let _ = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                if current == 0 {
                    return Some(code | (1 << FORCED_COUNT_SHIFT));
                }
                let retained = current & FORCED_MODULE_MASK;
                let retained_conflict =
                    (current >> FORCED_CONFLICT_MODULE_SHIFT) & FORCED_MODULE_MASK;
                let count = (current >> FORCED_COUNT_SHIFT) & FORCED_COUNT_MASK;
                let next_count = count.saturating_add(1).min(FORCED_SUSPEND_MAX_OCCURRENCES);
                let conflict_module = if retained == code || retained_conflict != 0 {
                    retained_conflict
                } else {
                    code
                };
                let conflict = if conflict_module == 0 {
                    0
                } else {
                    FORCED_IDENTITY_CONFLICT
                };
                Some(
                    retained
                        | (conflict_module << FORCED_CONFLICT_MODULE_SHIFT)
                        | (next_count << FORCED_COUNT_SHIFT)
                        | conflict,
                )
            });
    }

    pub fn drain(&self) -> Result<Option<ForcedSuspendReceipt>, ForcedSuspendHandoffError> {
        let state = self.state.swap(0, Ordering::AcqRel);
        if state == 0 {
            return Ok(None);
        }
        let code = state & FORCED_MODULE_MASK;
        let module =
            module_from_code(code).ok_or(ForcedSuspendHandoffError::CorruptModuleCode(code))?;
        let conflicting_code = (state >> FORCED_CONFLICT_MODULE_SHIFT) & FORCED_MODULE_MASK;
        let conflicting_module = if state & FORCED_IDENTITY_CONFLICT == 0 {
            None
        } else {
            Some(module_from_code(conflicting_code).ok_or(
                ForcedSuspendHandoffError::CorruptConflictingModuleCode(conflicting_code),
            )?)
        };
        Ok(Some(ForcedSuspendReceipt {
            module,
            occurrences: (state >> FORCED_COUNT_SHIFT) & FORCED_COUNT_MASK,
            identity_conflict: state & FORCED_IDENTITY_CONFLICT != 0,
            conflicting_module,
        }))
    }
}

impl Default for ForcedSuspendHandoff {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForcedSuspendRouteError {
    IdentityConflict {
        first_module: ModuleId,
        conflicting_module: Option<ModuleId>,
        occurrences: u32,
    },
    Runtime(RuntimeError),
}

impl From<RuntimeError> for ForcedSuspendRouteError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

struct ForcedSuspendPolicy;

impl FaultPolicy for ForcedSuspendPolicy {
    fn decide(
        &mut self,
        _module: ModuleId,
        _fault: &HealthFault,
        _counters: &HealthCounters,
    ) -> Action {
        Action::RebootModule
    }
}

/// Route a committed forced suspension into module-scoped recovery.
///
/// Call this from privileged dispatcher context after draining the PendSV
/// handoff, never from the budget ISR itself. A conflicting identity is refused;
/// a valid receipt always selects `RebootModule`, not whole-board reset.
pub fn route_forced_suspend<
    const STARTUP: usize,
    const QUOTAS: usize,
    const MAILBOX: usize,
    const ALARMS: usize,
    const KV: usize,
    const HEALTH: usize,
    const LOG: usize,
>(
    runtime: &mut Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
    receipt: ForcedSuspendReceipt,
    now_us: u64,
) -> Result<RecoveryOutcome, ForcedSuspendRouteError> {
    if receipt.identity_conflict {
        return Err(ForcedSuspendRouteError::IdentityConflict {
            first_module: receipt.module,
            conflicting_module: receipt.conflicting_module,
            occurrences: receipt.occurrences,
        });
    }
    let fault = HealthFault::new(
        crate::KernelError::DeadlineMissed,
        FaultContext::new(FaultSource::Scheduler, 1, receipt.occurrences, 0),
    );
    runtime
        .record_fault(receipt.module, fault, now_us, &mut ForcedSuspendPolicy)
        .map_err(Into::into)
}

/// One task-owned PSP stack. The platform port owns the saved PSP value; the
/// kernel owns bounds, attribution, and scheduling state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SliceProtection {
    #[default]
    Privileged,
    /// The target port must return this context to unprivileged Thread mode
    /// and install its module-specific MPU bank before exception return.
    UnprivilegedMpu,
}

/// Isolation capabilities of one architectural slice-switch port.
///
/// The default is deliberately privileged-only. A port must opt in after it
/// can restore unprivileged Thread mode and the matching MPU bank atomically
/// with the PSP context. This keeps an isolation request from silently
/// degrading on a target that only implements context switching.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlicePortCapabilities {
    pub unprivileged_mpu: bool,
    pub mpu_regions: u8,
}

impl SlicePortCapabilities {
    pub const fn privileged_only() -> Self {
        Self {
            unprivileged_mpu: false,
            mpu_regions: 0,
        }
    }

    pub const fn pmsav7_m(mpu_regions: u8) -> Self {
        Self {
            unprivileged_mpu: mpu_regions != 0,
            mpu_regions,
        }
    }

    pub const fn admits(self, protection: SliceProtection) -> bool {
        match protection {
            SliceProtection::Privileged => true,
            SliceProtection::UnprivilegedMpu => self.unprivileged_mpu && self.mpu_regions != 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliceContext {
    pub module: ModuleId,
    pub stack_base: usize,
    pub stack_len: usize,
    pub saved_psp: usize,
    pub allows_fpu: bool,
    pub protection: SliceProtection,
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
                protection: SliceProtection::Privileged,
            },
        }
    }

    pub const fn allows_fpu(mut self, allows_fpu: bool) -> Self {
        self.context.allows_fpu = allows_fpu;
        self
    }

    /// Require the target port to use an unprivileged PSP context with a
    /// module-specific MPU bank. Unsupported ports must reject the task during
    /// their setup; silently running it privileged violates this contract.
    pub const fn unprivileged_mpu(mut self) -> Self {
        self.context.protection = SliceProtection::UnprivilegedMpu;
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
    IsolationUnsupported(ModuleId),
    Cancelled(ModuleId),
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
/// 8-byte alignment, EXC_RETURN, and (when enabled) the extended/lazy FPU
/// frame. A context marked [`SliceProtection::UnprivilegedMpu`] must never be
/// started through a privileged/no-MPU fallback.
pub trait SlicePort {
    type Error;

    /// Report capabilities used by fail-closed task admission.
    ///
    /// Existing ports remain privileged-only until they explicitly override
    /// this method.
    fn capabilities(&self) -> SlicePortCapabilities {
        SlicePortCapabilities::privileged_only()
    }

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
    cancelled: bool,
    forced_suspends: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PostSwitchState {
    Ready,
    Blocked,
    Suspended,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingSwitch {
    current: usize,
    next: usize,
    from: SliceContext,
    to: SliceContext,
    forced: bool,
    from_state: PostSwitchState,
}

#[repr(C)]
struct CortexMSoftwareFrame {
    r4_r11: [u32; 8],
}

#[repr(C)]
struct CortexMBasicExceptionFrame {
    r0_r3_r12_lr_pc_xpsr: [u32; 8],
}

#[repr(C)]
struct CortexMFpuHardwareExtension {
    s0_s15_fpscr_reserved: [u32; 18],
}

#[repr(C)]
struct CortexMFpuSoftwareFrame {
    s16_s31: [u32; 16],
}

const SLICE_CANARY_BYTES: usize = 16;
const SLICE_RUNTIME_MARGIN_BYTES: usize = 32;

pub struct SliceController<const N: usize> {
    slots: [Option<SliceSlot>; N],
    len: usize,
    current: Option<usize>,
    cursor: usize,
    pending: Option<PendingSwitch>,
}

impl<const N: usize> SliceController<N> {
    /// Named architecture layouts plus explicit canary/runtime margin. These
    /// are admission floors, not measured stack claims.
    const BASIC_STACK_FLOOR: usize = core::mem::size_of::<CortexMSoftwareFrame>()
        + core::mem::size_of::<CortexMBasicExceptionFrame>()
        + SLICE_CANARY_BYTES
        + SLICE_RUNTIME_MARGIN_BYTES;
    const FPU_EXTRA: usize = core::mem::size_of::<CortexMFpuHardwareExtension>()
        + core::mem::size_of::<CortexMFpuSoftwareFrame>();

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
        self.add_with_capabilities(task, SlicePortCapabilities::privileged_only())
    }

    /// Admit a task against the exact architectural port that will execute it.
    ///
    /// An isolated task cannot enter the controller through [`add`](Self::add)
    /// or a port that reports only privileged execution.
    pub fn add_for_port(
        &mut self,
        task: SliceTask,
        port: &impl SlicePort,
    ) -> Result<usize, SliceError> {
        self.add_with_capabilities(task, port.capabilities())
    }

    pub fn add_with_capabilities(
        &mut self,
        task: SliceTask,
        capabilities: SlicePortCapabilities,
    ) -> Result<usize, SliceError> {
        if self.len == N {
            return Err(SliceError::Full);
        }
        if !capabilities.admits(task.context.protection) {
            return Err(SliceError::IsolationUnsupported(task.module));
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
            cancelled: false,
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
        if slot.cancelled {
            return false;
        }
        slot.ready = true;
        slot.suspended = false;
        true
    }

    /// Restart a cancelled or forcibly suspended task after its owner has
    /// rebuilt any module-local state. This is the only route that clears a
    /// cancellation; an ordinary wake cannot resurrect cancelled work.
    pub fn restart(&mut self, module: ModuleId) -> bool {
        let Some(slot) = self
            .slots
            .iter_mut()
            .flatten()
            .find(|slot| slot.task.module == module)
        else {
            return false;
        };
        slot.cancelled = false;
        slot.suspended = false;
        slot.ready = true;
        true
    }

    pub fn is_cancelled(&self, module: ModuleId) -> Option<bool> {
        self.slots
            .iter()
            .flatten()
            .find(|slot| slot.task.module == module)
            .map(|slot| slot.cancelled)
    }

    pub fn is_suspended(&self, module: ModuleId) -> Option<bool> {
        self.slots
            .iter()
            .flatten()
            .find(|slot| slot.task.module == module)
            .map(|slot| slot.suspended)
    }

    /// Publish a ready transition and proactively request a context switch when
    /// the new task is more urgent than the running task.
    ///
    /// This is the portable ready-to-preempt contract: board ports decide how
    /// to pend their architectural switch, while the controller keeps the
    /// transition pending until the port confirms it through
    /// [`commit_pending_switch_at`](Self::commit_pending_switch_at). Equal or
    /// lower urgency remains cooperative and incurs no port call.
    pub fn mark_ready_and_request_preemption(
        &mut self,
        module: ModuleId,
        port: &mut impl SlicePort,
    ) -> Result<SliceDecision, SliceError> {
        let next = self
            .slots
            .iter()
            .position(|slot| slot.is_some_and(|slot| slot.task.module == module))
            .ok_or(SliceError::NoReadyTask)?;
        let next_slot = self.slots[next].as_mut().ok_or(SliceError::NoReadyTask)?;
        if next_slot.cancelled {
            return Err(SliceError::Cancelled(module));
        }
        next_slot.ready = true;
        next_slot.suspended = false;

        if let Some(pending) = self.pending {
            return Ok(SliceDecision::Pending {
                from: pending.from,
                to: pending.to,
                forced: pending.forced,
            });
        }
        let Some(current) = self.current else {
            return Ok(SliceDecision::None);
        };
        if current == next {
            return Ok(SliceDecision::None);
        }
        let from = self.slots[current].ok_or(SliceError::NoReadyTask)?;
        let to = self.slots[next].ok_or(SliceError::NoReadyTask)?;
        if to.task.criticality <= from.task.criticality {
            return Ok(SliceDecision::None);
        }
        port.pend_switch(from.task.context, to.task.context, false)
            .map_err(|_| SliceError::Port)?;
        self.pending = Some(PendingSwitch {
            current,
            next,
            from: from.task.context,
            to: to.task.context,
            forced: false,
            from_state: PostSwitchState::Ready,
        });
        Ok(SliceDecision::Switch {
            from: from.task.context,
            to: to.task.context,
            forced: false,
        })
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
            if !slot.ready || slot.suspended || slot.cancelled {
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

    /// Cooperatively yield the current task to the next admitted ready task.
    /// Equal-priority tasks rotate by the controller cursor, while higher
    /// criticality remains dominant.
    pub fn yield_current(
        &mut self,
        port: &mut impl SlicePort,
    ) -> Result<SliceDecision, SliceError> {
        self.switch_current(port, PostSwitchState::Ready)
    }

    /// Block the current task until an explicit [`mark_ready`](Self::mark_ready)
    /// wake. The state changes only after the architectural switch commits.
    pub fn block_current(
        &mut self,
        port: &mut impl SlicePort,
    ) -> Result<SliceDecision, SliceError> {
        self.switch_current(port, PostSwitchState::Blocked)
    }

    /// Cancel a task without allocation or an asynchronous destructor. A
    /// non-running task is cancelled immediately. The running task is marked
    /// cancelled only after PendSV installs another context.
    pub fn cancel(
        &mut self,
        module: ModuleId,
        port: &mut impl SlicePort,
    ) -> Result<SliceDecision, SliceError> {
        let index = self
            .slots
            .iter()
            .position(|slot| slot.is_some_and(|slot| slot.task.module == module))
            .ok_or(SliceError::NoReadyTask)?;
        if self.current == Some(index) {
            return self.switch_current(port, PostSwitchState::Cancelled);
        }
        let slot = self.slots[index].as_mut().ok_or(SliceError::NoReadyTask)?;
        slot.ready = false;
        slot.suspended = false;
        slot.cancelled = true;
        Ok(SliceDecision::None)
    }

    fn switch_current(
        &mut self,
        port: &mut impl SlicePort,
        from_state: PostSwitchState,
    ) -> Result<SliceDecision, SliceError> {
        if let Some(pending) = self.pending {
            return Ok(SliceDecision::Pending {
                from: pending.from,
                to: pending.to,
                forced: pending.forced,
            });
        }
        let current = self.current.ok_or(SliceError::NoReadyTask)?;
        let from = self.slots[current].ok_or(SliceError::NoReadyTask)?;
        let next = self.choose(Some(current)).ok_or(SliceError::NoReadyTask)?;
        let to = self.slots[next].ok_or(SliceError::NoReadyTask)?;
        port.pend_switch(from.task.context, to.task.context, false)
            .map_err(|_| SliceError::Port)?;
        self.pending = Some(PendingSwitch {
            current,
            next,
            from: from.task.context,
            to: to.task.context,
            forced: false,
            from_state,
        });
        Ok(SliceDecision::Switch {
            from: from.task.context,
            to: to.task.context,
            forced: false,
        })
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
            from_state: PostSwitchState::Suspended,
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
        match pending.from_state {
            PostSwitchState::Ready => {
                // A priority preemption or yield pauses rather than faults the
                // previous task, which remains eligible to resume.
                current_slot.suspended = false;
                current_slot.cancelled = false;
                current_slot.ready = true;
            }
            PostSwitchState::Blocked => {
                current_slot.suspended = false;
                current_slot.cancelled = false;
                current_slot.ready = false;
            }
            PostSwitchState::Suspended => {
                current_slot.suspended = true;
                current_slot.cancelled = false;
                current_slot.ready = false;
                current_slot.forced_suspends = current_slot.forced_suspends.saturating_add(1);
            }
            PostSwitchState::Cancelled => {
                current_slot.suspended = false;
                current_slot.cancelled = true;
                current_slot.ready = false;
            }
        }
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

    /// Commit a completed PendSV switch and publish forced suspension for
    /// privileged module-scoped recovery.
    pub fn commit_pending_switch_and_publish_at(
        &mut self,
        now_us: u64,
        sentinel: &ExecutionSentinel,
        handoff: &ForcedSuspendHandoff,
    ) -> Result<SliceDecision, SliceError> {
        let decision = self.commit_pending_switch_at(now_us, sentinel)?;
        if let SliceDecision::Switch {
            from, forced: true, ..
        } = decision
        {
            handoff.publish(from.module);
        }
        Ok(decision)
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
        proactive_switches: u32,
        forced_switches: u32,
        fail: bool,
        isolation: bool,
    }

    impl SlicePort for Port {
        type Error = ();

        fn capabilities(&self) -> SlicePortCapabilities {
            if self.isolation {
                SlicePortCapabilities::pmsav7_m(8)
            } else {
                SlicePortCapabilities::privileged_only()
            }
        }

        fn pend_switch(
            &mut self,
            _from: SliceContext,
            _to: SliceContext,
            forced: bool,
        ) -> Result<(), Self::Error> {
            if self.fail {
                Err(())
            } else {
                self.switches += 1;
                if forced {
                    self.forced_switches += 1;
                } else {
                    self.proactive_switches += 1;
                }
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
    fn forced_suspend_handoff_retains_identity_count_and_conflict() {
        let handoff = ForcedSuspendHandoff::new();
        assert_eq!(handoff.drain(), Ok(None));
        handoff.publish(ModuleId::Sensor);
        handoff.publish(ModuleId::Sensor);
        assert_eq!(
            handoff.drain(),
            Ok(Some(ForcedSuspendReceipt {
                module: ModuleId::Sensor,
                occurrences: 2,
                identity_conflict: false,
                conflicting_module: None,
            }))
        );

        handoff.publish(ModuleId::Sensor);
        handoff.publish(ModuleId::Radio);
        assert_eq!(
            handoff.drain(),
            Ok(Some(ForcedSuspendReceipt {
                module: ModuleId::Sensor,
                occurrences: 2,
                identity_conflict: true,
                conflicting_module: Some(ModuleId::Radio),
            }))
        );
    }

    #[test]
    fn forced_suspend_count_saturates_without_losing_conflict_provenance() {
        let handoff = ForcedSuspendHandoff::new();
        let first = module_code(ModuleId::Sensor);
        let conflicting = module_code(ModuleId::Radio);
        handoff.state.store(
            first
                | (conflicting << FORCED_CONFLICT_MODULE_SHIFT)
                | (FORCED_COUNT_MASK << FORCED_COUNT_SHIFT)
                | FORCED_IDENTITY_CONFLICT,
            Ordering::Release,
        );
        handoff.publish(ModuleId::Actuator);
        assert_eq!(
            handoff.drain(),
            Ok(Some(ForcedSuspendReceipt {
                module: ModuleId::Sensor,
                occurrences: FORCED_COUNT_MASK,
                identity_conflict: true,
                conflicting_module: Some(ModuleId::Radio),
            }))
        );
    }

    #[test]
    fn interrupt_overflow_count_saturates_instead_of_wrapping() {
        let handoff = InterruptHandoff::new();
        handoff.events.store(1, Ordering::Release);
        handoff.overflows.store(u32::MAX, Ordering::Release);
        handoff.publish(0, 1);
        assert_eq!(handoff.drain().overflows, u32::MAX);
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
        assert_eq!(port.forced_switches, 1);
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
        let handoff = ForcedSuspendHandoff::new();
        let committed = controller
            .commit_pending_switch_and_publish_at(150, &sentinel, &handoff)
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
        assert_eq!(
            handoff.drain(),
            Ok(Some(ForcedSuspendReceipt {
                module: ModuleId::Actuator,
                occurrences: 1,
                identity_conflict: false,
                conflicting_module: None,
            }))
        );
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
            ..Port::default()
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
    fn higher_priority_ready_transition_requests_portable_preemption() {
        let mut storage = [0u64; 64];
        let base = storage.as_mut_ptr() as usize;
        let mut controller = SliceController::<2>::new();
        controller
            .add(SliceTask::new(
                ModuleId::Sensor,
                Criticality::User,
                100,
                base,
                256,
            ))
            .unwrap();
        controller
            .add(SliceTask::new(
                ModuleId::Actuator,
                Criticality::HardRealtime,
                50,
                base + 256,
                256,
            ))
            .unwrap();
        controller.mark_ready(ModuleId::Sensor);
        let sentinel = ExecutionSentinel::new();
        assert_eq!(
            controller.start_next_at(0, &sentinel).unwrap().module,
            ModuleId::Sensor
        );

        let mut port = Port::default();
        assert!(matches!(
            controller
                .mark_ready_and_request_preemption(ModuleId::Actuator, &mut port)
                .unwrap(),
            SliceDecision::Switch {
                from: SliceContext {
                    module: ModuleId::Sensor,
                    ..
                },
                to: SliceContext {
                    module: ModuleId::Actuator,
                    ..
                },
                forced: false,
            }
        ));
        assert_eq!(port.proactive_switches, 1);
        assert_eq!(port.forced_switches, 0);

        controller
            .commit_pending_switch_at(10, &sentinel)
            .expect("priority switch commits");
        assert_eq!(controller.current, Some(1));
        assert_eq!(controller.forced_suspends(ModuleId::Sensor), Some(0));
        assert_eq!(sentinel.check(60), None);
        assert_eq!(
            sentinel.check(61).map(|stuck| stuck.module_code),
            Some(module_code(ModuleId::Actuator))
        );
    }

    #[test]
    fn yield_block_wake_cancel_and_restart_have_distinct_bounded_states() {
        let mut storage = [0u64; 64];
        let base = storage.as_mut_ptr() as usize;
        let mut controller = SliceController::<2>::new();
        for (index, module) in [ModuleId::Sensor, ModuleId::Actuator]
            .into_iter()
            .enumerate()
        {
            controller
                .add(SliceTask::new(
                    module,
                    Criticality::User,
                    100,
                    base + index * 256,
                    256,
                ))
                .unwrap();
            assert!(controller.mark_ready(module));
        }
        let sentinel = ExecutionSentinel::new();
        assert_eq!(
            controller.start_next_at(0, &sentinel).unwrap().module,
            ModuleId::Sensor
        );
        let mut port = Port::default();

        assert!(matches!(
            controller.yield_current(&mut port),
            Ok(SliceDecision::Switch { forced: false, .. })
        ));
        controller.commit_pending_switch_at(10, &sentinel).unwrap();
        assert_eq!(controller.current, Some(1));

        assert!(matches!(
            controller.block_current(&mut port),
            Ok(SliceDecision::Switch { forced: false, .. })
        ));
        controller.commit_pending_switch_at(20, &sentinel).unwrap();
        assert_eq!(controller.current, Some(0));
        assert_eq!(controller.is_suspended(ModuleId::Actuator), Some(false));
        assert!(controller.mark_ready(ModuleId::Actuator));

        assert_eq!(
            controller.cancel(ModuleId::Actuator, &mut port),
            Ok(SliceDecision::None)
        );
        assert_eq!(controller.is_cancelled(ModuleId::Actuator), Some(true));
        assert!(!controller.mark_ready(ModuleId::Actuator));
        assert!(controller.restart(ModuleId::Actuator));

        assert!(matches!(
            controller.cancel(ModuleId::Sensor, &mut port),
            Ok(SliceDecision::Switch { forced: false, .. })
        ));
        assert_eq!(controller.is_cancelled(ModuleId::Sensor), Some(false));
        controller.commit_pending_switch_at(30, &sentinel).unwrap();
        assert_eq!(controller.is_cancelled(ModuleId::Sensor), Some(true));
        assert!(!controller.mark_ready(ModuleId::Sensor));
        assert!(controller.restart(ModuleId::Sensor));
        assert_eq!(port.proactive_switches, 3);
        assert_eq!(port.forced_switches, 0);
    }

    #[test]
    fn failed_cancel_switch_and_deadline_wrap_leave_state_retryable() {
        let mut storage = [0u64; 64];
        let base = storage.as_mut_ptr() as usize;
        let mut controller = SliceController::<2>::new();
        controller
            .add(SliceTask::new(
                ModuleId::Sensor,
                Criticality::User,
                100,
                base,
                256,
            ))
            .unwrap();
        controller
            .add(SliceTask::new(
                ModuleId::Actuator,
                Criticality::User,
                100,
                base + 256,
                256,
            ))
            .unwrap();
        controller.mark_ready(ModuleId::Sensor);
        controller.mark_ready(ModuleId::Actuator);
        let sentinel = ExecutionSentinel::new();
        assert_eq!(
            controller.start_next_at(u64::MAX - 50, &sentinel),
            Err(SliceError::DeadlineOverflow(ModuleId::Sensor))
        );
        controller.start_next_at(0, &sentinel).unwrap();

        let mut failing = Port {
            fail: true,
            ..Port::default()
        };
        assert_eq!(
            controller.cancel(ModuleId::Sensor, &mut failing),
            Err(SliceError::Port)
        );
        assert_eq!(controller.is_cancelled(ModuleId::Sensor), Some(false));
        assert_eq!(controller.current, Some(0));
        assert_eq!(
            sentinel.check(101).unwrap().module_code,
            module_code(ModuleId::Sensor)
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

    #[test]
    fn isolation_requirement_survives_selection_and_switching() {
        let mut storage = [0u64; 64];
        let base = storage.as_mut_ptr() as usize;
        let mut controller = SliceController::<2>::new();
        let mut port = Port {
            isolation: true,
            ..Port::default()
        };
        controller
            .add_for_port(
                SliceTask::new(ModuleId::Sensor, Criticality::System, 100, base, 256)
                    .unprivileged_mpu(),
                &port,
            )
            .unwrap();
        controller
            .add_for_port(
                SliceTask::new(ModuleId::Radio, Criticality::Driver, 100, base + 256, 256)
                    .unprivileged_mpu(),
                &port,
            )
            .unwrap();
        controller.mark_ready(ModuleId::Sensor);
        controller.mark_ready(ModuleId::Radio);
        let sentinel = ExecutionSentinel::new();
        let first = controller.start_next_at(0, &sentinel).unwrap();
        assert_eq!(first.protection, SliceProtection::UnprivilegedMpu);
        let decision = controller
            .on_budget_interrupt(101, &sentinel, &mut port)
            .unwrap();
        let SliceDecision::Switch { from, to, .. } = decision else {
            panic!("isolated task should switch");
        };
        assert_eq!(from.protection, SliceProtection::UnprivilegedMpu);
        assert_eq!(to.protection, SliceProtection::UnprivilegedMpu);
    }

    #[test]
    fn isolated_task_is_rejected_without_an_isolation_capable_port() {
        let mut storage = [0u64; 32];
        let base = storage.as_mut_ptr() as usize;
        let task = SliceTask::new(ModuleId::Sensor, Criticality::System, 100, base, 256)
            .unprivileged_mpu();
        let mut controller = SliceController::<1>::new();
        assert_eq!(
            controller.add(task),
            Err(SliceError::IsolationUnsupported(ModuleId::Sensor))
        );
        assert_eq!(
            controller.add_for_port(task, &Port::default()),
            Err(SliceError::IsolationUnsupported(ModuleId::Sensor))
        );
        let isolated = Port {
            isolation: true,
            ..Port::default()
        };
        assert!(controller.add_for_port(task, &isolated).is_ok());
    }

    #[test]
    fn committed_forced_suspend_routes_to_module_recovery_not_board_reset() {
        use crate::{
            kernel_module_spec, CapabilitySet, DeadlineContract, DependencySet, MemoryBudget,
            ModuleRunState, ModuleSpec, StartupNode, SystemManifest, SystemProfile,
        };

        type TestRuntime = Runtime<2, 2, 1, 0, 0, 2, 0>;
        let manifest = SystemManifest::<2>::from_specs(&[
            kernel_module_spec(
                MemoryBudget::new(16 * 1024, 4 * 1024, 1),
                DeadlineContract::new(20_000, 0),
            ),
            ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
                .requires(CapabilitySet::empty())
                .owns(CapabilitySet::empty())
                .memory(MemoryBudget::new(4 * 1024, 1024, 0)),
        ])
        .unwrap();
        let startup = [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty()),
            StartupNode::new(ModuleId::Sensor, DependencySet::empty().with_index(0)),
        ];
        let mut runtime = TestRuntime::admit(
            &manifest,
            &startup,
            SystemProfile::NRF52840_CORE,
            crate::FaultThresholds::DEFAULT,
        )
        .unwrap();
        runtime.boot_to_running(0).unwrap();

        let outcome = route_forced_suspend(
            &mut runtime,
            ForcedSuspendReceipt {
                module: ModuleId::Sensor,
                occurrences: 1,
                identity_conflict: false,
                conflicting_module: None,
            },
            101,
        )
        .unwrap();
        assert_eq!(outcome.module, ModuleId::Sensor);
        assert_eq!(outcome.action, Action::RebootModule);
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Recovering)
        );
        assert_eq!(
            runtime.module_state(ModuleId::Kernel),
            Some(ModuleRunState::Active)
        );

        assert_eq!(
            route_forced_suspend(
                &mut runtime,
                ForcedSuspendReceipt {
                    module: ModuleId::Sensor,
                    occurrences: 2,
                    identity_conflict: true,
                    conflicting_module: Some(ModuleId::Radio),
                },
                102,
            ),
            Err(ForcedSuspendRouteError::IdentityConflict {
                first_module: ModuleId::Sensor,
                conflicting_module: Some(ModuleId::Radio),
                occurrences: 2,
            })
        );
    }
}
