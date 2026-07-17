//! The authoritative execution loop (ARC-01 closure).
//!
//! `KernelExecutor` owns the assembled `Runtime` and a `TaskTable`; nothing else
//! can drive module execution while it runs. One `run_cycle` is one closed path:
//!
//! 1. watchdog edge sweep → recovery records
//! 2. due-alarm dispatch with recovery on backpressure
//! 3. task selection (highest criticality, then earliest phase-anchored release)
//! 4. module run-state check — suspended/recovering/disabled modules have their
//!    release skipped *and counted*, never silently polled
//! 5. the poll body runs inside a dispatcher-owned [`ModuleCtx`], so every
//!    protected operation is authorized, charged, and traced
//! 6. measured execution time feeds the CPU ledger and the task statistics;
//!    a budget overrun records a health fault (recovery thresholds escalate)
//!    and, under the bounded containment profile, disables the module after a
//!    configured number of overruns
//! 7. the outcome names the next due time so the idle decision (sleep/power)
//!    has an authoritative input
//!
//! Admission is fail-closed: the executor refuses to run until the registered
//! task set passes a response-time analysis (fixed priority by criticality,
//! pessimistic same-priority interference) plus the utilization bound — a set
//! whose declared worst-case costs cannot be scheduled never executes.
//!
//! A lock-free [`ExecutionSentinel`] marks the in-flight poll and its deadline
//! so an interrupt (deadline tick, hardware watchdog) can detect a non-yielding
//! module in real time — the bounded containment answer for cooperative
//! execution; preemption is intentionally out of scope for this profile.

use core::{
    cell::UnsafeCell,
    mem::{ManuallyDrop, MaybeUninit},
};

#[cfg(test)]
use core::cell::Cell;
use nobro_power::{ExecutorPower, PowerHookError, PowerMode, PowerPlatform};
use portable_atomic::{AtomicU32, AtomicU8, Ordering};

use crate::{
    module_code, AdmissionPlan, ExecutorInstrumentation, FaultThresholds, KernelError, ModuleCtx,
    ModuleId, ModuleRunState, Poll, Runtime, RuntimeError, StackFault, StackGuardTable,
    StartupNode, SystemManifest, SystemProfile, TaskMeta, TaskTable, TaskTableError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainmentPolicy {
    /// Smallest profile: overruns are measured, recorded, and escalate through
    /// health thresholds only.
    Cooperative,
    /// Bounded profile for blocking/untrusted work: after `disable_after_overruns`
    /// measured budget overruns the module is disabled and cleaned up.
    Bounded { disable_after_overruns: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecError {
    Task(TaskTableError),
    /// The task set failed response-time or utilization admission.
    Unschedulable {
        module: ModuleId,
        response_us: u64,
        period_us: u32,
    },
    /// `run_cycle` was called before the task set was sealed.
    NotSealed,
    /// A task was registered after sealing.
    Sealed,
    Runtime(RuntimeError),
    Power(PowerHookError),
    PowerLedgerFull,
    /// Scheduler membership and task-slot state disagreed.
    TaskStateCorrupt,
}

/// Failure to claim or assemble a [`KernelExecutorCell`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutorInitError {
    /// The one-shot cell is being initialized or already contains an executor.
    AlreadyInitialized,
    Runtime(RuntimeError),
}

impl From<RuntimeError> for ExecutorInitError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<TaskTableError> for ExecError {
    fn from(error: TaskTableError) -> Self {
        Self::Task(error)
    }
}

impl From<RuntimeError> for ExecError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<PowerHookError> for ExecError {
    fn from(error: PowerHookError) -> Self {
        Self::Power(error)
    }
}

/// Lock-free marker of the in-flight poll: an ISR calls [`check`](Self::check)
/// to detect a module running past its declared budget while the cooperative
/// loop cannot regain control.
pub struct ExecutionSentinel {
    /// Even = stable generation, odd = writer in progress. Readers never spin:
    /// an ISR that preempts an update simply reports no stable sample.
    sequence: AtomicU32,
    /// `module_code` of the polling module; 0 = idle.
    module: AtomicU32,
    /// Absolute time the in-flight poll must have yielded by, split into two
    /// halves so Cortex-M targets without native 64-bit atomics stay lock-free.
    /// The halves only change while `module == 0`, and `module` is the
    /// release/acquire gate, so a reader that sees a nonzero module sees a
    /// consistent deadline.
    deadline_lo: AtomicU32,
    deadline_hi: AtomicU32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StuckPoll {
    pub module_code: u32,
    pub deadline_us: u64,
    pub late_us: u64,
}

impl ExecutionSentinel {
    pub const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            module: AtomicU32::new(0),
            deadline_lo: AtomicU32::new(0),
            deadline_hi: AtomicU32::new(0),
        }
    }

    pub(crate) fn arm(&self, module: ModuleId, deadline_us: u64) {
        // The sentinel is sampled asynchronously and its fields are separate
        // atomics. A single sequentially-consistent order is intentional here:
        // it makes the odd/even generation a real publication bracket instead
        // of permitting a new module to be paired with an older deadline.
        self.sequence.fetch_add(1, Ordering::SeqCst);
        self.deadline_lo.store(deadline_us as u32, Ordering::SeqCst);
        self.deadline_hi
            .store((deadline_us >> 32) as u32, Ordering::SeqCst);
        self.module.store(module_code(module), Ordering::SeqCst);
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn disarm(&self) {
        self.sequence.fetch_add(1, Ordering::SeqCst);
        self.module.store(0, Ordering::SeqCst);
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }

    /// ISR-safe: returns the in-flight poll iff it has outlived its budget.
    pub fn check(&self, now_us: u64) -> Option<StuckPoll> {
        let before = self.sequence.load(Ordering::SeqCst);
        if before & 1 != 0 {
            return None;
        }
        let module_code = self.module.load(Ordering::SeqCst);
        let deadline_us = u64::from(self.deadline_lo.load(Ordering::SeqCst))
            | (u64::from(self.deadline_hi.load(Ordering::SeqCst)) << 32);
        let after = self.sequence.load(Ordering::SeqCst);
        if before != after || after & 1 != 0 || module_code == 0 {
            return None;
        }
        if now_us <= deadline_us {
            return None;
        }
        Some(StuckPoll {
            module_code,
            deadline_us,
            late_us: now_us - deadline_us,
        })
    }
}

struct SentinelArmGuard<'a> {
    sentinel: &'a ExecutionSentinel,
}

impl<'a> SentinelArmGuard<'a> {
    fn new(sentinel: &'a ExecutionSentinel, module: ModuleId, deadline_us: u64) -> Self {
        sentinel.arm(module, deadline_us);
        Self { sentinel }
    }
}

impl Drop for SentinelArmGuard<'_> {
    fn drop(&mut self) {
        self.sentinel.disarm();
    }
}

impl Default for ExecutionSentinel {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CycleOutcome {
    pub polled: Option<ModuleId>,
    pub poll: Option<Poll>,
    pub duration_us: u32,
    pub isr_releases: u32,
    pub rejected_isr_releases: u32,
    /// Async timer slots fired at cycle entry after a hardware/polled compare.
    pub async_timer_wakes: usize,
    pub observed_wake_latency_us: u32,
    pub overrun: bool,
    /// The bounded containment profile disabled the module this cycle.
    pub contained: bool,
    pub skipped_release: Option<ModuleId>,
    pub alarms_dispatched: usize,
    pub watchdog_recoveries: usize,
    /// Nothing runnable before this time — the authoritative idle/sleep input.
    pub idle_until_us: Option<u64>,
    pub power_mode: Option<PowerMode>,
}

#[repr(C)]
pub struct KernelExecutor<
    const TASKS: usize,
    const STARTUP: usize,
    const QUOTAS: usize,
    const MAILBOX: usize,
    const ALARMS: usize,
    const KV: usize,
    const HEALTH: usize,
    const LOG: usize,
> {
    runtime: Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
    tasks: TaskTable<TASKS>,
    containment: ContainmentPolicy,
    sentinel: ExecutionSentinel,
    power: ExecutorPower<TASKS>,
    wake_latency_us: u32,
    sealed: bool,
}

const CELL_EMPTY: u8 = 0;
const CELL_INITIALIZING: u8 = 1;
const CELL_READY: u8 = 2;

/// One-shot static storage for constructing an admitted executor in place.
///
/// This removes duplicate by-value `Runtime`/`KernelExecutor` and graph-scratch
/// temporaries from the entry stack. It does **not** make them free: the larger
/// of the final executor and its disjoint admission/graph workspace resides in
/// `.bss`, and the complete [`storage_bytes`](Self::storage_bytes) value must
/// be included in total-RAM accounting alongside every measured stack peak.
///
/// A cell never exposes a partial value.  Initialization is claimed with an
/// atomic state transition; failures and unwinding panics restore the empty
/// state after dropping only completed fields.  Successful cells are one-shot
/// for the firmware lifetime, matching normal static executor ownership.
pub struct KernelExecutorCell<
    const TASKS: usize,
    const STARTUP: usize,
    const QUOTAS: usize,
    const MAILBOX: usize,
    const ALARMS: usize,
    const KV: usize,
    const HEALTH: usize,
    const LOG: usize,
> {
    state: AtomicU8,
    value: UnsafeCell<
        MaybeUninit<
            KernelExecutorStorage<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        >,
    >,
}

#[repr(C)]
struct ExecutorGraphScratch<const MODULES: usize> {
    manifest: MaybeUninit<SystemManifest<MODULES>>,
    startup: MaybeUninit<[StartupNode; MODULES]>,
}

#[repr(C)]
struct ExecutorGraphWorkspace<const STARTUP: usize, const QUOTAS: usize> {
    admission: MaybeUninit<AdmissionPlan<STARTUP, QUOTAS>>,
    scratch: ExecutorGraphScratch<STARTUP>,
}

union KernelExecutorStorage<
    const TASKS: usize,
    const STARTUP: usize,
    const QUOTAS: usize,
    const MAILBOX: usize,
    const ALARMS: usize,
    const KV: usize,
    const HEALTH: usize,
    const LOG: usize,
> {
    executor:
        ManuallyDrop<KernelExecutor<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>>,
    graph: ManuallyDrop<ExecutorGraphWorkspace<STARTUP, QUOTAS>>,
}

pub(crate) enum ExecutorGraphInitError<E> {
    Graph(E),
    Executor(ExecutorInitError),
}

// The atomic state grants exactly one initializer exclusive access to the
// UnsafeCell.  The only successful borrow is unique and one-shot; racing calls
// receive `AlreadyInitialized` and can never observe the storage.
unsafe impl<
        const TASKS: usize,
        const STARTUP: usize,
        const QUOTAS: usize,
        const MAILBOX: usize,
        const ALARMS: usize,
        const KV: usize,
        const HEALTH: usize,
        const LOG: usize,
    > Sync for KernelExecutorCell<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>
{
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExecutorInitStage {
    Runtime,
    Tasks,
    Containment,
    Sentinel,
    Power,
    Sealed,
}

#[cfg(test)]
impl ExecutorInitStage {
    const ALL: [Self; 6] = [
        Self::Runtime,
        Self::Tasks,
        Self::Containment,
        Self::Sentinel,
        Self::Power,
        Self::Sealed,
    ];
}

struct ExecutorInitGuard<
    'a,
    const TASKS: usize,
    const STARTUP: usize,
    const QUOTAS: usize,
    const MAILBOX: usize,
    const ALARMS: usize,
    const KV: usize,
    const HEALTH: usize,
    const LOG: usize,
> {
    cell: &'a KernelExecutorCell<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
    initialized: u8,
    armed: bool,
    #[cfg(test)]
    cleanup_mask: Option<*const Cell<u8>>,
}

impl<
        'a,
        const TASKS: usize,
        const STARTUP: usize,
        const QUOTAS: usize,
        const MAILBOX: usize,
        const ALARMS: usize,
        const KV: usize,
        const HEALTH: usize,
        const LOG: usize,
    > ExecutorInitGuard<'a, TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>
{
    fn new(
        cell: &'a KernelExecutorCell<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        #[cfg(test)] cleanup_mask: Option<&Cell<u8>>,
    ) -> Self {
        Self {
            cell,
            initialized: 0,
            armed: true,
            #[cfg(test)]
            cleanup_mask: cleanup_mask.map(core::ptr::from_ref),
        }
    }

    fn mark(&mut self, stage: ExecutorInitStage) {
        self.initialized |= 1 << stage as u8;
    }

    fn has(&self, stage: ExecutorInitStage) -> bool {
        self.initialized & (1 << stage as u8) != 0
    }

    fn finish(mut self) {
        self.cell.state.store(CELL_READY, Ordering::Release);
        self.armed = false;
    }
}

impl<
        const TASKS: usize,
        const STARTUP: usize,
        const QUOTAS: usize,
        const MAILBOX: usize,
        const ALARMS: usize,
        const KV: usize,
        const HEALTH: usize,
        const LOG: usize,
    > Drop for ExecutorInitGuard<'_, TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>
{
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        #[cfg(test)]
        if let Some(cleanup_mask) = self.cleanup_mask {
            // SAFETY: the test observer outlives this constructor call.
            unsafe { (*cleanup_mask).set(self.initialized) };
        }

        unsafe {
            let destination = self.cell.destination();
            if self.has(ExecutorInitStage::Sealed) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).sealed));
            }
            if self.has(ExecutorInitStage::Power) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).power));
            }
            if self.has(ExecutorInitStage::Sentinel) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).sentinel));
            }
            if self.has(ExecutorInitStage::Containment) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).containment));
            }
            if self.has(ExecutorInitStage::Tasks) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).tasks));
            }
            if self.has(ExecutorInitStage::Runtime) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).runtime));
            }
        }
        self.cell.state.store(CELL_EMPTY, Ordering::Release);
    }
}

impl<
        const TASKS: usize,
        const STARTUP: usize,
        const QUOTAS: usize,
        const MAILBOX: usize,
        const ALARMS: usize,
        const KV: usize,
        const HEALTH: usize,
        const LOG: usize,
    > KernelExecutorCell<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>
{
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(CELL_EMPTY),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Bytes reserved by the cell, including executor storage and state/alignment.
    pub const fn storage_bytes() -> usize {
        core::mem::size_of::<Self>()
    }

    fn destination(
        &self,
    ) -> *mut KernelExecutor<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG> {
        type Storage<
            const T: usize,
            const S: usize,
            const Q: usize,
            const M: usize,
            const A: usize,
            const K: usize,
            const H: usize,
            const L: usize,
        > = KernelExecutorStorage<T, S, Q, M, A, K, H, L>;
        let storage =
            self.value
                .get()
                .cast::<Storage<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>>();
        unsafe {
            core::ptr::addr_of_mut!((*storage).executor).cast::<KernelExecutor<
                TASKS,
                STARTUP,
                QUOTAS,
                MAILBOX,
                ALARMS,
                KV,
                HEALTH,
                LOG,
            >>()
        }
    }

    unsafe fn graph_scratch_destination(&self) -> *mut ExecutorGraphScratch<STARTUP> {
        type Storage<
            const T: usize,
            const S: usize,
            const Q: usize,
            const M: usize,
            const A: usize,
            const K: usize,
            const H: usize,
            const L: usize,
        > = KernelExecutorStorage<T, S, Q, M, A, K, H, L>;
        let destination = self.destination();
        let storage =
            self.value
                .get()
                .cast::<Storage<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>>();
        let workspace = core::ptr::addr_of_mut!((*storage).graph)
            .cast::<ExecutorGraphWorkspace<STARTUP, QUOTAS>>();
        let runtime = core::ptr::addr_of_mut!((*destination).runtime);
        let (admission, admission_size) = Runtime::admission_storage_range(runtime);
        debug_assert_eq!(
            admission,
            core::ptr::addr_of_mut!((*workspace).admission).cast()
        );
        debug_assert_eq!(
            admission_size,
            core::mem::size_of::<AdmissionPlan<STARTUP, QUOTAS>>()
        );
        core::ptr::addr_of_mut!((*workspace).scratch)
    }

    /// Admit and construct one executor directly in this static cell.
    #[allow(
        clippy::mut_from_ref,
        reason = "the atomic one-shot claim proves this UnsafeCell can yield exactly one mutable borrow"
    )]
    pub fn init_admitted<const MODULES: usize>(
        &'static self,
        manifest: &SystemManifest<MODULES>,
        startup_nodes: &[StartupNode],
        profile: SystemProfile,
        thresholds: FaultThresholds,
        containment: ContainmentPolicy,
    ) -> Result<
        &'static mut KernelExecutor<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        ExecutorInitError,
    > {
        let mut checkpoint = |_stage| Ok(());
        #[cfg(not(test))]
        {
            self.init_admitted_inner(
                manifest,
                startup_nodes,
                profile,
                thresholds,
                containment,
                &mut checkpoint,
            )
        }
        #[cfg(test)]
        {
            self.init_admitted_inner(
                manifest,
                startup_nodes,
                profile,
                thresholds,
                containment,
                &mut checkpoint,
                None,
            )
        }
    }

    /// Claim the cell, derive a prevalidated graph in storage that will later
    /// belong to the executor, and overwrite that scratch only after admission
    /// has consumed it.
    ///
    /// The cell's union workspace keeps the admission destination and graph
    /// scratch disjoint. A graph error restores the empty cell state.
    ///
    /// # Safety
    ///
    /// On `Ok(startup_len)`, `build` must have made `manifest` pass
    /// `validate_profile(profile)`, initialized `startup[..startup_len]`, and
    /// returned a length no greater than `STARTUP`.
    #[allow(
        clippy::mut_from_ref,
        reason = "the atomic one-shot claim proves this UnsafeCell can yield exactly one mutable borrow"
    )]
    pub(crate) unsafe fn init_prevalidated_with_graph_scratch<E>(
        &'static self,
        profile: SystemProfile,
        thresholds: FaultThresholds,
        containment: ContainmentPolicy,
        build: impl FnOnce(
            &mut SystemManifest<STARTUP>,
            &mut [StartupNode; STARTUP],
        ) -> Result<usize, E>,
    ) -> Result<
        &'static mut KernelExecutor<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        ExecutorGraphInitError<E>,
    > {
        let scratch = self.graph_scratch_destination();
        self.state
            .compare_exchange(
                CELL_EMPTY,
                CELL_INITIALIZING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| ExecutorGraphInitError::Executor(ExecutorInitError::AlreadyInitialized))?;

        let destination = self.destination();
        let mut guard = ExecutorInitGuard::new(
            self,
            #[cfg(test)]
            None,
        );
        let manifest =
            core::ptr::addr_of_mut!((*scratch).manifest).cast::<SystemManifest<STARTUP>>();
        SystemManifest::init_in_place(manifest);
        let startup = core::ptr::addr_of_mut!((*scratch).startup).cast::<StartupNode>();
        for index in 0..STARTUP {
            startup.add(index).write(StartupNode::EMPTY);
        }
        let mut checkpoint = |_stage| Ok(());
        let runtime = core::ptr::addr_of_mut!((*destination).runtime);
        let module_count = {
            let manifest = &mut *manifest;
            let startup = &mut *startup.cast::<[StartupNode; STARTUP]>();
            let startup_len = build(manifest, startup).map_err(ExecutorGraphInitError::Graph)?;
            debug_assert!(startup_len <= STARTUP);
            Runtime::admit_prevalidated_plan_in_place(
                runtime,
                manifest,
                &startup[..startup_len],
                profile,
            )
            .map_err(ExecutorInitError::from)
            .map_err(ExecutorGraphInitError::Executor)?
        };
        Runtime::finish_graph_plan_in_place(runtime, module_count, thresholds)
            .map_err(ExecutorInitError::from)
            .map_err(ExecutorGraphInitError::Executor)?;
        guard.mark(ExecutorInitStage::Runtime);
        checkpoint(ExecutorInitStage::Runtime).map_err(ExecutorGraphInitError::Executor)?;
        self.populate_after_runtime(
            destination,
            profile,
            containment,
            &mut guard,
            &mut checkpoint,
        )
        .map_err(ExecutorGraphInitError::Executor)?;

        guard.finish();
        Ok(&mut *destination)
    }

    #[allow(
        clippy::mut_from_ref,
        reason = "testable inner half of the same atomic one-shot UnsafeCell protocol"
    )]
    #[allow(
        clippy::too_many_arguments,
        reason = "the test-only checkpoint and cleanup observer keep failure coverage off the production API"
    )]
    fn init_admitted_inner<const MODULES: usize>(
        &'static self,
        manifest: &SystemManifest<MODULES>,
        startup_nodes: &[StartupNode],
        profile: SystemProfile,
        thresholds: FaultThresholds,
        containment: ContainmentPolicy,
        checkpoint: &mut impl FnMut(ExecutorInitStage) -> Result<(), ExecutorInitError>,
        #[cfg(test)] cleanup_mask: Option<&Cell<u8>>,
    ) -> Result<
        &'static mut KernelExecutor<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        ExecutorInitError,
    > {
        self.state
            .compare_exchange(
                CELL_EMPTY,
                CELL_INITIALIZING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| ExecutorInitError::AlreadyInitialized)?;

        let destination = self.destination();
        let mut guard = ExecutorInitGuard::new(
            self,
            #[cfg(test)]
            cleanup_mask,
        );
        unsafe {
            self.populate_claimed(
                destination,
                manifest,
                startup_nodes,
                profile,
                thresholds,
                containment,
                &mut guard,
                checkpoint,
            )?;
        }

        guard.finish();
        // SAFETY: every field is initialized, the release store published the
        // ready state, and this one-shot cell can never yield another borrow.
        Ok(unsafe { &mut *destination })
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the shared claimed-cell constructor keeps graph scratch and ordinary admission on one initialization path"
    )]
    unsafe fn populate_claimed<const MODULES: usize>(
        &'static self,
        destination: *mut KernelExecutor<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        manifest: &SystemManifest<MODULES>,
        startup_nodes: &[StartupNode],
        profile: SystemProfile,
        thresholds: FaultThresholds,
        containment: ContainmentPolicy,
        guard: &mut ExecutorInitGuard<
            'static,
            TASKS,
            STARTUP,
            QUOTAS,
            MAILBOX,
            ALARMS,
            KV,
            HEALTH,
            LOG,
        >,
        checkpoint: &mut impl FnMut(ExecutorInitStage) -> Result<(), ExecutorInitError>,
    ) -> Result<(), ExecutorInitError> {
        unsafe {
            Runtime::admit_in_place(
                core::ptr::addr_of_mut!((*destination).runtime),
                manifest,
                startup_nodes,
                profile,
                thresholds,
            )?;
            guard.mark(ExecutorInitStage::Runtime);
            checkpoint(ExecutorInitStage::Runtime)?;
        }
        self.populate_after_runtime(destination, profile, containment, guard, checkpoint)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the shared post-runtime constructor preserves one cleanup checkpoint sequence"
    )]
    unsafe fn populate_after_runtime(
        &'static self,
        destination: *mut KernelExecutor<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        profile: SystemProfile,
        containment: ContainmentPolicy,
        guard: &mut ExecutorInitGuard<
            'static,
            TASKS,
            STARTUP,
            QUOTAS,
            MAILBOX,
            ALARMS,
            KV,
            HEALTH,
            LOG,
        >,
        checkpoint: &mut impl FnMut(ExecutorInitStage) -> Result<(), ExecutorInitError>,
    ) -> Result<(), ExecutorInitError> {
        TaskTable::init_in_place(core::ptr::addr_of_mut!((*destination).tasks));
        guard.mark(ExecutorInitStage::Tasks);
        checkpoint(ExecutorInitStage::Tasks)?;

        core::ptr::addr_of_mut!((*destination).containment).write(containment);
        guard.mark(ExecutorInitStage::Containment);
        checkpoint(ExecutorInitStage::Containment)?;

        core::ptr::addr_of_mut!((*destination).sentinel).write(ExecutionSentinel::new());
        guard.mark(ExecutorInitStage::Sentinel);
        checkpoint(ExecutorInitStage::Sentinel)?;

        ExecutorPower::init_in_place(
            core::ptr::addr_of_mut!((*destination).power),
            1_000_000,
            1_000_000,
            1_000,
        );
        guard.mark(ExecutorInitStage::Power);
        checkpoint(ExecutorInitStage::Power)?;

        core::ptr::addr_of_mut!((*destination).wake_latency_us).write(profile.wake_latency_us);
        core::ptr::addr_of_mut!((*destination).sealed).write(false);
        guard.mark(ExecutorInitStage::Sealed);
        checkpoint(ExecutorInitStage::Sealed)?;
        Ok(())
    }
}

impl<
        const TASKS: usize,
        const STARTUP: usize,
        const QUOTAS: usize,
        const MAILBOX: usize,
        const ALARMS: usize,
        const KV: usize,
        const HEALTH: usize,
        const LOG: usize,
    > Default for KernelExecutorCell<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<
        const TASKS: usize,
        const STARTUP: usize,
        const QUOTAS: usize,
        const MAILBOX: usize,
        const ALARMS: usize,
        const KV: usize,
        const HEALTH: usize,
        const LOG: usize,
    > KernelExecutor<TASKS, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>
{
    /// Take ownership of the runtime: from here on, execution has one driver.
    pub fn new(
        runtime: Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        containment: ContainmentPolicy,
    ) -> Self {
        Self {
            runtime,
            tasks: TaskTable::new(),
            containment,
            sentinel: ExecutionSentinel::new(),
            power: ExecutorPower::new(1_000_000, 1_000_000, 1_000),
            wake_latency_us: 0,
            sealed: false,
        }
    }

    pub fn add_task(&mut self, meta: TaskMeta, now_us: u64) -> Result<(), ExecError> {
        if self.sealed {
            return Err(ExecError::Sealed);
        }
        self.runtime.ensure_module_enabled(meta.module)?;
        self.tasks.add(meta, now_us)?;
        Ok(())
    }

    pub(crate) fn rebase_unstarted_task_epoch(&mut self, now_us: u64) -> Result<(), ExecError> {
        if self.tasks.rebase_unstarted_epoch(now_us) {
            Ok(())
        } else {
            Err(ExecError::TaskStateCorrupt)
        }
    }

    /// Fail-closed schedulability admission: response-time analysis over the
    /// registered set (fixed priority = criticality, same-priority interference
    /// counted pessimistically) plus the utilization bound. Explicit phases
    /// shape releases but do not reduce this conservative interference proof.
    /// `run_cycle` refuses to execute until this has passed.
    pub fn seal(&mut self) -> Result<(), ExecError> {
        let metas = self.tasks.metas();
        for meta in metas.iter().flatten() {
            let response_us = response_time(*meta, &metas, self.wake_latency_us)?;
            if response_us > u64::from(meta.deadline_us) {
                return Err(ExecError::Unschedulable {
                    module: meta.module,
                    response_us,
                    period_us: meta.deadline_us,
                });
            }
        }
        self.sealed = true;
        Ok(())
    }

    /// Set the measured compare-wake-to-dispatch bound before admission.
    pub fn set_wake_latency_us(&mut self, wake_latency_us: u32) -> Result<(), ExecError> {
        if self.sealed {
            return Err(ExecError::Sealed);
        }
        self.wake_latency_us = wake_latency_us;
        Ok(())
    }

    pub const fn wake_latency_us(&self) -> u32 {
        self.wake_latency_us
    }

    pub const fn sentinel(&self) -> &ExecutionSentinel {
        &self.sentinel
    }

    pub const fn runtime(&self) -> &Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG> {
        &self.runtime
    }

    pub const fn tasks(&self) -> &TaskTable<TASKS> {
        &self.tasks
    }

    /// Transfer a bounded compare-ISR handoff into the executor's ready queues.
    pub fn accept_isr_releases(
        &mut self,
        ready_mask: u32,
        now_us: u64,
    ) -> crate::IsrReleaseReceipt {
        self.tasks.accept_isr_releases(ready_mask, now_us)
    }

    pub const fn power(&self) -> &ExecutorPower<TASKS> {
        &self.power
    }

    pub fn set_task_power(&mut self, module: ModuleId, power_uw: u64) -> bool {
        self.power
            .set_task_power(module_code(module) as u16, power_uw)
    }

    pub fn suspend_module(
        &mut self,
        module: ModuleId,
        now_us: u64,
        platform: &mut impl PowerPlatform,
    ) -> Result<(), ExecError> {
        platform.suspend(module_code(module) as u16)?;
        self.runtime.suspend_module(module, now_us)?;
        Ok(())
    }

    pub fn resume_module(
        &mut self,
        module: ModuleId,
        now_us: u64,
        platform: &mut impl PowerPlatform,
    ) -> Result<(), ExecError> {
        platform.resume(module_code(module) as u16)?;
        self.runtime.resume_module(module, now_us)?;
        Ok(())
    }

    /// Trusted-dispatcher escape hatch for setup that must happen between
    /// cycles (never hand this to module code).
    pub fn runtime_mut(
        &mut self,
    ) -> &mut Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG> {
        &mut self.runtime
    }

    /// Sweep the registered stack guards and route the first broken canary
    /// into recovery as a `StackViolation` attributed to the owning module
    /// (MEM-01). Call once per cycle or from a maintenance tick.
    pub fn enforce_stack_guards<const G: usize>(
        &mut self,
        guards: &StackGuardTable<G>,
        now_us: u64,
    ) -> Result<Option<StackFault>, ExecError> {
        let Some(fault) = guards.sweep() else {
            return Ok(None);
        };
        if self.runtime.module_state(fault.module) == Some(ModuleRunState::Active) {
            let _ = self
                .runtime
                .record_error(fault.module, KernelError::StackViolation, now_us)?;
        }
        Ok(Some(fault))
    }

    /// One bounded cycle of the authoritative loop. `clock` supplies monotonic
    /// microseconds; `dispatch` is the application's module body, receiving a
    /// context fixed to the selected module's identity.
    pub fn run_cycle(
        &mut self,
        clock: impl Fn() -> u64,
        power_platform: &mut impl PowerPlatform,
        dispatch: impl FnMut(
            &mut ModuleCtx<'_, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        ) -> Result<Poll, KernelError>,
    ) -> Result<CycleOutcome, ExecError> {
        self.run_cycle_inner::<false, false, 1, 0>(clock, power_platform, dispatch, None, None)
    }

    /// Run one cycle with an admitted reactor's timer queue included in the
    /// authoritative compare/idle decision.
    ///
    /// A due compare advances the queue at cycle entry and event-wakes the
    /// named reactor task without consuming or shifting that task's periodic
    /// release. The queue's next deadline is merged with tasks and alarms
    /// before the power provider is armed, so the CPU may sleep until the
    /// earliest async deadline instead of polling the reactor.
    pub fn run_cycle_with_reactor_deadlines<const TIMERS: usize>(
        &mut self,
        clock: impl Fn() -> u64,
        power_platform: &mut impl PowerPlatform,
        reactor_module: ModuleId,
        timers: &crate::TimerQueue<TIMERS>,
        dispatch: impl FnMut(
            &mut ModuleCtx<'_, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        ) -> Result<Poll, KernelError>,
    ) -> Result<CycleOutcome, ExecError> {
        self.run_cycle_inner::<false, true, 1, TIMERS>(
            clock,
            power_platform,
            dispatch,
            None,
            Some((reactor_module, timers)),
        )
    }

    /// Opt-in attribution path. The caller owns the bounded recorder; the
    /// ordinary [`run_cycle`](Self::run_cycle) specialization has no recorder
    /// storage and performs none of the attribution-only clock reads.
    pub fn run_cycle_instrumented<const GROUPS: usize>(
        &mut self,
        clock: impl Fn() -> u64,
        power_platform: &mut impl PowerPlatform,
        dispatch: impl FnMut(
            &mut ModuleCtx<'_, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        ) -> Result<Poll, KernelError>,
        instrumentation: &mut ExecutorInstrumentation<GROUPS>,
    ) -> Result<CycleOutcome, ExecError> {
        self.run_cycle_inner::<true, false, GROUPS, 0>(
            clock,
            power_platform,
            dispatch,
            Some(instrumentation),
            None,
        )
    }

    /// Instrumented form of [`run_cycle_with_reactor_deadlines`](Self::run_cycle_with_reactor_deadlines).
    pub fn run_cycle_with_reactor_deadlines_instrumented<
        const TIMERS: usize,
        const GROUPS: usize,
    >(
        &mut self,
        clock: impl Fn() -> u64,
        power_platform: &mut impl PowerPlatform,
        reactor_module: ModuleId,
        timers: &crate::TimerQueue<TIMERS>,
        dispatch: impl FnMut(
            &mut ModuleCtx<'_, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        ) -> Result<Poll, KernelError>,
        instrumentation: &mut ExecutorInstrumentation<GROUPS>,
    ) -> Result<CycleOutcome, ExecError> {
        self.run_cycle_inner::<true, true, GROUPS, TIMERS>(
            clock,
            power_platform,
            dispatch,
            Some(instrumentation),
            Some((reactor_module, timers)),
        )
    }

    #[inline]
    fn run_cycle_inner<
        const INSTRUMENTED: bool,
        const ASYNC_DEADLINES: bool,
        const GROUPS: usize,
        const TIMERS: usize,
    >(
        &mut self,
        clock: impl Fn() -> u64,
        power_platform: &mut impl PowerPlatform,
        mut dispatch: impl FnMut(
            &mut ModuleCtx<'_, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        ) -> Result<Poll, KernelError>,
        mut instrumentation: Option<&mut ExecutorInstrumentation<GROUPS>>,
        async_deadlines: Option<(ModuleId, &crate::TimerQueue<TIMERS>)>,
    ) -> Result<CycleOutcome, ExecError> {
        if !self.sealed {
            return Err(ExecError::NotSealed);
        }
        let now_us = clock();
        let mut outcome = CycleOutcome::default();
        let isr = self.accept_isr_releases(power_platform.take_deadline_releases(now_us), now_us);
        outcome.isr_releases = isr.accepted;
        outcome.rejected_isr_releases = isr.rejected;
        outcome.observed_wake_latency_us = power_platform.observed_wake_latency_us();
        if ASYNC_DEADLINES {
            let Some((reactor_module, timers)) = async_deadlines else {
                return Err(ExecError::TaskStateCorrupt);
            };
            outcome.async_timer_wakes = timers.advance(now_us);
            if outcome.async_timer_wakes != 0 {
                let _ = self.tasks.wake_event(reactor_module)?;
            }
        }

        let sweep = self.runtime.sweep_watchdogs(now_us)?;
        outcome.watchdog_recoveries = sweep.len;

        let alarms = self.runtime.dispatch_due_alarms_with_recovery(now_us)?;
        outcome.alarms_dispatched = alarms.dispatched;

        let ordinary_selection = if INSTRUMENTED {
            None
        } else {
            self.tasks.select_due(now_us)
        };
        let mut selected = ordinary_selection.map(|selection| selection.index);
        let mut release_us = ordinary_selection.map_or(0, |selection| selection.release_us);
        let mut simultaneous_width = ordinary_selection.map_or(0, |selection| {
            self.tasks.selected_group_width(selection.index)
        });
        let mut selection_sweep_slots = 0u32;
        let mut selection_due_tasks = 0u32;
        let mut peer_scan_slots = 0u32;
        let mut scheduling_now_us = now_us;
        let mut instrumented_start_us = None;
        let mut instrumented_meta = None;
        let mut instrumented_active = false;
        let mut instrumentation_clock_invalid = false;
        let mut selection_reevaluated = false;
        let mut selection_started_us = 0;
        let mut selection_finished_us = 0;
        let mut probe_clock_reads = 0u32;
        if INSTRUMENTED {
            selection_started_us = clock();
            selection_finished_us = selection_started_us;
            let mut snapshot_us = selection_started_us;
            probe_clock_reads = 1;
            instrumentation_clock_invalid = selection_started_us < now_us;
            let mut instrumentation_stable = !instrumentation_clock_invalid;
            if instrumentation_stable {
                instrumentation_stable = false;
                // Each failed stabilization can only be caused by time moving
                // across another release. Keep the diagnostic bounded even for
                // a pathological clock/task set; fail closed instead of using
                // a stale choice if no stable snapshot is found.
                for _ in 0..TASKS.saturating_add(2) {
                    let sweep = self.tasks.due_sweep(snapshot_us);
                    selected = sweep.selected.map(|selection| selection.index);
                    release_us = sweep.selected.map_or(0, |selection| selection.release_us);
                    simultaneous_width = sweep.simultaneous_width;
                    let next_release_us = sweep.next_release_us;
                    selection_sweep_slots =
                        selection_sweep_slots.saturating_add(sweep.inspected_slots);
                    selection_due_tasks = sweep.due_tasks;
                    peer_scan_slots = peer_scan_slots.saturating_add(sweep.peer_inspected_slots);

                    let sweep_finished_us = clock();
                    probe_clock_reads = probe_clock_reads.saturating_add(1);
                    selection_finished_us = sweep_finished_us;
                    if sweep_finished_us < snapshot_us {
                        instrumentation_clock_invalid = true;
                        break;
                    }
                    if next_release_us.is_some_and(|release| release <= sweep_finished_us) {
                        selection_reevaluated = true;
                        snapshot_us = sweep_finished_us;
                        continue;
                    }
                    scheduling_now_us = sweep_finished_us;
                    if selected.is_none() {
                        instrumentation_stable = true;
                        break;
                    }

                    let Some(candidate_index) = selected else {
                        return Err(ExecError::TaskStateCorrupt);
                    };
                    let Some(candidate_meta) = self.tasks.meta_at(candidate_index) else {
                        return Err(ExecError::TaskStateCorrupt);
                    };
                    let candidate_active = self.runtime.module_state(candidate_meta.module)
                        == Some(ModuleRunState::Active);

                    // This is the ordinary poll-start read when the choice is
                    // stable. If it crosses a release it becomes an extra probe
                    // read and the complete measured sweep is repeated at that
                    // timestamp before any module is dispatched.
                    let dispatch_candidate_us = clock();
                    if dispatch_candidate_us < sweep_finished_us {
                        instrumentation_clock_invalid = true;
                        probe_clock_reads = probe_clock_reads.saturating_add(1);
                        selection_finished_us = dispatch_candidate_us;
                        break;
                    }
                    if next_release_us.is_some_and(|release| release <= dispatch_candidate_us) {
                        probe_clock_reads = probe_clock_reads.saturating_add(1);
                        selection_reevaluated = true;
                        snapshot_us = dispatch_candidate_us;
                        continue;
                    }
                    instrumented_start_us = Some(dispatch_candidate_us);
                    instrumented_meta = Some(candidate_meta);
                    instrumented_active = candidate_active;
                    scheduling_now_us = dispatch_candidate_us;
                    if !candidate_active {
                        // The ordinary inactive-module path has no poll-start
                        // sample, so this stable candidate read is probe-only.
                        probe_clock_reads = probe_clock_reads.saturating_add(1);
                    }
                    instrumentation_stable = true;
                    break;
                }
            }
            if !instrumentation_stable {
                if let Some(recorder) = instrumentation.as_mut() {
                    if instrumentation_clock_invalid {
                        recorder.record_clock_invalid();
                    }
                    if selection_reevaluated {
                        recorder.record_selection_reevaluated();
                    }
                    recorder.record_probe_scan_slots(peer_scan_slots);
                    recorder.record_selection(
                        selection_sweep_slots,
                        selection_due_tasks,
                        selection_started_us,
                        selection_finished_us,
                        probe_clock_reads,
                    );
                    recorder.record_selection_unstable();
                }
                outcome.idle_until_us =
                    self.next_activity_us::<ASYNC_DEADLINES, TIMERS>(async_deadlines);
                outcome.power_mode = Some(self.apply_idle(
                    scheduling_now_us,
                    true,
                    outcome.idle_until_us,
                    power_platform,
                )?);
                return Ok(outcome);
            }
        }
        let Some(idx) = selected else {
            if INSTRUMENTED {
                // Final post-recorder sample keeps the idle decision from
                // sleeping past work that became due while telemetry was
                // serialized. Count it before serialization so no recorder
                // mutation follows a valid final snapshot.
                probe_clock_reads = probe_clock_reads.saturating_add(1);
                if let Some(recorder) = instrumentation.as_mut() {
                    if instrumentation_clock_invalid {
                        recorder.record_clock_invalid();
                    }
                    if selection_reevaluated {
                        recorder.record_selection_reevaluated();
                    }
                    recorder.record_probe_scan_slots(peer_scan_slots);
                    recorder.record_selection(
                        selection_sweep_slots,
                        selection_due_tasks,
                        selection_started_us,
                        selection_finished_us,
                        probe_clock_reads,
                    );
                }
                let idle_now_us = clock();
                if idle_now_us < scheduling_now_us {
                    if let Some(recorder) = instrumentation.as_mut() {
                        recorder.record_clock_invalid();
                    }
                } else {
                    scheduling_now_us = idle_now_us;
                }
            }
            outcome.idle_until_us =
                self.next_activity_us::<ASYNC_DEADLINES, TIMERS>(async_deadlines);
            let work_pending = self.tasks.has_due(scheduling_now_us)
                || self
                    .runtime
                    .alarms()
                    .next_due_us()
                    .is_some_and(|due| due <= scheduling_now_us);
            outcome.power_mode = Some(self.apply_idle(
                scheduling_now_us,
                work_pending,
                outcome.idle_until_us,
                power_platform,
            )?);
            return Ok(outcome);
        };
        let meta = if INSTRUMENTED {
            instrumented_meta.ok_or(ExecError::TaskStateCorrupt)?
        } else {
            self.tasks.meta_at(idx).ok_or(ExecError::TaskStateCorrupt)?
        };
        let periodic_release = self.tasks.take_selected(idx);
        let module_active = if INSTRUMENTED {
            instrumented_active
        } else {
            self.runtime.module_state(meta.module) == Some(ModuleRunState::Active)
        };

        if !module_active {
            // Not runnable: the release is skipped and counted, never executed.
            self.tasks
                .skip_selected_release(idx, scheduling_now_us, periodic_release);
            outcome.skipped_release = Some(meta.module);
            if INSTRUMENTED {
                probe_clock_reads = probe_clock_reads.saturating_add(1);
                if let Some(recorder) = instrumentation.as_mut() {
                    if instrumentation_clock_invalid {
                        recorder.record_clock_invalid();
                    }
                    if selection_reevaluated {
                        recorder.record_selection_reevaluated();
                    }
                    recorder.record_probe_scan_slots(peer_scan_slots);
                    recorder.record_selection(
                        selection_sweep_slots,
                        selection_due_tasks,
                        selection_started_us,
                        selection_finished_us,
                        probe_clock_reads,
                    );
                }
                let skip_now_us = clock();
                if skip_now_us < scheduling_now_us {
                    if let Some(recorder) = instrumentation.as_mut() {
                        recorder.record_clock_invalid();
                    }
                } else {
                    scheduling_now_us = skip_now_us;
                }
            }
            outcome.idle_until_us =
                self.next_activity_us::<ASYNC_DEADLINES, TIMERS>(async_deadlines);
            let work_pending = self.tasks.has_due(scheduling_now_us)
                || self
                    .runtime
                    .alarms()
                    .next_due_us()
                    .is_some_and(|due| due <= scheduling_now_us);
            outcome.power_mode = Some(self.apply_idle(
                scheduling_now_us,
                work_pending,
                outcome.idle_until_us,
                power_platform,
            )?);
            return Ok(outcome);
        }

        let start_us = instrumented_start_us.unwrap_or_else(&clock);
        let sentinel_guard = SentinelArmGuard::new(
            &self.sentinel,
            meta.module,
            start_us.saturating_add(u64::from(meta.budget_us)),
        );
        let poll = self
            .runtime
            .with_module(meta.module, start_us, &mut dispatch);
        drop(sentinel_guard);
        let end_us = clock();
        if INSTRUMENTED && end_us < start_us {
            instrumentation_clock_invalid = true;
        }
        if INSTRUMENTED {
            if let Some(recorder) = instrumentation.as_mut() {
                if instrumentation_clock_invalid {
                    recorder.record_clock_invalid();
                }
                if selection_reevaluated {
                    recorder.record_selection_reevaluated();
                }
                recorder.record_probe_scan_slots(peer_scan_slots);
                recorder.record_selection(
                    selection_sweep_slots,
                    selection_due_tasks,
                    selection_started_us,
                    selection_finished_us,
                    probe_clock_reads,
                );
                recorder.record_poll_attempt();
                recorder.record_dispatch(
                    release_us,
                    start_us,
                    simultaneous_width,
                    module_code(meta.module),
                    meta.criticality as u32,
                    idx.min(u32::MAX as usize) as u32,
                );
                recorder.record_poll_clock(start_us, end_us);
            }
        }
        let poll = poll?;
        let duration_us = end_us.saturating_sub(start_us).min(u64::from(u32::MAX)) as u32;

        // Accounting is unconditional: measured time reaches the CPU ledger and
        // the task statistics whether the poll succeeded or faulted.
        self.runtime.charge_cpu(meta.module, duration_us);
        if !self
            .power
            .account_task(module_code(meta.module) as u16, u64::from(duration_us))
        {
            return Err(ExecError::PowerLedgerFull);
        }
        let stats = self
            .tasks
            .record_selected_poll(
                idx,
                end_us,
                duration_us,
                *poll.as_ref().unwrap_or(&Poll::Pending),
                periodic_release,
            )
            .ok_or(ExecError::TaskStateCorrupt)?;

        outcome.polled = Some(meta.module);
        outcome.duration_us = duration_us;

        match poll {
            Ok(result) => {
                outcome.poll = Some(result);
                if self.runtime.watchdog_entry(meta.module).is_some() {
                    self.runtime.heartbeat(meta.module, end_us)?;
                } else {
                    self.runtime.record_ok(meta.module, end_us)?;
                }
            }
            Err(error) => {
                let _ = self.runtime.record_error(meta.module, error, end_us)?;
            }
        }

        if duration_us > meta.budget_us {
            outcome.overrun = true;
            match self.containment {
                // Cooperative: the overrun is a deadline-contract violation and
                // enters the health pipeline (default policy faults the module,
                // handing it to recovery).
                ContainmentPolicy::Cooperative => {
                    let _ = self.runtime.record_error(
                        meta.module,
                        KernelError::DeadlineMissed,
                        end_us,
                    )?;
                }
                // Bounded: persistent overrunners are disabled outright after
                // the configured count — containment, not recovery.
                ContainmentPolicy::Bounded {
                    disable_after_overruns,
                } => {
                    if stats.overruns >= disable_after_overruns {
                        self.runtime.disable_module(meta.module, end_us)?;
                        outcome.contained = true;
                    }
                }
            }
        }

        outcome.idle_until_us = self.next_activity_us::<ASYNC_DEADLINES, TIMERS>(async_deadlines);
        let mut power_now_us = end_us;
        let mut work_pending = self.tasks.has_due(end_us)
            || self
                .runtime
                .alarms()
                .next_due_us()
                .is_some_and(|due| due <= end_us);
        if INSTRUMENTED {
            let bookkeeping_finished_us = clock();
            if let Some(recorder) = instrumentation.as_mut() {
                recorder.record_bookkeeping(end_us, bookkeeping_finished_us);
                // Account for the final post-serialization snapshot before it
                // is taken. A valid snapshot has no recorder mutation after it.
                recorder.record_probe_clock_reads(1);
            }
            let final_power_us = clock();
            if !instrumentation_clock_invalid
                && bookkeeping_finished_us >= end_us
                && final_power_us >= bookkeeping_finished_us
            {
                // Telemetry serialization itself can straddle the next
                // release. Base the power decision on the final post-telemetry
                // snapshot, never the stale poll-end/bookkeeping values.
                power_now_us = final_power_us;
                work_pending = self.tasks.has_due(power_now_us)
                    || self
                        .runtime
                        .alarms()
                        .next_due_us()
                        .is_some_and(|due| due <= power_now_us);
            } else {
                // A regressing clock cannot support an idle-safety decision.
                // Mark the evidence invalid and force the fail-closed active
                // path rather than programming a potentially late wake.
                work_pending = true;
                if let Some(recorder) = instrumentation.as_mut() {
                    recorder.record_clock_invalid();
                }
            }
        }
        outcome.power_mode = Some(self.apply_idle(
            power_now_us,
            work_pending,
            outcome.idle_until_us,
            power_platform,
        )?);
        Ok(outcome)
    }

    fn next_activity_us<const ASYNC_DEADLINES: bool, const TIMERS: usize>(
        &self,
        async_deadlines: Option<(ModuleId, &crate::TimerQueue<TIMERS>)>,
    ) -> Option<u64> {
        let task = self.tasks.next_due_us();
        let alarm = self.runtime.alarms().next_due_us();
        let kernel = match (task, alarm) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let async_deadline = if ASYNC_DEADLINES {
            async_deadlines.and_then(|(_, timers)| timers.next_deadline_us())
        } else {
            None
        };
        match (kernel, async_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn apply_idle(
        &self,
        now_us: u64,
        work_pending: bool,
        deadline_us: Option<u64>,
        platform: &mut impl PowerPlatform,
    ) -> Result<PowerMode, PowerHookError> {
        let ready_mask = self
            .tasks
            .next_release_arm()
            .filter(|arm| Some(arm.deadline_us) == deadline_us)
            .map_or(0, |arm| arm.ready_mask);
        self.power
            .apply_idle_release(now_us, work_pending, deadline_us, ready_mask, platform)
    }
}

/// Iterative response-time analysis for one task against the whole set.
/// Priority: higher criticality preempts; equal criticality is counted as
/// interference both ways (pessimistic, since the selector breaks ties by
/// release time, not a fixed order).
fn response_time<const N: usize>(
    task: TaskMeta,
    metas: &[Option<TaskMeta>; N],
    wake_latency_us: u32,
) -> Result<u64, ExecError> {
    let cost = u64::from(task.budget_us)
        .saturating_add(u64::from(task.blocking_us))
        .saturating_add(u64::from(wake_latency_us));
    let mut response = cost;
    // The response never needs more iterations than it has microseconds below
    // the deadline bound; cap defensively to stay bounded on pathological input.
    for _ in 0..64 {
        let mut next = cost;
        for other in metas.iter().flatten() {
            if other.module == task.module || other.criticality < task.criticality {
                continue;
            }
            let releases = response.div_ceil(u64::from(other.period_us));
            next = next.saturating_add(releases.saturating_mul(u64::from(other.budget_us)));
        }
        if next == response {
            return Ok(response);
        }
        if next > u64::from(task.deadline_us) {
            return Err(ExecError::Unschedulable {
                module: task.module,
                response_us: next,
                period_us: task.deadline_us,
            });
        }
        response = next;
    }
    Err(ExecError::Unschedulable {
        module: task.module,
        response_us: response,
        period_us: task.deadline_us,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        kernel_module_spec, Capability, CapabilitySet, Criticality, DeadlineContract,
        DependencySet, FaultThresholds, MemoryBudget, MessageKind, ModuleSpec, StartupNode,
        SystemManifest, SystemProfile,
    };
    use core::cell::Cell;

    type TestRuntime = Runtime<4, 4, 8, 4, 8, 4, 32>;
    type TestExecutor = KernelExecutor<4, 4, 4, 8, 4, 8, 4, 32>;

    #[derive(Default)]
    struct PowerHooks {
        wake: Option<u64>,
        ready_mask: u32,
        pending_ready: u32,
        mode: Option<PowerMode>,
        suspended: Option<u16>,
    }

    impl PowerPlatform for PowerHooks {
        fn program_wake(&mut self, deadline_us: Option<u64>) -> Result<(), PowerHookError> {
            self.wake = deadline_us;
            Ok(())
        }
        fn program_deadline_release(
            &mut self,
            deadline_us: Option<u64>,
            ready_mask: u32,
        ) -> Result<(), PowerHookError> {
            self.wake = deadline_us;
            self.ready_mask = ready_mask;
            Ok(())
        }
        fn take_deadline_releases(&mut self, _now_us: u64) -> u32 {
            core::mem::take(&mut self.pending_ready)
        }
        fn observed_wake_latency_us(&self) -> u32 {
            7
        }
        fn enter(&mut self, mode: PowerMode) -> Result<(), PowerHookError> {
            self.mode = Some(mode);
            Ok(())
        }
        fn suspend(&mut self, task_id: u16) -> Result<(), PowerHookError> {
            self.suspended = Some(task_id);
            Ok(())
        }
        fn resume(&mut self, task_id: u16) -> Result<(), PowerHookError> {
            if self.suspended != Some(task_id) {
                return Err(PowerHookError { source: 1, code: 1 });
            }
            self.suspended = None;
            Ok(())
        }
    }

    fn admitted_inputs() -> (SystemManifest<3>, [StartupNode; 3]) {
        let mut manifest = SystemManifest::<3>::new();
        manifest
            .add(kernel_module_spec(
                MemoryBudget::new(4096, 1024, 1),
                DeadlineContract::new(1000, 10),
            ))
            .unwrap();
        manifest
            .add(
                ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
                    .requires(CapabilitySet::empty().with(Capability::Mailbox))
                    .memory(MemoryBudget::new(1024, 256, 2)),
            )
            .unwrap();
        manifest
            .add(
                ModuleSpec::new(ModuleId::Actuator, Criticality::System)
                    .owns(CapabilitySet::empty().with(Capability::Mailbox))
                    .memory(MemoryBudget::new(1024, 256, 0)),
            )
            .unwrap();
        let nodes = [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty()),
            StartupNode::new(ModuleId::Sensor, DependencySet::empty()),
            StartupNode::new(ModuleId::Actuator, DependencySet::empty()),
        ];
        (manifest, nodes)
    }

    fn runtime() -> TestRuntime {
        let (manifest, nodes) = admitted_inputs();
        let mut runtime = TestRuntime::admit(
            &manifest,
            &nodes,
            SystemProfile::NRF52840_CORE,
            FaultThresholds::DEFAULT,
        )
        .unwrap();
        runtime.boot_to_running(0).unwrap();
        runtime
    }

    fn expect_init_error<T>(result: Result<T, ExecutorInitError>) -> ExecutorInitError {
        match result {
            Ok(_) => panic!("executor initialization unexpectedly succeeded"),
            Err(error) => error,
        }
    }

    #[test]
    fn executor_cell_cleans_every_completed_field_and_can_retry() {
        static CELL: KernelExecutorCell<4, 4, 4, 8, 4, 8, 4, 32> = KernelExecutorCell::new();
        let (manifest, nodes) = admitted_inputs();
        for (index, fail_stage) in ExecutorInitStage::ALL.into_iter().enumerate() {
            let cleanup = Cell::new(0u8);
            let error = expect_init_error(CELL.init_admitted_inner(
                &manifest,
                &nodes,
                SystemProfile::NRF52840_CORE,
                FaultThresholds::DEFAULT,
                ContainmentPolicy::Cooperative,
                &mut |stage| {
                    if stage == fail_stage {
                        Err(ExecutorInitError::Runtime(RuntimeError::PoolExhausted))
                    } else {
                        Ok(())
                    }
                },
                Some(&cleanup),
            ));
            assert_eq!(
                error,
                ExecutorInitError::Runtime(RuntimeError::PoolExhausted)
            );
            assert_eq!(cleanup.get(), (1u8 << (index + 1)) - 1);
            assert_eq!(CELL.state.load(Ordering::Acquire), CELL_EMPTY);
        }

        let executor = CELL
            .init_admitted(
                &manifest,
                &nodes,
                SystemProfile::NRF52840_CORE,
                FaultThresholds::DEFAULT,
                ContainmentPolicy::Cooperative,
            )
            .unwrap();
        assert_eq!(executor.runtime().plan().module_count(), 3);
        assert_eq!(
            expect_init_error(CELL.init_admitted(
                &manifest,
                &nodes,
                SystemProfile::NRF52840_CORE,
                FaultThresholds::DEFAULT,
                ContainmentPolicy::Cooperative,
            )),
            ExecutorInitError::AlreadyInitialized
        );
    }

    #[test]
    fn executor_cell_restores_empty_state_after_every_unwinding_stage() {
        static CELL: KernelExecutorCell<4, 4, 4, 8, 4, 8, 4, 32> = KernelExecutorCell::new();
        let (manifest, nodes) = admitted_inputs();
        for (index, panic_stage) in ExecutorInitStage::ALL.into_iter().enumerate() {
            let cleanup = Cell::new(0u8);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = CELL.init_admitted_inner(
                    &manifest,
                    &nodes,
                    SystemProfile::NRF52840_CORE,
                    FaultThresholds::DEFAULT,
                    ContainmentPolicy::Cooperative,
                    &mut |stage| {
                        assert_ne!(stage, panic_stage, "injected executor init panic");
                        Ok(())
                    },
                    Some(&cleanup),
                );
            }));
            assert!(result.is_err());
            assert_eq!(cleanup.get(), (1u8 << (index + 1)) - 1);
            assert_eq!(CELL.state.load(Ordering::Acquire), CELL_EMPTY);
        }
    }

    #[test]
    fn executor_cell_runtime_failure_never_publishes_partial_storage() {
        static CELL: KernelExecutorCell<4, 4, 4, 8, 4, 8, 4, 32> = KernelExecutorCell::new();
        let (manifest, nodes) = admitted_inputs();
        let error = expect_init_error(CELL.init_admitted(
            &manifest,
            &nodes[..2],
            SystemProfile::NRF52840_CORE,
            FaultThresholds::DEFAULT,
            ContainmentPolicy::Cooperative,
        ));
        assert!(matches!(error, ExecutorInitError::Runtime(_)));
        assert_eq!(CELL.state.load(Ordering::Acquire), CELL_EMPTY);

        let executor = CELL
            .init_admitted(
                &manifest,
                &nodes,
                SystemProfile::NRF52840_CORE,
                FaultThresholds::DEFAULT,
                ContainmentPolicy::Cooperative,
            )
            .unwrap();
        assert_eq!(executor.runtime().plan().module_count(), 3);
        assert!(
            KernelExecutorCell::<4, 4, 4, 8, 4, 8, 4, 32>::storage_bytes()
                >= core::mem::size_of::<TestExecutor>()
        );
    }

    #[test]
    fn executor_graph_workspace_keeps_scratch_disjoint_from_admission() {
        type CellType = KernelExecutorCell<4, 4, 4, 8, 4, 8, 4, 32>;
        type StorageType = KernelExecutorStorage<4, 4, 4, 8, 4, 8, 4, 32>;
        type WorkspaceType = ExecutorGraphWorkspace<4, 4>;
        static CELL: CellType = CellType::new();
        let cell = &CELL;

        unsafe {
            let destination = cell.destination();
            let runtime = core::ptr::addr_of_mut!((*destination).runtime);
            let (admission, admission_size) = TestRuntime::admission_storage_range(runtime);
            let scratch = cell.graph_scratch_destination().cast::<u8>();
            let admission_start = admission as usize;
            let admission_end = admission_start + admission_size;
            let scratch_start = scratch as usize;
            let scratch_end = scratch_start + core::mem::size_of::<ExecutorGraphScratch<4>>();

            assert!(admission_end <= scratch_start || scratch_end <= admission_start);
            assert_eq!(admission_start, destination as usize);
        }
        assert_eq!(
            core::mem::size_of::<StorageType>(),
            core::mem::size_of::<TestExecutor>().max(core::mem::size_of::<WorkspaceType>())
        );
        assert!(core::mem::size_of::<WorkspaceType>() <= core::mem::size_of::<TestExecutor>());
        assert!(CellType::storage_bytes() >= core::mem::size_of::<StorageType>());
    }

    #[test]
    fn executor_cell_allows_exactly_one_initializer_across_threads() {
        static CELL: KernelExecutorCell<4, 4, 4, 8, 4, 8, 4, 32> = KernelExecutorCell::new();
        let (manifest, nodes) = admitted_inputs();
        let winners = std::thread::scope(|scope| {
            let contenders = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        match CELL.init_admitted(
                            &manifest,
                            &nodes,
                            SystemProfile::NRF52840_CORE,
                            FaultThresholds::DEFAULT,
                            ContainmentPolicy::Cooperative,
                        ) {
                            Ok(executor) => {
                                assert_eq!(executor.runtime().plan().module_count(), 3);
                                1u8
                            }
                            Err(ExecutorInitError::AlreadyInitialized) => 0,
                            Err(error) => panic!("unexpected executor init failure: {error:?}"),
                        }
                    })
                })
                .collect::<std::vec::Vec<_>>();
            contenders
                .into_iter()
                .map(|thread| thread.join().expect("initializer thread"))
                .sum::<u8>()
        });
        assert_eq!(winners, 1);
        assert_eq!(CELL.state.load(Ordering::Acquire), CELL_READY);
    }

    #[test]
    fn executor_refuses_to_run_unsealed_and_rejects_unschedulable_sets() {
        let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1000, 600),
            0,
        )
        .unwrap();
        let mut power = PowerHooks::default();
        assert_eq!(
            exec.run_cycle(|| 0, &mut power, |_| Ok(Poll::Ready)).err(),
            Some(ExecError::NotSealed)
        );

        // A second task pushes the set over the schedulability bound:
        // actuator (System) interferes 700us per 1000us; sensor response
        // 600 + 700 = 1300 > 1000.
        exec.add_task(
            TaskMeta::new(ModuleId::Actuator, Criticality::System, 1000, 700),
            0,
        )
        .unwrap();
        assert!(matches!(
            exec.seal(),
            Err(ExecError::Unschedulable {
                module: ModuleId::Sensor,
                ..
            })
        ));
    }

    #[test]
    fn executor_admits_the_measured_wake_bound_and_freezes_it_at_seal() {
        let mut exact = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exact
            .add_task(
                TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1_000, 900),
                0,
            )
            .unwrap();
        exact.set_wake_latency_us(100).unwrap();
        assert_eq!(exact.wake_latency_us(), 100);
        exact.seal().unwrap();
        assert_eq!(exact.set_wake_latency_us(0), Err(ExecError::Sealed));

        let mut late = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        late.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1_000, 900),
            0,
        )
        .unwrap();
        late.set_wake_latency_us(101).unwrap();
        assert!(matches!(
            late.seal(),
            Err(ExecError::Unschedulable {
                module: ModuleId::Sensor,
                response_us: 1_001,
                period_us: 1_000,
            })
        ));
    }

    #[test]
    fn one_cycle_selects_measures_charges_and_reports_idle() {
        let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 10_000, 2_000),
            0,
        )
        .unwrap();
        exec.add_task(
            TaskMeta::new(ModuleId::Actuator, Criticality::System, 20_000, 2_000),
            0,
        )
        .unwrap();
        assert!(exec.set_task_power(ModuleId::Actuator, 2_000_000));
        exec.seal().unwrap();

        // Deterministic clock: entry, poll start, poll end, ...
        let ticks = Cell::new(0u64);
        let clock = || {
            let t = ticks.get();
            ticks.set(t + 500);
            t
        };
        let outcome = exec
            .run_cycle(clock, &mut PowerHooks::default(), |ctx| {
                // Higher criticality wins the tie: actuator runs first and can
                // use its granted mailbox capability through the context.
                assert_eq!(ctx.module(), ModuleId::Actuator);
                ctx.send(ModuleId::Sensor, MessageKind::Command, 7, 0)
                    .map_err(|_| KernelError::SensorReadFail)?;
                Ok(Poll::Ready)
            })
            .unwrap();

        assert_eq!(outcome.polled, Some(ModuleId::Actuator));
        assert_eq!(outcome.poll, Some(Poll::Ready));
        assert_eq!(outcome.duration_us, 500);
        assert!(!outcome.overrun);
        // Measured time reached the CPU ledger.
        assert_eq!(
            exec.runtime()
                .object_usage(ModuleId::Actuator)
                .unwrap()
                .cpu_us,
            500
        );
        // The sensor task is still due: the loop names the next activity.
        assert_eq!(outcome.idle_until_us, Some(0));
        assert_eq!(exec.power().ledger().energy_uj(6), Some(1_000));
    }

    #[test]
    fn opt_in_timing_report_observes_simultaneous_release_order() {
        let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 10_000, 2_000),
            0,
        )
        .unwrap();
        exec.add_task(
            TaskMeta::new(ModuleId::Actuator, Criticality::System, 20_000, 2_000),
            0,
        )
        .unwrap();
        exec.seal().unwrap();

        let ticks = Cell::new(0u64);
        let clock = || {
            let now = ticks.get();
            ticks.set(now + 10);
            now
        };
        let mut recorder =
            ExecutorInstrumentation::<8>::with_identity(crate::ReportIdentity::new(1, 2, 3));
        let mut power = PowerHooks::default();
        exec.run_cycle_instrumented(clock, &mut power, |_| Ok(Poll::Ready), &mut recorder)
            .unwrap();
        exec.run_cycle_instrumented(clock, &mut power, |_| Ok(Poll::Ready), &mut recorder)
            .unwrap();

        let report = recorder.report();
        assert!(report.verify_checksum());
        assert_eq!(report.dispatch_samples, 2);
        assert_eq!(report.simultaneous_release_groups, 1);
        assert_eq!(report.simultaneous_max_width, 2);
        assert_eq!(report.simultaneous_max_rank, 2);
        assert_ne!(report.simultaneous_order_hash, 0x811C_9DC5);
        assert_eq!(report.selection_sweep_slots_total(), 2);
        assert_eq!(report.selection_due_tasks_total(), 3);
        assert_eq!(report.selection_duration_total_us(), 20);
        assert_eq!(report.poll_bookkeeping_total_us(), 20);
        assert_eq!(report.probe_clock_reads, 8);
        assert_eq!(report.probe_scan_slots_total(), 0);
        assert_eq!(report.completed, 1);
    }

    #[test]
    fn ordinary_and_instrumented_cycles_match_outputs_with_probe_reads_opt_in() {
        fn executor() -> TestExecutor {
            let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
            exec.add_task(
                TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1_000, 100),
                0,
            )
            .unwrap();
            exec.seal().unwrap();
            exec
        }

        let mut ordinary = executor();
        let ordinary_values = [0, 30, 40];
        let ordinary_calls = Cell::new(0usize);
        let ordinary_clock = || {
            let index = ordinary_calls.get();
            ordinary_calls.set(index + 1);
            ordinary_values[index]
        };
        let ordinary_outcome = ordinary
            .run_cycle(ordinary_clock, &mut PowerHooks::default(), |_| {
                Ok(Poll::Ready)
            })
            .unwrap();

        let mut instrumented = executor();
        let instrumented_values = [0, 10, 20, 30, 40, 50, 60];
        let instrumented_calls = Cell::new(0usize);
        let instrumented_clock = || {
            let index = instrumented_calls.get();
            instrumented_calls.set(index + 1);
            instrumented_values[index]
        };
        let mut recorder =
            ExecutorInstrumentation::<1>::with_identity(crate::ReportIdentity::new(1, 2, 3));
        let instrumented_outcome = instrumented
            .run_cycle_instrumented(
                instrumented_clock,
                &mut PowerHooks::default(),
                |_| Ok(Poll::Ready),
                &mut recorder,
            )
            .unwrap();

        assert_eq!(ordinary_calls.get(), 3);
        assert_eq!(instrumented_calls.get(), 7);
        assert_eq!(ordinary_outcome, instrumented_outcome);
        assert_eq!(recorder.report().probe_clock_reads, 4);
    }

    #[test]
    fn instrumented_selection_rechecks_a_release_crossed_by_its_probe() {
        let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1_000, 100),
            0,
        )
        .unwrap();
        exec.add_task(
            TaskMeta::new(ModuleId::Actuator, Criticality::System, 1_000, 100),
            8,
        )
        .unwrap();
        exec.seal().unwrap();
        let values = [0, 5, 10, 11, 12, 13, 14, 15];
        let calls = Cell::new(0usize);
        let clock = || {
            let index = calls.get();
            calls.set(index + 1);
            values[index]
        };
        let mut recorder =
            ExecutorInstrumentation::<2>::with_identity(crate::ReportIdentity::new(1, 2, 3));
        let outcome = exec
            .run_cycle_instrumented(
                clock,
                &mut PowerHooks::default(),
                |_| Ok(Poll::Ready),
                &mut recorder,
            )
            .unwrap();
        assert_eq!(outcome.polled, Some(ModuleId::Actuator));
        let report = recorder.report();
        assert_ne!(report.flags & crate::EXECUTOR_FLAG_SELECTION_REEVALUATED, 0);
        assert_eq!(report.selection_sweep_slots_total(), 2);
        assert_eq!(report.probe_scan_slots_total(), 0);
    }

    #[test]
    fn instrumented_post_telemetry_snapshot_prevents_sleeping_past_new_work() {
        let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1_000, 100),
            0,
        )
        .unwrap();
        exec.add_task(
            TaskMeta::new(ModuleId::Actuator, Criticality::System, 1_000, 100),
            50,
        )
        .unwrap();
        exec.seal().unwrap();
        // The actuator becomes due after the measured bookkeeping snapshot
        // (40) but before the final post-telemetry snapshot (60).
        let values = [0, 1, 2, 3, 4, 40, 60];
        let calls = Cell::new(0usize);
        let clock = || {
            let index = calls.get();
            calls.set(index + 1);
            values[index]
        };
        let mut recorder =
            ExecutorInstrumentation::<2>::with_identity(crate::ReportIdentity::new(1, 2, 3));
        let mut hooks = PowerHooks::default();
        let outcome = exec
            .run_cycle_instrumented(clock, &mut hooks, |_| Ok(Poll::Ready), &mut recorder)
            .unwrap();
        assert_eq!(outcome.idle_until_us, Some(50));
        assert_eq!(outcome.power_mode, Some(PowerMode::Active));
        assert_eq!(hooks.wake, None);
        assert_eq!(recorder.report().poll_bookkeeping_max_us, 36);
        assert_eq!(recorder.report().probe_clock_reads, 4);
    }

    #[test]
    fn instrumented_global_clock_regression_invalidates_report_and_forces_active() {
        let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1_000, 100),
            0,
        )
        .unwrap();
        exec.seal().unwrap();
        let values = [100, 110, 120, 130, 90, 140, 150];
        let calls = Cell::new(0usize);
        let clock = || {
            let index = calls.get();
            calls.set(index + 1);
            values[index]
        };
        let mut recorder =
            ExecutorInstrumentation::<1>::with_identity(crate::ReportIdentity::new(1, 2, 3));
        let mut hooks = PowerHooks::default();
        let outcome = exec
            .run_cycle_instrumented(clock, &mut hooks, |_| Ok(Poll::Ready), &mut recorder)
            .unwrap();
        let report = recorder.report();
        assert!(!report.clock_valid());
        assert_eq!(report.completed, 0);
        assert_eq!(outcome.power_mode, Some(PowerMode::Active));
        assert_eq!(hooks.wake, None);
    }

    #[test]
    fn executor_programs_idle_wake_and_runs_peripheral_power_hooks() {
        let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 10_000, 1_000),
            10_000,
        )
        .unwrap();
        exec.seal().unwrap();
        let mut hooks = PowerHooks::default();
        let outcome = exec
            .run_cycle(|| 0, &mut hooks, |_| panic!("not due"))
            .unwrap();
        assert_eq!(outcome.idle_until_us, Some(10_000));
        assert_eq!(outcome.power_mode, Some(PowerMode::LowPower));
        assert_eq!(hooks.wake, Some(10_000));
        assert_eq!(hooks.ready_mask, 1);

        // Model the compare ISR publishing the exact armed group. The normal
        // cycle drains it; application code performs no release plumbing.
        hooks.pending_ready = hooks.ready_mask;
        let released = exec
            .run_cycle(|| 10_000, &mut hooks, |_| Ok(Poll::Ready))
            .unwrap();
        assert_eq!(released.isr_releases, 1);
        assert_eq!(released.rejected_isr_releases, 0);
        assert_eq!(released.observed_wake_latency_us, 7);
        assert_eq!(released.polled, Some(ModuleId::Sensor));

        exec.suspend_module(ModuleId::Sensor, 1, &mut hooks)
            .unwrap();
        assert_eq!(hooks.suspended, Some(5));
        exec.resume_module(ModuleId::Sensor, 2, &mut hooks).unwrap();
        assert_eq!(hooks.suspended, None);
    }

    #[test]
    fn bounded_containment_disables_a_persistent_overrunner() {
        let mut exec = TestExecutor::new(
            runtime(),
            ContainmentPolicy::Bounded {
                disable_after_overruns: 2,
            },
        );
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 100_000, 100),
            0,
        )
        .unwrap();
        exec.seal().unwrap();

        // Each poll takes 1000us against a 100us budget.
        let ticks = Cell::new(0u64);
        let clock = || {
            let t = ticks.get();
            ticks.set(t + 1_000);
            t
        };
        let mut power = PowerHooks::default();
        let first = exec
            .run_cycle(clock, &mut power, |_| Ok(Poll::Pending))
            .unwrap();
        assert!(first.overrun && !first.contained);
        // Force the next release to be due immediately.
        ticks.set(200_000);
        let second = exec
            .run_cycle(clock, &mut power, |_| Ok(Poll::Pending))
            .unwrap();
        assert!(second.overrun && second.contained);
        assert_eq!(
            exec.runtime().module_state(ModuleId::Sensor),
            Some(ModuleRunState::Disabled)
        );

        // The disabled module is never polled again: its releases are skipped
        // and counted.
        ticks.set(400_000);
        let third = exec
            .run_cycle(clock, &mut power, |_| panic!("must not poll"))
            .unwrap();
        assert_eq!(third.skipped_release, Some(ModuleId::Sensor));
        assert_eq!(third.polled, None);
    }

    #[test]
    fn stack_guard_enforcement_attributes_and_routes_into_recovery() {
        use crate::{StackGuardTable, StackRegion};
        let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exec.seal().unwrap();

        let mut fake_stack = [0u8; 64];
        let mut guards = StackGuardTable::<1>::new();
        unsafe {
            guards
                .register(
                    ModuleId::Sensor,
                    StackRegion {
                        base: fake_stack.as_mut_ptr() as usize,
                        len: fake_stack.len(),
                        canary_bytes: 8,
                    },
                )
                .unwrap();
        }
        assert_eq!(exec.enforce_stack_guards(&guards, 10).unwrap(), None);

        // Overflow the fake stack through its canary.
        for byte in fake_stack.iter_mut() {
            *byte = 0xEE;
        }
        let fault = exec
            .enforce_stack_guards(&guards, 20)
            .unwrap()
            .expect("stack fault");
        assert_eq!(fault.module, ModuleId::Sensor);
        // The violation entered the health pipeline for the module.
        let health = exec.runtime().health_report(ModuleId::Sensor).unwrap();
        assert!(health.total_errors >= 1);
        assert_eq!(
            health.last_error,
            crate::error_code(KernelError::StackViolation)
        );
    }

    #[test]
    fn sentinel_flags_a_non_yielding_poll_for_the_isr() {
        let sentinel = ExecutionSentinel::new();
        assert_eq!(sentinel.check(1_000), None);
        sentinel.sequence.store(1, Ordering::Release);
        sentinel
            .module
            .store(module_code(ModuleId::Radio), Ordering::Release);
        sentinel.deadline_lo.store(10, Ordering::Relaxed);
        sentinel.deadline_hi.store(0, Ordering::Release);
        assert_eq!(sentinel.check(1_000), None);
        sentinel.sequence.store(2, Ordering::Release);
        sentinel.arm(ModuleId::Radio, 5_000);
        assert_eq!(sentinel.check(4_999), None);
        let stuck = sentinel.check(6_000).expect("stuck poll");
        assert_eq!(stuck.module_code, module_code(ModuleId::Radio));
        assert_eq!(stuck.late_us, 1_000);
        sentinel.disarm();
        assert_eq!(sentinel.check(10_000), None);
    }

    #[test]
    fn sentinel_is_disarmed_when_dispatch_unwinds() {
        let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1_000, 100),
            0,
        )
        .unwrap();
        exec.seal().unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = exec.run_cycle(
                || 0,
                &mut PowerHooks::default(),
                |_| panic!("injected dispatch panic"),
            );
        }));
        assert!(result.is_err());
        assert_eq!(exec.sentinel().check(u64::MAX), None);
    }

    #[test]
    fn sentinel_never_reports_a_torn_module_deadline_pair() {
        use std::sync::atomic::{AtomicBool, AtomicU32 as StdAtomicU32, Ordering as StdOrdering};

        let sentinel = ExecutionSentinel::new();
        let finished = AtomicBool::new(false);
        let observations = StdAtomicU32::new(0);
        sentinel.arm(ModuleId::Radio, 100);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let iterations = if cfg!(miri) { 1_000 } else { 100_000 };
                for index in 0..iterations {
                    sentinel.arm(ModuleId::Radio, 100);
                    sentinel.disarm();
                    sentinel.arm(ModuleId::Sensor, 200);
                    if index % 64 == 0 {
                        std::thread::yield_now();
                    }
                }
                finished.store(true, StdOrdering::Release);
            });
            scope.spawn(|| {
                while !finished.load(StdOrdering::Acquire)
                    || observations.load(StdOrdering::Relaxed) == 0
                {
                    if let Some(stuck) = sentinel.check(1_000) {
                        match stuck.module_code {
                            code if code == module_code(ModuleId::Radio) => {
                                assert_eq!(stuck.late_us, 900)
                            }
                            code if code == module_code(ModuleId::Sensor) => {
                                assert_eq!(stuck.late_us, 800)
                            }
                            code => panic!("unexpected sentinel module code {code}"),
                        }
                        observations.fetch_add(1, StdOrdering::Relaxed);
                    }
                }
            });
        });
        assert!(observations.load(StdOrdering::Relaxed) > 0);
        sentinel.disarm();
    }

    #[test]
    fn schedulable_set_passes_rta_with_interference() {
        // sensor: C=100 T=1000 (Driver); actuator: C=200 T=2000 (System).
        // sensor response = 100 + ceil(R/2000)*200 -> 300 <= 1000: schedulable.
        let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1000, 100),
            0,
        )
        .unwrap();
        exec.add_task(
            TaskMeta::new(ModuleId::Actuator, Criticality::System, 2000, 200),
            0,
        )
        .unwrap();
        exec.seal().unwrap();
        assert_eq!(
            exec.add_task(
                TaskMeta::new(ModuleId::Kernel, Criticality::HardRealtime, 1000, 1),
                0
            ),
            Err(ExecError::Sealed)
        );
    }

    #[test]
    fn measured_blocking_term_participates_in_response_time_admission() {
        let mut exec = TestExecutor::new(runtime(), ContainmentPolicy::Cooperative);
        exec.add_task(
            TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 1000, 100).with_blocking_us(750),
            0,
        )
        .unwrap();
        exec.add_task(
            TaskMeta::new(ModuleId::Actuator, Criticality::System, 2000, 200),
            0,
        )
        .unwrap();
        assert!(matches!(
            exec.seal(),
            Err(ExecError::Unschedulable {
                module: ModuleId::Sensor,
                ..
            })
        ));
    }
}
