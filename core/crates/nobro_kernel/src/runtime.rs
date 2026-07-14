//! Fixed-capacity runtime control plane assembled after admission.

#[cfg(test)]
use core::cell::Cell;

use crate::{
    AdmissionController, AdmissionError, AdmissionPlan, Alarm, AlarmError, AlarmId, AlarmQueue,
    Capability, CapabilityGrantError, CapabilityReplayScope, CapabilityTrace, CapabilityTraceInput,
    CapabilityTraceOp, CapabilityTraceRecord, DegradeApplicationReport, DegradeDecision,
    DegradeReason, DependencyImpact, EventLogReport, EventSeverity, FaultPolicy,
    FaultThresholdError, FaultThresholds, HealthFault, HealthReport, HotReloadError,
    HotReloadOutcome, HotReloadPlan, KernelError, KvError, KvKey, KvStore, KvValue, LeaseReleaser,
    Mailbox, MailboxError, Message, MessageKind, ModuleHookError, ModuleId, ModuleLifecycleHooks,
    ModuleReloadHooks, ModuleReloadRequest, ModuleRunState, ModuleRuntimeEntry, ModuleRuntimeError,
    ModuleRuntimeGuard, ModuleRuntimeReport, ObjectKind, ObjectLedger, ObjectQuota,
    ObjectQuotaError, ObjectUsage, QuotaError, RecoveryCoordinator, RecoveryError, RecoveryOutcome,
    RecoveryPlan, RecoveryPlanError, RecoveryPlanPolicy, RecoveryStep, RecoveryStepKind,
    RuntimeReport, RuntimeReportInput, StartupGraph, StartupNode, SystemBudget, SystemManifest,
    SystemProfile, SystemState, Watchdog, WatchdogEntry, WatchdogError,
};

/// Capacities a `Runtime` instantiation was compiled with, and the coherence
/// failure reported when they cannot serve the admitted module set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeCapacities {
    pub startup: usize,
    pub quotas: usize,
    pub mailbox: usize,
    pub alarms: usize,
    pub kv: usize,
    pub health: usize,
    pub log: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityError {
    /// A per-module table is smaller than the admitted module count, so some
    /// module would silently lack quota/health/object tracking.
    ModuleTablesTooSmall {
        modules: usize,
        capacities: RuntimeCapacities,
    },
    /// A shared queue was compiled with zero capacity.
    EmptyQueue { capacities: RuntimeCapacities },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    Admission(AdmissionError),
    Alarm(AlarmError),
    Capability(CapabilityGrantError),
    Kv(KvError),
    Mailbox(MailboxError),
    Module(ModuleRuntimeError),
    HotReload(HotReloadError),
    FaultThreshold(FaultThresholdError),
    ModuleHook(ModuleHookError),
    RecoveryNotActive(ModuleId),
    KvOwnedByOther { key: KvKey, owner: ModuleId },
    AlarmOwnedByOther { id: AlarmId, owner: ModuleId },
    PoolExhausted,
    PoolStaleHandle,
    Quota(QuotaError),
    Object(ObjectQuotaError),
    Capacity(CapacityError),
    Recovery(RecoveryError),
    RecoveryPlan(RecoveryPlanError),
    Watchdog(WatchdogError),
}

impl From<ObjectQuotaError> for RuntimeError {
    fn from(error: ObjectQuotaError) -> Self {
        Self::Object(error)
    }
}

impl From<CapacityError> for RuntimeError {
    fn from(error: CapacityError) -> Self {
        Self::Capacity(error)
    }
}

impl From<AdmissionError> for RuntimeError {
    fn from(error: AdmissionError) -> Self {
        Self::Admission(error)
    }
}

impl From<AlarmError> for RuntimeError {
    fn from(error: AlarmError) -> Self {
        Self::Alarm(error)
    }
}

impl From<CapabilityGrantError> for RuntimeError {
    fn from(error: CapabilityGrantError) -> Self {
        Self::Capability(error)
    }
}

impl From<KvError> for RuntimeError {
    fn from(error: KvError) -> Self {
        Self::Kv(error)
    }
}

impl From<MailboxError> for RuntimeError {
    fn from(error: MailboxError) -> Self {
        Self::Mailbox(error)
    }
}

impl From<ModuleRuntimeError> for RuntimeError {
    fn from(error: ModuleRuntimeError) -> Self {
        Self::Module(error)
    }
}

impl From<HotReloadError> for RuntimeError {
    fn from(error: HotReloadError) -> Self {
        Self::HotReload(error)
    }
}

impl From<FaultThresholdError> for RuntimeError {
    fn from(error: FaultThresholdError) -> Self {
        Self::FaultThreshold(error)
    }
}

impl From<ModuleHookError> for RuntimeError {
    fn from(error: ModuleHookError) -> Self {
        Self::ModuleHook(error)
    }
}

impl From<QuotaError> for RuntimeError {
    fn from(error: QuotaError) -> Self {
        Self::Quota(error)
    }
}

impl From<RecoveryError> for RuntimeError {
    fn from(error: RecoveryError) -> Self {
        Self::Recovery(error)
    }
}

impl From<RecoveryPlanError> for RuntimeError {
    fn from(error: RecoveryPlanError) -> Self {
        Self::RecoveryPlan(error)
    }
}

impl From<WatchdogError> for RuntimeError {
    fn from(error: WatchdogError) -> Self {
        Self::Watchdog(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchdogSweep<const N: usize> {
    pub outcomes: [Option<RecoveryOutcome>; N],
    pub len: usize,
}

impl<const N: usize> WatchdogSweep<N> {
    pub const fn new() -> Self {
        Self {
            outcomes: [None; N],
            len: 0,
        }
    }

    pub fn push(&mut self, outcome: RecoveryOutcome) {
        if self.len < N {
            self.outcomes[self.len] = Some(outcome);
            self.len += 1;
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const N: usize> Default for WatchdogSweep<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryPlanning<const N: usize> {
    pub outcome: RecoveryOutcome,
    pub plan: RecoveryPlan<N>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlarmDispatch {
    pub dispatched: usize,
    pub blocked: Option<Alarm>,
    pub error: Option<RuntimeError>,
}

impl AlarmDispatch {
    pub const fn completed(dispatched: usize) -> Self {
        Self {
            dispatched,
            blocked: None,
            error: None,
        }
    }

    pub const fn blocked(dispatched: usize, alarm: Alarm, error: RuntimeError) -> Self {
        Self {
            dispatched,
            blocked: Some(alarm),
            error: Some(error),
        }
    }

    pub const fn is_blocked(&self) -> bool {
        self.blocked.is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DegradeApplication {
    pub requested: usize,
    pub disabled: usize,
    pub already_disabled: usize,
    pub reason: Option<DegradeReason>,
    pub applied_at_us: u64,
}

impl DegradeApplication {
    pub const fn none() -> Self {
        Self {
            requested: 0,
            disabled: 0,
            already_disabled: 0,
            reason: None,
            applied_at_us: 0,
        }
    }

    pub const fn touched_modules(&self) -> usize {
        self.disabled + self.already_disabled
    }
}

pub struct Runtime<
    const STARTUP: usize,
    const QUOTAS: usize,
    const MAILBOX: usize,
    const ALARMS: usize,
    const KV: usize,
    const HEALTH: usize,
    const LOG: usize,
> {
    plan: AdmissionPlan<STARTUP, QUOTAS>,
    mailbox: Mailbox<MAILBOX>,
    alarms: AlarmQueue<ALARMS>,
    kv: KvStore<KV>,
    kv_owners: [Option<(KvKey, ModuleId)>; KV],
    recovery: RecoveryCoordinator<HEALTH, LOG>,
    watchdog: Watchdog<HEALTH>,
    modules: ModuleRuntimeGuard<QUOTAS>,
    objects: ObjectLedger<QUOTAS>,
    trace: CapabilityTrace<LOG>,
    degrade: DegradeApplication,
}

/// Internal checkpoints for the guarded in-place constructor.  Keeping these
/// explicit makes it possible to exercise cleanup after every initialized
/// field under Miri without adding a runtime fault-injection surface.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeInitStage {
    Plan,
    Mailbox,
    Alarms,
    Kv,
    KvOwners,
    Recovery,
    Watchdog,
    Modules,
    Objects,
    Trace,
    Degrade,
}

#[cfg(test)]
impl RuntimeInitStage {
    const ALL: [Self; 11] = [
        Self::Plan,
        Self::Mailbox,
        Self::Alarms,
        Self::Kv,
        Self::KvOwners,
        Self::Recovery,
        Self::Watchdog,
        Self::Modules,
        Self::Objects,
        Self::Trace,
        Self::Degrade,
    ];
}

struct RuntimeInitGuard<
    const STARTUP: usize,
    const QUOTAS: usize,
    const MAILBOX: usize,
    const ALARMS: usize,
    const KV: usize,
    const HEALTH: usize,
    const LOG: usize,
> {
    destination: *mut Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
    initialized: u16,
    armed: bool,
    #[cfg(test)]
    cleanup_mask: Option<*const Cell<u16>>,
}

impl<
        const STARTUP: usize,
        const QUOTAS: usize,
        const MAILBOX: usize,
        const ALARMS: usize,
        const KV: usize,
        const HEALTH: usize,
        const LOG: usize,
    > RuntimeInitGuard<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>
{
    fn new(
        destination: *mut Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        #[cfg(test)] cleanup_mask: Option<&Cell<u16>>,
    ) -> Self {
        Self {
            destination,
            initialized: 0,
            armed: true,
            #[cfg(test)]
            cleanup_mask: cleanup_mask.map(core::ptr::from_ref),
        }
    }

    fn mark(&mut self, stage: RuntimeInitStage) {
        self.initialized |= 1 << stage as u8;
    }

    fn finish(mut self) {
        self.armed = false;
    }

    fn has(&self, stage: RuntimeInitStage) -> bool {
        self.initialized & (1 << stage as u8) != 0
    }
}

impl<
        const STARTUP: usize,
        const QUOTAS: usize,
        const MAILBOX: usize,
        const ALARMS: usize,
        const KV: usize,
        const HEALTH: usize,
        const LOG: usize,
    > Drop for RuntimeInitGuard<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>
{
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        #[cfg(test)]
        if let Some(cleanup_mask) = self.cleanup_mask {
            // SAFETY: the test observer outlives the constructor call and this
            // guard; production builds contain neither the pointer nor branch.
            unsafe { (*cleanup_mask).set(self.initialized) };
        }

        // Drop in reverse initialization order, and only fields whose writes
        // completed.  `addr_of_mut!` does not form a reference to the still
        // partially initialized enclosing `Runtime`.
        unsafe {
            let destination = self.destination;
            if self.has(RuntimeInitStage::Degrade) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).degrade));
            }
            if self.has(RuntimeInitStage::Trace) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).trace));
            }
            if self.has(RuntimeInitStage::Objects) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).objects));
            }
            if self.has(RuntimeInitStage::Modules) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).modules));
            }
            if self.has(RuntimeInitStage::Watchdog) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).watchdog));
            }
            if self.has(RuntimeInitStage::Recovery) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).recovery));
            }
            if self.has(RuntimeInitStage::KvOwners) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).kv_owners));
            }
            if self.has(RuntimeInitStage::Kv) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).kv));
            }
            if self.has(RuntimeInitStage::Alarms) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).alarms));
            }
            if self.has(RuntimeInitStage::Mailbox) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).mailbox));
            }
            if self.has(RuntimeInitStage::Plan) {
                core::ptr::drop_in_place(core::ptr::addr_of_mut!((*destination).plan));
            }
        }
    }
}

impl<
        const STARTUP: usize,
        const QUOTAS: usize,
        const MAILBOX: usize,
        const ALARMS: usize,
        const KV: usize,
        const HEALTH: usize,
        const LOG: usize,
    > Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>
{
    /// The capacities this instantiation was compiled with.
    pub const fn capacities() -> RuntimeCapacities {
        RuntimeCapacities {
            startup: STARTUP,
            quotas: QUOTAS,
            mailbox: MAILBOX,
            alarms: ALARMS,
            kv: KV,
            health: HEALTH,
            log: LOG,
        }
    }

    /// One coherence check over the const-generic capacities, run before any
    /// runtime is assembled: every admitted module needs a startup, quota,
    /// object, and health slot, and the shared queues must be non-empty.
    fn validate_capacities(modules: usize) -> Result<(), CapacityError> {
        let capacities = Self::capacities();
        if modules > QUOTAS || modules > HEALTH || modules > STARTUP {
            return Err(CapacityError::ModuleTablesTooSmall {
                modules,
                capacities,
            });
        }
        if MAILBOX == 0 || ALARMS == 0 || KV == 0 || LOG == 0 {
            return Err(CapacityError::EmptyQueue { capacities });
        }
        Ok(())
    }

    /// Assemble a complete admitted runtime directly at `destination`.
    ///
    /// The caller owns an uninitialized, properly aligned `Self` slot and must
    /// not read, move, or expose it until this function returns `Ok(())`.  On
    /// every error or unwinding panic, the internal guard drops exactly the
    /// fields whose initialization completed.
    pub(crate) unsafe fn admit_in_place<const MODULES: usize>(
        destination: *mut Self,
        manifest: &SystemManifest<MODULES>,
        startup_nodes: &[StartupNode],
        profile: SystemProfile,
        thresholds: FaultThresholds,
    ) -> Result<(), RuntimeError> {
        let mut checkpoint = |_stage| Ok(());
        #[cfg(not(test))]
        {
            Self::admit_in_place_inner(
                destination,
                manifest,
                startup_nodes,
                profile,
                thresholds,
                &mut checkpoint,
            )
        }
        #[cfg(test)]
        {
            Self::admit_in_place_inner(
                destination,
                manifest,
                startup_nodes,
                profile,
                thresholds,
                &mut checkpoint,
                None,
            )
        }
    }

    unsafe fn admit_in_place_inner<const MODULES: usize>(
        destination: *mut Self,
        manifest: &SystemManifest<MODULES>,
        startup_nodes: &[StartupNode],
        profile: SystemProfile,
        thresholds: FaultThresholds,
        checkpoint: &mut impl FnMut(RuntimeInitStage) -> Result<(), RuntimeError>,
        #[cfg(test)] cleanup_mask: Option<&Cell<u16>>,
    ) -> Result<(), RuntimeError> {
        let plan = AdmissionController::admit::<MODULES, STARTUP, QUOTAS>(
            manifest,
            startup_nodes,
            profile,
        )?;
        // Preserve `Runtime::admit` error precedence: admission is observable
        // before the `from_plan` threshold and capacity checks.
        thresholds.validate()?;
        Self::validate_capacities(plan.module_count())?;

        let mut guard = RuntimeInitGuard::new(
            destination,
            #[cfg(test)]
            cleanup_mask,
        );

        core::ptr::addr_of_mut!((*destination).plan).write(plan);
        guard.mark(RuntimeInitStage::Plan);
        checkpoint(RuntimeInitStage::Plan)?;

        core::ptr::addr_of_mut!((*destination).mailbox)
            .write(Mailbox::with_control_reserve(usize::from(MAILBOX > 1)));
        guard.mark(RuntimeInitStage::Mailbox);
        checkpoint(RuntimeInitStage::Mailbox)?;

        core::ptr::addr_of_mut!((*destination).alarms).write(AlarmQueue::new());
        guard.mark(RuntimeInitStage::Alarms);
        checkpoint(RuntimeInitStage::Alarms)?;

        core::ptr::addr_of_mut!((*destination).kv).write(KvStore::new());
        guard.mark(RuntimeInitStage::Kv);
        checkpoint(RuntimeInitStage::Kv)?;

        core::ptr::addr_of_mut!((*destination).kv_owners).write([None; KV]);
        guard.mark(RuntimeInitStage::KvOwners);
        checkpoint(RuntimeInitStage::KvOwners)?;

        core::ptr::addr_of_mut!((*destination).recovery)
            .write(RecoveryCoordinator::new(thresholds));
        guard.mark(RuntimeInitStage::Recovery);
        checkpoint(RuntimeInitStage::Recovery)?;

        core::ptr::addr_of_mut!((*destination).watchdog).write(Watchdog::new());
        guard.mark(RuntimeInitStage::Watchdog);
        checkpoint(RuntimeInitStage::Watchdog)?;

        core::ptr::addr_of_mut!((*destination).modules).write(ModuleRuntimeGuard::new());
        guard.mark(RuntimeInitStage::Modules);
        let startup = core::ptr::addr_of!((*destination).plan.startup);
        (&mut *core::ptr::addr_of_mut!((*destination).modules))
            .register_startup_plan(&*startup, 0)?;
        checkpoint(RuntimeInitStage::Modules)?;

        core::ptr::addr_of_mut!((*destination).objects).write(ObjectLedger::new());
        guard.mark(RuntimeInitStage::Objects);
        let objects = &mut *core::ptr::addr_of_mut!((*destination).objects);
        for module in (*startup).order.iter().flatten() {
            objects.register(*module, ObjectQuota::DEFAULT);
        }
        // Manifest-declared object quotas replace the defaults seeded above.
        for spec in manifest.iter() {
            objects.register(spec.id, spec.objects);
        }
        checkpoint(RuntimeInitStage::Objects)?;

        core::ptr::addr_of_mut!((*destination).trace).write(CapabilityTrace::new());
        guard.mark(RuntimeInitStage::Trace);
        checkpoint(RuntimeInitStage::Trace)?;

        core::ptr::addr_of_mut!((*destination).degrade).write(DegradeApplication::none());
        guard.mark(RuntimeInitStage::Degrade);
        checkpoint(RuntimeInitStage::Degrade)?;

        guard.finish();
        Ok(())
    }

    pub fn from_plan(
        plan: AdmissionPlan<STARTUP, QUOTAS>,
        thresholds: FaultThresholds,
    ) -> Result<Self, RuntimeError> {
        thresholds.validate()?;
        Self::validate_capacities(plan.module_count())?;
        let modules = ModuleRuntimeGuard::try_from_startup_plan(&plan.startup)?;
        let mut objects = ObjectLedger::new();
        for module in plan.startup.order.iter().flatten() {
            objects.register(*module, ObjectQuota::DEFAULT);
        }
        Ok(Self {
            plan,
            mailbox: Mailbox::with_control_reserve(usize::from(MAILBOX > 1)),
            alarms: AlarmQueue::new(),
            kv: KvStore::new(),
            kv_owners: [None; KV],
            recovery: RecoveryCoordinator::new(thresholds),
            watchdog: Watchdog::new(),
            modules,
            objects,
            trace: CapabilityTrace::new(),
            degrade: DegradeApplication::none(),
        })
    }

    pub fn admit<const MODULES: usize>(
        manifest: &SystemManifest<MODULES>,
        startup_nodes: &[StartupNode],
        profile: SystemProfile,
        thresholds: FaultThresholds,
    ) -> Result<Self, RuntimeError> {
        let plan = AdmissionController::admit::<MODULES, STARTUP, QUOTAS>(
            manifest,
            startup_nodes,
            profile,
        )?;
        let mut runtime = Self::from_plan(plan, thresholds)?;
        // Manifest-declared object quotas replace the defaults the plan seeded.
        for spec in manifest.iter() {
            runtime.objects.register(spec.id, spec.objects);
        }
        Ok(runtime)
    }

    pub fn admit_graph<const MODULES: usize, const GRAPH: usize>(
        manifest: &SystemManifest<MODULES>,
        startup: &StartupGraph<GRAPH>,
        profile: SystemProfile,
        thresholds: FaultThresholds,
    ) -> Result<Self, RuntimeError> {
        Self::admit(manifest, startup.as_slice(), profile, thresholds)
    }

    pub fn boot_to_running(&mut self, now_us: u64) -> Result<(), RuntimeError> {
        self.recovery
            .transition(SystemState::ValidateManifest, now_us)?;
        self.recovery.transition(SystemState::InitDrivers, now_us)?;
        self.recovery.transition(SystemState::Running, now_us)?;
        self.modules.activate_all(now_us)?;
        Ok(())
    }

    pub fn authorize(&self, module: ModuleId, capability: Capability) -> Result<(), RuntimeError> {
        self.ensure_module_enabled(module)?;
        self.plan.grants.authorize(module, capability)?;
        Ok(())
    }

    /// Module a mailbox slot is charged to: the sender, except that
    /// kernel-origin notifications (alarm/recovery fan-out) are charged to
    /// their destination so a module's own alarm storm cannot hide behind the
    /// exempt kernel identity.
    fn accountable(message: Message) -> ModuleId {
        if message.from == ModuleId::Kernel {
            message.to
        } else {
            message.from
        }
    }

    pub(crate) fn send(&mut self, message: Message) -> Result<(), RuntimeError> {
        self.ensure_message_endpoints_enabled(message)?;
        let accountable = Self::accountable(message);
        self.objects.charge(accountable, ObjectKind::MailboxSlot)?;
        if let Err(error) = self.mailbox.push(message) {
            self.objects.release(accountable, ObjectKind::MailboxSlot)?;
            return Err(RuntimeError::Mailbox(error));
        }
        Ok(())
    }

    pub(crate) fn recv_for(&mut self, module: ModuleId) -> Option<Message> {
        let message = self.mailbox.pop_for(module)?;
        let _ = self
            .objects
            .release(Self::accountable(message), ObjectKind::MailboxSlot);
        Some(message)
    }

    pub(crate) fn schedule_once(
        &mut self,
        id: AlarmId,
        module: ModuleId,
        delay_us: u64,
        now_us: u64,
    ) -> Result<(), RuntimeError> {
        self.ensure_module_enabled(module)?;
        self.objects.charge(module, ObjectKind::Alarm)?;
        if let Err(error) = self.alarms.schedule_once(id, module, delay_us, now_us) {
            self.objects.release(module, ObjectKind::Alarm)?;
            return Err(RuntimeError::Alarm(error));
        }
        Ok(())
    }

    pub(crate) fn schedule_periodic(
        &mut self,
        id: AlarmId,
        module: ModuleId,
        period_us: u32,
        now_us: u64,
    ) -> Result<(), RuntimeError> {
        self.ensure_module_enabled(module)?;
        self.objects.charge(module, ObjectKind::Alarm)?;
        if let Err(error) = self.alarms.schedule_periodic(id, module, period_us, now_us) {
            self.objects.release(module, ObjectKind::Alarm)?;
            return Err(RuntimeError::Alarm(error));
        }
        Ok(())
    }

    pub(crate) fn cancel_alarm(&mut self, id: AlarmId) -> Result<Alarm, RuntimeError> {
        let alarm = self.alarms.cancel(id)?;
        self.objects.release(alarm.module, ObjectKind::Alarm)?;
        Ok(alarm)
    }

    /// Put a cancelled alarm back exactly as it was (undo path for a denied
    /// cross-module cancel), restoring its owner's object charge.
    pub(crate) fn restore_alarm(&mut self, alarm: Alarm) -> Result<(), RuntimeError> {
        self.objects.charge(alarm.module, ObjectKind::Alarm)?;
        if let Err(error) = self.alarms.restore(alarm) {
            self.objects.release(alarm.module, ObjectKind::Alarm)?;
            return Err(RuntimeError::Alarm(error));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn dispatch_due_alarms(&mut self, now_us: u64) -> Result<usize, RuntimeError> {
        let dispatch = self.try_dispatch_due_alarms(now_us);
        match dispatch.error {
            Some(error) => Err(error),
            None => Ok(dispatch.dispatched),
        }
    }

    pub(crate) fn try_dispatch_due_alarms(&mut self, now_us: u64) -> AlarmDispatch {
        let mut dispatched = 0;
        while let Some(alarm) = self.alarms.next_due(now_us) {
            let message = alarm_message(alarm);
            let accountable = Self::accountable(message);
            if self
                .objects
                .charge(accountable, ObjectKind::MailboxSlot)
                .is_err()
            {
                return AlarmDispatch::blocked(
                    dispatched,
                    alarm,
                    RuntimeError::Mailbox(MailboxError::Full),
                );
            }
            if let Err(error) = self.mailbox.push(message) {
                let _ = self.objects.release(accountable, ObjectKind::MailboxSlot);
                return AlarmDispatch::blocked(dispatched, alarm, RuntimeError::Mailbox(error));
            }
            // One-shot alarms leave the queue on pop; periodic alarms reschedule
            // and keep their charge.
            if let Some(fired) = self.alarms.pop_due(now_us) {
                if fired.period_us.is_none() {
                    let _ = self.objects.release(fired.module, ObjectKind::Alarm);
                }
            }
            dispatched += 1;
        }
        AlarmDispatch::completed(dispatched)
    }

    pub(crate) fn dispatch_due_alarms_with_recovery(
        &mut self,
        now_us: u64,
    ) -> Result<AlarmDispatch, RuntimeError> {
        let dispatch = self.try_dispatch_due_alarms(now_us);
        if let Some(alarm) = dispatch.blocked {
            self.record_error(alarm.module, KernelError::DeadlineMissed, now_us)?;
        }
        Ok(dispatch)
    }

    #[cfg(test)]
    pub(crate) fn kv_set(&mut self, key: KvKey, value: KvValue) -> Result<(), RuntimeError> {
        // Trusted-dispatcher path: entries are kernel-owned (exempt from object
        // quotas). Module-owned writes go through `ModuleCtx::kv_set`.
        self.kv.set(key, value)?;
        Ok(())
    }

    pub(crate) fn kv_get(&self, key: KvKey) -> Option<KvValue> {
        self.kv.get(key)
    }

    pub fn kv_owner(&self, key: KvKey) -> Option<ModuleId> {
        self.kv_owners
            .iter()
            .flatten()
            .find(|(owned, _)| *owned == key)
            .map(|(_, owner)| *owner)
    }

    fn take_kv_owner(&mut self, key: KvKey) -> Option<ModuleId> {
        for slot in self.kv_owners.iter_mut() {
            if slot.map(|(owned, _)| owned == key).unwrap_or(false) {
                return slot.take().map(|(_, owner)| owner);
            }
        }
        None
    }

    /// Module-owned KV write: charges the module's KV quota for new keys and
    /// refuses to overwrite a key another module owns.
    pub(crate) fn kv_set_owned(
        &mut self,
        module: ModuleId,
        key: KvKey,
        value: KvValue,
    ) -> Result<(), RuntimeError> {
        match self.kv_owner(key) {
            Some(owner) if owner != module => {
                return Err(RuntimeError::KvOwnedByOther { key, owner });
            }
            Some(_) => {
                self.kv.set(key, value)?;
                return Ok(());
            }
            None => {}
        }
        if self.kv.contains(key) {
            // Kernel-owned key: modules may not overwrite dispatcher state.
            return Err(RuntimeError::KvOwnedByOther {
                key,
                owner: ModuleId::Kernel,
            });
        }
        self.objects.charge(module, ObjectKind::KvEntry)?;
        if let Err(error) = self.kv.set(key, value) {
            self.objects.release(module, ObjectKind::KvEntry)?;
            return Err(RuntimeError::Kv(error));
        }
        if let Some(slot) = self.kv_owners.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some((key, module));
        }
        Ok(())
    }

    /// Module-owned KV delete: only the owner may delete its keys.
    pub(crate) fn kv_delete_owned(
        &mut self,
        module: ModuleId,
        key: KvKey,
    ) -> Result<KvValue, RuntimeError> {
        match self.kv_owner(key) {
            Some(owner) if owner != module => Err(RuntimeError::KvOwnedByOther { key, owner }),
            Some(_) => {
                let value = self.kv.delete(key)?;
                let _ = self.take_kv_owner(key);
                self.objects.release(module, ObjectKind::KvEntry)?;
                Ok(value)
            }
            None => Err(RuntimeError::KvOwnedByOther {
                key,
                owner: ModuleId::Kernel,
            }),
        }
    }

    pub(crate) fn reserve_quota(
        &mut self,
        module: ModuleId,
        amount: SystemBudget,
    ) -> Result<(), RuntimeError> {
        self.ensure_module_enabled(module)?;
        self.plan.quotas.reserve(module, amount)?;
        Ok(())
    }

    pub(crate) fn release_quota(
        &mut self,
        module: ModuleId,
        amount: SystemBudget,
    ) -> Result<(), RuntimeError> {
        self.ensure_module_enabled(module)?;
        self.plan.quotas.release(module, amount)?;
        Ok(())
    }

    pub fn quota_usage(&self, module: ModuleId) -> Option<SystemBudget> {
        self.plan.quotas.usage(module)
    }

    pub fn quota_limit(&self, module: ModuleId) -> Option<SystemBudget> {
        self.plan.quotas.limit(module)
    }

    pub fn quota_available(&self, module: ModuleId) -> Option<SystemBudget> {
        self.plan.quotas.available(module)
    }

    pub fn total_quota_used(&self) -> SystemBudget {
        self.plan.quotas.total_used()
    }

    pub fn record_ok(&mut self, module: ModuleId, now_us: u64) -> Result<(), RuntimeError> {
        self.ensure_module_enabled(module)?;
        self.recovery.record_ok(module, now_us);
        Ok(())
    }

    pub fn record_error(
        &mut self,
        module: ModuleId,
        error: KernelError,
        now_us: u64,
    ) -> Result<RecoveryOutcome, RuntimeError> {
        self.ensure_module_enabled(module)?;
        let outcome = self
            .recovery
            .record_error(module, error, now_us)
            .map_err(RuntimeError::from)?;
        if outcome.coalesced {
            self.modules.note_coalesced_fault(module, now_us)?;
        } else {
            self.modules.note_recovery_outcome(outcome, now_us)?;
        }
        Ok(outcome)
    }

    pub fn record_fault(
        &mut self,
        module: ModuleId,
        fault: HealthFault,
        now_us: u64,
        policy: &mut impl FaultPolicy,
    ) -> Result<RecoveryOutcome, RuntimeError> {
        self.ensure_module_enabled(module)?;
        let outcome = self.recovery.record_fault(module, fault, now_us, policy)?;
        if outcome.coalesced {
            self.modules.note_coalesced_fault(module, now_us)?;
        } else {
            self.modules.note_recovery_outcome(outcome, now_us)?;
        }
        Ok(outcome)
    }

    pub fn record_error_with_plan<const STEPS: usize>(
        &mut self,
        module: ModuleId,
        error: KernelError,
        now_us: u64,
        policy: RecoveryPlanPolicy,
    ) -> Result<RecoveryPlanning<STEPS>, RuntimeError> {
        let outcome = self.record_error(module, error, now_us)?;
        let plan = RecoveryPlan::from_outcome(outcome, now_us, policy)?;
        Ok(RecoveryPlanning { outcome, plan })
    }

    pub fn record_error_with_plan_and_impact<const STEPS: usize, const IMPACT: usize>(
        &mut self,
        module: ModuleId,
        error: KernelError,
        impact: &DependencyImpact<IMPACT>,
        now_us: u64,
        policy: RecoveryPlanPolicy,
    ) -> Result<RecoveryPlanning<STEPS>, RuntimeError> {
        let outcome = self.record_error(module, error, now_us)?;
        let plan = RecoveryPlan::from_outcome_with_impact(outcome, impact, now_us, policy)?;
        Ok(RecoveryPlanning { outcome, plan })
    }

    pub fn record_watchdog_expired(
        &mut self,
        module: ModuleId,
        now_us: u64,
    ) -> Result<RecoveryOutcome, RuntimeError> {
        self.ensure_module_enabled(module)?;
        let outcome = self
            .recovery
            .record_watchdog_expired(module, now_us)
            .map_err(RuntimeError::from)?;
        if outcome.coalesced {
            self.modules.note_coalesced_fault(module, now_us)?;
        } else {
            self.modules.note_recovery_outcome(outcome, now_us)?;
        }
        Ok(outcome)
    }

    pub fn record_watchdog_expired_with_plan<const STEPS: usize>(
        &mut self,
        module: ModuleId,
        now_us: u64,
        policy: RecoveryPlanPolicy,
    ) -> Result<RecoveryPlanning<STEPS>, RuntimeError> {
        let outcome = self.record_watchdog_expired(module, now_us)?;
        let plan = RecoveryPlan::from_outcome(outcome, now_us, policy)?;
        Ok(RecoveryPlanning { outcome, plan })
    }

    pub fn record_watchdog_expired_with_plan_and_impact<const STEPS: usize, const IMPACT: usize>(
        &mut self,
        module: ModuleId,
        impact: &DependencyImpact<IMPACT>,
        now_us: u64,
        policy: RecoveryPlanPolicy,
    ) -> Result<RecoveryPlanning<STEPS>, RuntimeError> {
        let outcome = self.record_watchdog_expired(module, now_us)?;
        let plan = RecoveryPlan::from_outcome_with_impact(outcome, impact, now_us, policy)?;
        Ok(RecoveryPlanning { outcome, plan })
    }

    pub fn register_watchdog(
        &mut self,
        module: ModuleId,
        timeout_us: u64,
        now_us: u64,
    ) -> Result<(), RuntimeError> {
        self.ensure_module_enabled(module)?;
        self.watchdog.register(module, timeout_us, now_us)?;
        Ok(())
    }

    pub fn heartbeat(&mut self, module: ModuleId, now_us: u64) -> Result<(), RuntimeError> {
        self.ensure_module_enabled(module)?;
        self.watchdog.beat(module, now_us)?;
        self.record_ok(module, now_us)?;
        Ok(())
    }

    pub fn sweep_watchdogs(&mut self, now_us: u64) -> Result<WatchdogSweep<HEALTH>, RuntimeError> {
        let mut modules = [ModuleId::Kernel; HEALTH];
        let expired = self.watchdog.expired_edges(now_us, &mut modules);
        let mut sweep = WatchdogSweep::new();

        for module in modules.iter().copied().take(expired) {
            sweep.push(self.record_watchdog_expired(module, now_us)?);
        }

        Ok(sweep)
    }

    fn complete_module_recovery(
        &mut self,
        module: ModuleId,
        now_us: u64,
    ) -> Result<(), RuntimeError> {
        self.ensure_module_enabled(module)?;
        if self.module_state(module) != Some(ModuleRunState::Recovering) {
            return Err(RuntimeError::RecoveryNotActive(module));
        }
        self.record_ok(module, now_us)?;
        if self.modules.count_state(ModuleRunState::Recovering) == 1 {
            if self.modules.count_state(ModuleRunState::Faulted) == 0 {
                self.recovery.transition(SystemState::InitDrivers, now_us)?;
                self.recovery.transition(SystemState::Running, now_us)?;
            } else {
                self.recovery.transition(SystemState::Degraded, now_us)?;
            }
        }
        self.modules.complete_recovery(module, now_us)?;
        Ok(())
    }

    pub fn apply_recovery_step<H: ModuleLifecycleHooks>(
        &mut self,
        step: RecoveryStep,
        now_us: u64,
        hooks: &mut H,
    ) -> Result<(), RuntimeError> {
        self.ensure_module_enabled(step.module)?;
        match step.kind {
            RecoveryStepKind::Observe => Ok(()),
            RecoveryStepKind::Notify => hooks.notify(step.module).map_err(RuntimeError::from),
            RecoveryStepKind::Retry => hooks.retry(step.module).map_err(RuntimeError::from),
            RecoveryStepKind::QuiesceModule => match self.module_state(step.module) {
                Some(ModuleRunState::Suspended) => Ok(()),
                _ => {
                    hooks.quiesce(step.module)?;
                    if self.module_state(step.module) != Some(ModuleRunState::Recovering) {
                        self.modules.suspend(step.module, now_us)?;
                    }
                    Ok(())
                }
            },
            RecoveryStepKind::RestartModule => {
                hooks.stop(step.module)?;
                hooks.start(step.module)?;
                Ok(())
            }
            RecoveryStepKind::VerifyHeartbeat => {
                hooks.self_test(step.module)?;
                hooks.heartbeat(step.module)?;
                self.record_ok(step.module, now_us)
            }
            RecoveryStepKind::ResumeModule => match self.module_state(step.module) {
                Some(ModuleRunState::Active) => Ok(()),
                _ => {
                    hooks.resume(step.module)?;
                    if self.module_state(step.module) == Some(ModuleRunState::Recovering) {
                        self.complete_module_recovery(step.module, now_us)
                    } else {
                        self.modules.resume(step.module, now_us)?;
                        Ok(())
                    }
                }
            },
        }
    }

    pub fn suspend_module(&mut self, module: ModuleId, now_us: u64) -> Result<(), RuntimeError> {
        self.ensure_module_admitted(module)?;
        self.modules.suspend(module, now_us)?;
        Ok(())
    }

    pub fn resume_module(&mut self, module: ModuleId, now_us: u64) -> Result<(), RuntimeError> {
        self.ensure_module_admitted(module)?;
        self.modules.resume(module, now_us)?;
        Ok(())
    }

    pub fn disable_module(&mut self, module: ModuleId, now_us: u64) -> Result<(), RuntimeError> {
        self.ensure_module_admitted(module)?;
        if self.module_state(module) == Some(ModuleRunState::Disabled) {
            self.cleanup_module_resources(module)?;
            return Ok(());
        }
        self.modules.disable(module, now_us)?;
        self.cleanup_module_resources(module)?;
        Ok(())
    }

    pub fn reload_module<const STEPS: usize, L: LeaseReleaser, H: ModuleReloadHooks>(
        &mut self,
        request: ModuleReloadRequest,
        leases: &mut L,
        hooks: &mut H,
    ) -> Result<HotReloadOutcome<STEPS>, RuntimeError> {
        let ModuleReloadRequest {
            module,
            lease_owner,
            new_revision,
            now_us,
            policy,
        } = request;
        self.ensure_module_enabled(module)?;
        let plan = HotReloadPlan::build(module, new_revision, now_us, policy)?;

        hooks.quiesce(module)?;
        self.modules.suspend(module, now_us)?;

        let released_leases = leases.release_all_for_owner(lease_owner);
        let released_quota = self.cleanup_module_resources(module)?;
        hooks.unmount(module)?;
        hooks.mount(module, new_revision)?;
        hooks.self_test(module)?;
        hooks.heartbeat(module)?;
        hooks.resume(module)?;
        self.modules.resume(module, plan.deadline_us)?;

        Ok(HotReloadOutcome {
            module,
            lease_owner,
            released_leases,
            released_quota,
            new_revision,
            plan,
        })
    }

    pub fn apply_degrade_decision<const N: usize>(
        &mut self,
        decision: &DegradeDecision<N>,
        now_us: u64,
    ) -> Result<DegradeApplication, RuntimeError> {
        for module in decision
            .disabled
            .iter()
            .copied()
            .take(decision.disabled_count)
            .flatten()
        {
            self.ensure_module_admitted(module)?;
        }

        let mut application = DegradeApplication {
            requested: 0,
            disabled: 0,
            already_disabled: 0,
            reason: decision.reason,
            applied_at_us: now_us,
        };

        for module in decision
            .disabled
            .iter()
            .copied()
            .take(decision.disabled_count)
        {
            let Some(module) = module else {
                continue;
            };
            application.requested += 1;
            if self.module_state(module) == Some(ModuleRunState::Disabled) {
                application.already_disabled += 1;
                continue;
            }
            self.modules.disable(module, now_us)?;
            self.cleanup_module_resources(module)?;
            application.disabled += 1;
        }

        if application.disabled > 0 && self.state() == SystemState::Running {
            self.recovery.transition(SystemState::Degraded, now_us)?;
        }

        self.degrade = application;
        Ok(application)
    }

    pub fn module_state(&self, module: ModuleId) -> Option<ModuleRunState> {
        self.modules.state(module)
    }

    pub fn module_runtime_entry(&self, module: ModuleId) -> Option<ModuleRuntimeEntry> {
        self.modules.entry(module)
    }

    pub fn health_report(&self, module: ModuleId) -> Option<HealthReport> {
        let snapshot = self.recovery.snapshot(module)?;
        Some(HealthReport::from_snapshot(
            snapshot,
            self.recovery
                .events()
                .count_at_or_above(EventSeverity::Error) as u32,
            self.recovery
                .events()
                .count_at_or_above(EventSeverity::Fatal) as u32,
        ))
    }

    pub fn runtime_report(&self) -> RuntimeReport {
        RuntimeReport::from_input(RuntimeReportInput {
            state: self.state(),
            module_count: self.plan.module_count() as u32,
            mailbox_len: self.mailbox.len() as u32,
            mailbox_dropped: self.mailbox.dropped(),
            alarm_len: self.alarms.len() as u32,
            next_alarm_due_us: self.alarms.next_due_us().unwrap_or(0),
            kv_len: self.kv.len() as u32,
            kv_writes: self.kv.writes(),
            kv_deletes: self.kv.deletes(),
            quota_used: self.total_quota_used(),
            event_count: self.recovery.events().len() as u32,
            dropped_events: self.recovery.events().dropped(),
        })
    }

    pub fn event_log_report(&self) -> EventLogReport {
        EventLogReport::from_event_log(self.recovery.events())
    }

    pub fn module_runtime_report(&self) -> ModuleRuntimeReport {
        ModuleRuntimeReport::from_guard(&self.modules)
    }

    pub fn degrade_application_report(&self) -> DegradeApplicationReport {
        DegradeApplicationReport::from_application(self.degrade)
    }

    pub const fn state(&self) -> SystemState {
        self.recovery.state()
    }

    pub const fn plan(&self) -> &AdmissionPlan<STARTUP, QUOTAS> {
        &self.plan
    }

    pub const fn mailbox(&self) -> &Mailbox<MAILBOX> {
        &self.mailbox
    }

    pub const fn alarms(&self) -> &AlarmQueue<ALARMS> {
        &self.alarms
    }

    pub const fn kv(&self) -> &KvStore<KV> {
        &self.kv
    }

    pub const fn recovery(&self) -> &RecoveryCoordinator<HEALTH, LOG> {
        &self.recovery
    }

    pub const fn watchdog(&self) -> &Watchdog<HEALTH> {
        &self.watchdog
    }

    pub fn watchdog_entry(&self, module: ModuleId) -> Option<WatchdogEntry> {
        self.watchdog.get(module)
    }

    pub fn watchdog_expired_count(&self, now_us: u64) -> usize {
        self.watchdog.expired_count(now_us)
    }

    fn ensure_module_admitted(&self, module: ModuleId) -> Result<(), RuntimeError> {
        if self.modules.entry(module).is_some() {
            Ok(())
        } else {
            Err(RuntimeError::Module(ModuleRuntimeError::Missing(module)))
        }
    }

    pub(crate) fn ensure_module_enabled(&self, module: ModuleId) -> Result<(), RuntimeError> {
        let Some(entry) = self.modules.entry(module) else {
            return Err(RuntimeError::Module(ModuleRuntimeError::Missing(module)));
        };
        if entry.state == ModuleRunState::Disabled {
            Err(RuntimeError::Module(ModuleRuntimeError::Disabled(module)))
        } else {
            Ok(())
        }
    }

    fn ensure_message_endpoints_enabled(&self, message: Message) -> Result<(), RuntimeError> {
        self.ensure_module_enabled(message.from)?;
        self.ensure_module_enabled(message.to)?;
        Ok(())
    }

    fn cleanup_module_resources(&mut self, module: ModuleId) -> Result<SystemBudget, RuntimeError> {
        let removed_alarms = self.alarms.remove_for(module);
        for _ in 0..removed_alarms {
            self.objects.release(module, ObjectKind::Alarm)?;
        }

        let objects = &mut self.objects;
        self.mailbox.remove_for_with(module, |message| {
            let _ = objects.release(Self::accountable(message), ObjectKind::MailboxSlot);
        });

        for idx in 0..KV {
            let Some((key, owner)) = self.kv_owners[idx] else {
                continue;
            };
            if owner != module {
                continue;
            }
            self.kv_owners[idx] = None;
            let _ = self.kv.delete(key);
            self.objects.release(module, ObjectKind::KvEntry)?;
        }

        self.watchdog.remove(module);
        let released = self.plan.quotas.reset_usage(module)?;
        // Post-cleanup invariant: the module must hold no kernel objects.
        // A residual charge means an accounting path missed a release.
        self.objects.verify_clear(module)?;
        Ok(released)
    }

    pub fn object_usage(&self, module: ModuleId) -> Option<ObjectUsage> {
        self.objects.usage(module)
    }

    pub fn object_quota(&self, module: ModuleId) -> Option<ObjectQuota> {
        self.objects.quota(module)
    }

    /// Accumulate measured execution time for a module (executor-fed).
    pub fn charge_cpu(&mut self, module: ModuleId, duration_us: u32) {
        self.objects.charge_cpu(module, duration_us);
    }

    pub const fn capability_trace(&self) -> &CapabilityTrace<LOG> {
        &self.trace
    }

    pub fn copy_capability_trace(
        &self,
        scope: CapabilityReplayScope,
        out: &mut [CapabilityTraceRecord],
    ) -> usize {
        self.trace.copy_replay(scope, out)
    }

    pub(crate) fn trace_record(&mut self, input: CapabilityTraceInput) {
        let _ = self.trace.record(input);
    }

    pub(crate) fn authorize_traced(
        &mut self,
        module: ModuleId,
        capability: Capability,
        at_us: u64,
    ) -> Result<(), RuntimeError> {
        if let Err(error) = self.plan.grants.authorize(module, capability) {
            self.trace_record(CapabilityTraceInput::new(
                module,
                capability,
                CapabilityTraceOp::Fault,
                at_us,
            ));
            return Err(RuntimeError::Capability(error));
        }
        Ok(())
    }
}

fn alarm_message(alarm: Alarm) -> Message {
    Message::new(
        ModuleId::Kernel,
        alarm.module,
        MessageKind::Notification,
        u32::from(alarm.id.0),
        alarm.due_us as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        kernel_module_spec, Action, CapabilitySet, Criticality, DeadlineContract, DependencySet,
        FaultThresholds, HotReloadPolicy, MemoryBudget, ModuleSpec, RecoveryPlanError,
        RecoveryPlanPolicy, RecoveryStep, RecoveryStepKind,
    };

    type TestRuntime = Runtime<4, 4, 4, 4, 4, 4, 16>;

    struct FakeLeases {
        owner: u8,
        released: usize,
        calls: usize,
    }

    impl LeaseReleaser for FakeLeases {
        fn release_all_for_owner(&mut self, owner: u8) -> usize {
            self.calls += 1;
            if owner == self.owner {
                self.released
            } else {
                0
            }
        }
    }

    #[derive(Default)]
    struct FakeHooks {
        calls: usize,
        mounted_revision: Option<u32>,
        fail: Option<ModuleHookError>,
    }

    impl FakeHooks {
        fn called(&mut self, operation: ModuleHookError) -> Result<(), ModuleHookError> {
            self.calls += 1;
            if self.fail == Some(operation) {
                Err(operation)
            } else {
                Ok(())
            }
        }
    }

    impl ModuleLifecycleHooks for FakeHooks {
        fn notify(&mut self, _module: ModuleId) -> Result<(), ModuleHookError> {
            self.called(ModuleHookError::Notify)
        }
        fn retry(&mut self, _module: ModuleId) -> Result<(), ModuleHookError> {
            self.called(ModuleHookError::Retry)
        }
        fn quiesce(&mut self, _module: ModuleId) -> Result<(), ModuleHookError> {
            self.called(ModuleHookError::Quiesce)
        }
        fn stop(&mut self, _module: ModuleId) -> Result<(), ModuleHookError> {
            self.called(ModuleHookError::Stop)
        }
        fn start(&mut self, _module: ModuleId) -> Result<(), ModuleHookError> {
            self.called(ModuleHookError::Start)
        }
        fn self_test(&mut self, _module: ModuleId) -> Result<(), ModuleHookError> {
            self.called(ModuleHookError::SelfTest)
        }
        fn heartbeat(&mut self, _module: ModuleId) -> Result<(), ModuleHookError> {
            self.called(ModuleHookError::Heartbeat)
        }
        fn resume(&mut self, _module: ModuleId) -> Result<(), ModuleHookError> {
            self.called(ModuleHookError::Resume)
        }
    }

    impl ModuleReloadHooks for FakeHooks {
        fn unmount(&mut self, _module: ModuleId) -> Result<(), ModuleHookError> {
            self.called(ModuleHookError::Unmount)
        }

        fn mount(&mut self, _module: ModuleId, revision: u32) -> Result<(), ModuleHookError> {
            self.called(ModuleHookError::Mount)?;
            self.mounted_revision = Some(revision);
            Ok(())
        }
    }

    fn profile() -> SystemProfile {
        SystemProfile {
            flash_limit_bytes: 80 * 1024,
            ram_limit_bytes: 32 * 1024,
            pool_slot_limit: 8,
            max_modules: 4,
        }
    }

    fn manifest() -> SystemManifest<4> {
        SystemManifest::from_specs(&[
            kernel_module_spec(
                MemoryBudget::new(16 * 1024, 4 * 1024, 4),
                DeadlineContract::new(20_000, 10),
            ),
            ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
                .requires(
                    CapabilitySet::empty()
                        .with(Capability::Bus0)
                        .with(Capability::SamplePool),
                )
                .owns(CapabilitySet::empty().with(Capability::Bus0))
                .memory(MemoryBudget::new(8 * 1024, 2 * 1024, 2)),
        ])
        .unwrap()
    }

    fn startup() -> [StartupNode; 2] {
        [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty()),
            StartupNode::new(ModuleId::Sensor, DependencySet::empty().with_index(0)),
        ]
    }

    fn runtime() -> TestRuntime {
        let manifest = manifest();
        Runtime::admit(
            &manifest,
            &startup(),
            profile(),
            FaultThresholds {
                notify_after: 1,
                reboot_after: 3,
            },
        )
        .unwrap()
    }

    #[test]
    fn in_place_runtime_cleans_exactly_initialized_fields_on_every_error_stage() {
        let manifest = manifest();
        let startup = startup();
        for (index, fail_stage) in RuntimeInitStage::ALL.into_iter().enumerate() {
            let mut slot = core::mem::MaybeUninit::<TestRuntime>::uninit();
            let cleanup = Cell::new(0u16);
            let error = unsafe {
                TestRuntime::admit_in_place_inner(
                    slot.as_mut_ptr(),
                    &manifest,
                    &startup,
                    profile(),
                    FaultThresholds {
                        notify_after: 1,
                        reboot_after: 3,
                    },
                    &mut |stage| {
                        if stage == fail_stage {
                            Err(RuntimeError::PoolExhausted)
                        } else {
                            Ok(())
                        }
                    },
                    Some(&cleanup),
                )
            }
            .unwrap_err();
            assert_eq!(error, RuntimeError::PoolExhausted);
            assert_eq!(cleanup.get(), (1u16 << (index + 1)) - 1);
        }
    }

    #[test]
    fn in_place_runtime_guard_runs_at_every_unwinding_stage() {
        let manifest = manifest();
        let startup = startup();
        for (index, panic_stage) in RuntimeInitStage::ALL.into_iter().enumerate() {
            let mut slot = core::mem::MaybeUninit::<TestRuntime>::uninit();
            let cleanup = Cell::new(0u16);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                let _ = TestRuntime::admit_in_place_inner(
                    slot.as_mut_ptr(),
                    &manifest,
                    &startup,
                    profile(),
                    FaultThresholds {
                        notify_after: 1,
                        reboot_after: 3,
                    },
                    &mut |stage| {
                        assert_ne!(stage, panic_stage, "injected init panic");
                        Ok(())
                    },
                    Some(&cleanup),
                );
            }));
            assert!(result.is_err());
            assert_eq!(cleanup.get(), (1u16 << (index + 1)) - 1);
        }
    }

    #[test]
    fn in_place_runtime_is_exposed_only_after_complete_initialization() {
        let manifest = manifest();
        let startup = startup();
        let mut slot = core::mem::MaybeUninit::<TestRuntime>::uninit();
        unsafe {
            TestRuntime::admit_in_place(
                slot.as_mut_ptr(),
                &manifest,
                &startup,
                profile(),
                FaultThresholds {
                    notify_after: 1,
                    reboot_after: 3,
                },
            )
            .unwrap();
            let runtime = slot.assume_init_mut();
            assert_eq!(runtime.plan().module_count(), 2);
            runtime.boot_to_running(10).unwrap();
            assert_eq!(runtime.state(), SystemState::Running);
            slot.assume_init_drop();
        }
    }

    #[test]
    fn in_place_runtime_preserves_admit_error_precedence() {
        type CapacityConstrainedRuntime = Runtime<4, 4, 4, 4, 4, 1, 16>;

        let manifest = manifest();
        let valid_startup = startup();
        let missing_sensor_startup = [valid_startup[0]];
        let invalid_thresholds = FaultThresholds {
            notify_after: 3,
            reboot_after: 1,
        };
        let valid_thresholds = FaultThresholds {
            notify_after: 1,
            reboot_after: 3,
        };

        // When every layer is invalid, admission remains the first visible
        // error, exactly as in the established by-value constructor.
        let by_value = CapacityConstrainedRuntime::admit(
            &manifest,
            &missing_sensor_startup,
            profile(),
            invalid_thresholds,
        )
        .err();
        let mut slot = core::mem::MaybeUninit::<CapacityConstrainedRuntime>::uninit();
        let in_place = unsafe {
            CapacityConstrainedRuntime::admit_in_place(
                slot.as_mut_ptr(),
                &manifest,
                &missing_sensor_startup,
                profile(),
                invalid_thresholds,
            )
        }
        .err();
        assert_eq!(in_place, by_value);
        assert_eq!(
            in_place,
            Some(RuntimeError::Admission(AdmissionError::MissingStartupNode(
                ModuleId::Sensor
            )))
        );

        // With admission fixed, threshold validation precedes capacity
        // validation in both paths.
        let by_value = CapacityConstrainedRuntime::admit(
            &manifest,
            &valid_startup,
            profile(),
            invalid_thresholds,
        )
        .err();
        let mut slot = core::mem::MaybeUninit::<CapacityConstrainedRuntime>::uninit();
        let in_place = unsafe {
            CapacityConstrainedRuntime::admit_in_place(
                slot.as_mut_ptr(),
                &manifest,
                &valid_startup,
                profile(),
                invalid_thresholds,
            )
        }
        .err();
        assert_eq!(in_place, by_value);
        assert_eq!(
            in_place,
            Some(RuntimeError::FaultThreshold(
                FaultThresholdError::RebootBeforeNotify
            ))
        );

        // With admission and thresholds valid, the constrained health table
        // reaches the same capacity error in both paths.
        let by_value = CapacityConstrainedRuntime::admit(
            &manifest,
            &valid_startup,
            profile(),
            valid_thresholds,
        )
        .err();
        let mut slot = core::mem::MaybeUninit::<CapacityConstrainedRuntime>::uninit();
        let in_place = unsafe {
            CapacityConstrainedRuntime::admit_in_place(
                slot.as_mut_ptr(),
                &manifest,
                &valid_startup,
                profile(),
                valid_thresholds,
            )
        }
        .err();
        assert_eq!(in_place, by_value);
        assert!(matches!(
            in_place,
            Some(RuntimeError::Capacity(
                CapacityError::ModuleTablesTooSmall { modules: 2, .. }
            ))
        ));
    }

    #[test]
    fn runtime_can_be_admitted_from_startup_graph() {
        let manifest = manifest();
        let mut graph = manifest.startup_graph::<4>().unwrap();
        graph
            .add_dependency(ModuleId::Sensor, ModuleId::Kernel)
            .unwrap();

        let runtime = TestRuntime::admit_graph(
            &manifest,
            &graph,
            profile(),
            FaultThresholds {
                notify_after: 1,
                reboot_after: 3,
            },
        )
        .unwrap();

        assert_eq!(runtime.plan().module_count(), 2);
        assert!(runtime
            .authorize(ModuleId::Sensor, Capability::SamplePool)
            .is_ok());
    }

    #[test]
    fn runtime_boots_to_running_from_admitted_plan() {
        let mut runtime = runtime();

        runtime.boot_to_running(10).unwrap();

        assert_eq!(runtime.state(), SystemState::Running);
        assert_eq!(runtime.plan().module_count(), 2);
        assert!(runtime
            .authorize(ModuleId::Sensor, Capability::SamplePool)
            .is_ok());
    }

    #[test]
    fn runtime_rejects_invalid_global_fault_thresholds() {
        let manifest = manifest();
        assert_eq!(
            TestRuntime::admit(
                &manifest,
                &startup(),
                profile(),
                FaultThresholds {
                    notify_after: 4,
                    reboot_after: 3,
                },
            )
            .map(|_| ()),
            Err(RuntimeError::FaultThreshold(
                FaultThresholdError::RebootBeforeNotify
            ))
        );
    }

    #[test]
    fn runtime_dispatches_due_alarm_as_notification() {
        let mut runtime = runtime();
        runtime
            .schedule_once(AlarmId(7), ModuleId::Sensor, 50, 100)
            .unwrap();

        assert_eq!(runtime.dispatch_due_alarms(149).unwrap(), 0);
        assert_eq!(runtime.dispatch_due_alarms(150).unwrap(), 1);

        assert_eq!(
            runtime.recv_for(ModuleId::Sensor),
            Some(Message::new(
                ModuleId::Kernel,
                ModuleId::Sensor,
                MessageKind::Notification,
                7,
                150
            ))
        );
    }

    #[test]
    fn runtime_alarm_dispatch_reports_blocked_alarm() {
        type SmallMailboxRuntime = Runtime<4, 4, 1, 4, 4, 4, 16>;
        let manifest = manifest();
        let mut runtime = SmallMailboxRuntime::admit(
            &manifest,
            &startup(),
            profile(),
            FaultThresholds {
                notify_after: 1,
                reboot_after: 3,
            },
        )
        .unwrap();
        runtime
            .schedule_once(AlarmId(1), ModuleId::Sensor, 10, 0)
            .unwrap();
        runtime
            .schedule_once(AlarmId(2), ModuleId::Kernel, 20, 0)
            .unwrap();

        let dispatch = runtime.try_dispatch_due_alarms(20);

        assert_eq!(
            dispatch,
            AlarmDispatch::blocked(
                1,
                Alarm::once(AlarmId(2), ModuleId::Kernel, 20),
                RuntimeError::Mailbox(MailboxError::Full)
            )
        );
        assert!(dispatch.is_blocked());
        assert_eq!(
            runtime.recv_for(ModuleId::Sensor),
            Some(Message::new(
                ModuleId::Kernel,
                ModuleId::Sensor,
                MessageKind::Notification,
                1,
                10
            ))
        );
        assert_eq!(
            runtime.alarms().next_due(20),
            Some(Alarm::once(AlarmId(2), ModuleId::Kernel, 20))
        );
        assert_eq!(runtime.mailbox().dropped(), 1);
    }

    #[test]
    fn runtime_alarm_dispatch_backpressure_enters_recovery() {
        type SmallMailboxRuntime = Runtime<4, 4, 1, 4, 4, 4, 16>;
        let manifest = manifest();
        let mut runtime = SmallMailboxRuntime::admit(
            &manifest,
            &startup(),
            profile(),
            FaultThresholds {
                notify_after: 1,
                reboot_after: 3,
            },
        )
        .unwrap();
        runtime.boot_to_running(1).unwrap();
        runtime
            .schedule_once(AlarmId(1), ModuleId::Kernel, 10, 0)
            .unwrap();
        runtime
            .schedule_once(AlarmId(2), ModuleId::Sensor, 20, 0)
            .unwrap();

        let dispatch = runtime.dispatch_due_alarms_with_recovery(20).unwrap();
        let report = runtime
            .health_report(ModuleId::Sensor)
            .expect("health report");

        assert_eq!(
            dispatch.blocked,
            Some(Alarm::once(AlarmId(2), ModuleId::Sensor, 20))
        );
        assert_eq!(
            dispatch.error,
            Some(RuntimeError::Mailbox(MailboxError::Full))
        );
        assert_eq!(runtime.state(), SystemState::Degraded);
        assert_eq!(
            runtime.alarms().next_due(20),
            Some(Alarm::once(AlarmId(2), ModuleId::Sensor, 20))
        );
        assert!(report.verify_checksum());
        assert_eq!(
            report.last_error,
            crate::error_code(KernelError::DeadlineMissed)
        );
        assert_eq!(report.total_errors, 1);
    }

    #[test]
    fn runtime_keeps_due_alarm_when_mailbox_is_full() {
        let mut runtime = runtime();
        runtime
            .schedule_once(AlarmId(1), ModuleId::Sensor, 10, 0)
            .unwrap();
        runtime
            .send(Message::new(
                ModuleId::Kernel,
                ModuleId::Sensor,
                MessageKind::Command,
                0,
                0,
            ))
            .unwrap();
        runtime
            .send(Message::new(
                ModuleId::Kernel,
                ModuleId::Sensor,
                MessageKind::Command,
                1,
                0,
            ))
            .unwrap();
        runtime
            .send(Message::new(
                ModuleId::Kernel,
                ModuleId::Sensor,
                MessageKind::Command,
                2,
                0,
            ))
            .unwrap();
        assert_eq!(
            runtime.dispatch_due_alarms(10),
            Err(RuntimeError::Mailbox(MailboxError::Full))
        );
        assert_eq!(
            runtime.alarms().next_due(10),
            Some(Alarm::once(AlarmId(1), ModuleId::Sensor, 10))
        );
    }

    #[test]
    fn runtime_rejects_unknown_message_endpoints_before_mailbox_push() {
        let mut runtime = runtime();

        assert_eq!(
            runtime.send(Message::new(
                ModuleId::Radio,
                ModuleId::Sensor,
                MessageKind::Command,
                1,
                0,
            )),
            Err(RuntimeError::Module(ModuleRuntimeError::Missing(
                ModuleId::Radio
            )))
        );
        assert_eq!(
            runtime.send(Message::new(
                ModuleId::Kernel,
                ModuleId::Radio,
                MessageKind::Command,
                2,
                0,
            )),
            Err(RuntimeError::Module(ModuleRuntimeError::Missing(
                ModuleId::Radio
            )))
        );
        assert_eq!(runtime.mailbox().len(), 0);
        assert_eq!(runtime.mailbox().dropped(), 0);
    }

    #[test]
    fn runtime_wraps_kv_and_health_report() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();
        runtime.kv_set(KvKey(3), KvValue::U32(9)).unwrap();

        let outcome = runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 20)
            .unwrap();
        let report = runtime
            .health_report(ModuleId::Sensor)
            .expect("health report");

        assert_eq!(runtime.kv_get(KvKey(3)), Some(KvValue::U32(9)));
        assert_eq!(outcome.state, SystemState::Degraded);
        assert!(report.verify_checksum());
        assert_eq!(report.total_errors, 1);
        assert_eq!(report.error_events, 2);
    }

    #[test]
    fn runtime_report_summarizes_control_plane_state() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();
        runtime
            .send(Message::new(
                ModuleId::Kernel,
                ModuleId::Sensor,
                MessageKind::Command,
                1,
                0,
            ))
            .unwrap();
        runtime
            .schedule_once(AlarmId(9), ModuleId::Sensor, 500, 100)
            .unwrap();
        runtime.kv_set(KvKey(2), KvValue::Bool(true)).unwrap();
        runtime
            .reserve_quota(ModuleId::Sensor, SystemBudget::new(128, 64, 1))
            .unwrap();

        let report = runtime.runtime_report();

        assert!(report.verify_checksum());
        assert_eq!(report.state, crate::state_code(SystemState::Running));
        assert_eq!(report.module_count, 2);
        assert_eq!(report.mailbox_len, 1);
        assert_eq!(report.alarm_len, 1);
        assert_eq!(report.next_alarm_due_us(), 600);
        assert_eq!(report.kv_len, 1);
        assert_eq!(report.kv_writes, 1);
        assert_eq!(report.quota_flash_used_bytes, 128);
        assert_eq!(report.event_count, 3);
    }

    #[test]
    fn runtime_event_log_report_exposes_latest_event() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();
        runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 20)
            .unwrap();

        let report = runtime.event_log_report();

        assert!(report.verify_checksum());
        assert_eq!(report.event_count, runtime.recovery().events().len() as u32);
        assert_eq!(
            report.latest_module_tag,
            crate::module_tag(ModuleId::Kernel)
        );
        assert_eq!(
            report.latest_kind,
            crate::event_kind_code(crate::EventKind::Host)
        );
        assert_eq!(report.latest_payload_kind, 4);
        assert_eq!(report.latest_payload0, SystemState::Running as u32);
        assert_eq!(report.latest_payload1, SystemState::Degraded as u32);
    }

    #[test]
    fn runtime_tracks_module_state_from_boot_to_fault() {
        let mut runtime = runtime();

        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Registered)
        );
        runtime.boot_to_running(10).unwrap();
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Active)
        );

        runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 20)
            .unwrap();

        let entry = runtime.module_runtime_entry(ModuleId::Sensor).unwrap();
        assert_eq!(entry.state, ModuleRunState::Faulted);
        assert_eq!(entry.fault_count, 1);
        assert_eq!(entry.recovery_count, 0);
    }

    #[test]
    fn runtime_exports_module_runtime_report() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();
        runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 20)
            .unwrap();

        let report = runtime.module_runtime_report();

        assert!(report.verify_checksum());
        assert_eq!(report.module_count, runtime.plan().module_count() as u32);
        assert_eq!(report.faulted_count, 1);
        assert_eq!(
            report.latest_module_tag,
            crate::module_tag(ModuleId::Sensor)
        );
        assert_eq!(
            report.latest_state,
            crate::module_run_state_code(ModuleRunState::Faulted)
        );
    }

    #[test]
    fn runtime_marks_reboot_recovery_and_completion() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();

        runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 20)
            .unwrap();
        runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 30)
            .unwrap();
        runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 40)
            .unwrap();

        let entry = runtime.module_runtime_entry(ModuleId::Sensor).unwrap();
        assert_eq!(entry.state, ModuleRunState::Recovering);
        assert_eq!(entry.fault_count, 3);
        assert_eq!(entry.recovery_count, 1);

        runtime
            .complete_module_recovery(ModuleId::Sensor, 50)
            .unwrap();
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Active)
        );
    }

    #[test]
    fn runtime_exposes_manual_module_suspend_resume_disable() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();

        runtime.suspend_module(ModuleId::Sensor, 20).unwrap();
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Suspended)
        );
        runtime.resume_module(ModuleId::Sensor, 30).unwrap();
        runtime.disable_module(ModuleId::Sensor, 40).unwrap();
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Disabled)
        );
    }

    #[test]
    fn runtime_purges_mailbox_when_module_is_disabled() {
        let mut runtime = runtime();
        runtime
            .send(Message::new(
                ModuleId::Kernel,
                ModuleId::Sensor,
                MessageKind::Command,
                1,
                0,
            ))
            .unwrap();
        runtime
            .send(Message::new(
                ModuleId::Sensor,
                ModuleId::Kernel,
                MessageKind::Notification,
                2,
                0,
            ))
            .unwrap();

        runtime.disable_module(ModuleId::Sensor, 10).unwrap();

        assert_eq!(runtime.mailbox().len(), 0);
        assert_eq!(runtime.recv_for(ModuleId::Sensor), None);
        assert_eq!(runtime.recv_for(ModuleId::Kernel), None);
    }

    #[test]
    fn runtime_purges_alarms_when_module_is_disabled() {
        let mut runtime = runtime();
        runtime
            .schedule_once(AlarmId(1), ModuleId::Sensor, 10, 0)
            .unwrap();
        runtime
            .schedule_periodic(AlarmId(2), ModuleId::Sensor, 20, 0)
            .unwrap();
        runtime
            .schedule_once(AlarmId(3), ModuleId::Kernel, 30, 0)
            .unwrap();

        runtime.disable_module(ModuleId::Sensor, 5).unwrap();

        assert_eq!(runtime.alarms().len(), 1);
        assert_eq!(runtime.dispatch_due_alarms(30).unwrap(), 1);
        assert_eq!(
            runtime.recv_for(ModuleId::Kernel),
            Some(Message::new(
                ModuleId::Kernel,
                ModuleId::Kernel,
                MessageKind::Notification,
                3,
                30
            ))
        );
        assert_eq!(runtime.recv_for(ModuleId::Sensor), None);
    }

    #[test]
    fn runtime_releases_watchdog_and_quota_when_module_is_disabled() {
        let mut runtime = runtime();
        runtime.register_watchdog(ModuleId::Sensor, 100, 0).unwrap();
        runtime
            .reserve_quota(ModuleId::Sensor, SystemBudget::new(128, 64, 1))
            .unwrap();

        runtime.disable_module(ModuleId::Sensor, 10).unwrap();

        assert_eq!(runtime.watchdog_entry(ModuleId::Sensor), None);
        assert_eq!(
            runtime.quota_usage(ModuleId::Sensor),
            Some(SystemBudget::ZERO)
        );
        assert_eq!(runtime.total_quota_used(), SystemBudget::ZERO);
        assert!(runtime.sweep_watchdogs(200).unwrap().is_empty());
    }

    #[test]
    fn runtime_manual_disable_is_idempotent_cleanup() {
        let mut runtime = runtime();

        runtime.disable_module(ModuleId::Sensor, 10).unwrap();
        runtime.disable_module(ModuleId::Sensor, 20).unwrap();

        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Disabled)
        );
        assert_eq!(runtime.mailbox().len(), 0);
        assert_eq!(runtime.alarms().len(), 0);
        assert_eq!(runtime.watchdog_entry(ModuleId::Sensor), None);
        assert_eq!(
            runtime.quota_usage(ModuleId::Sensor),
            Some(SystemBudget::ZERO)
        );
    }

    #[test]
    fn runtime_rejects_disabled_modules_for_active_operations() {
        let mut runtime = runtime();
        runtime.disable_module(ModuleId::Sensor, 10).unwrap();

        assert_eq!(
            runtime.schedule_once(AlarmId(8), ModuleId::Sensor, 10, 10),
            Err(RuntimeError::Module(ModuleRuntimeError::Disabled(
                ModuleId::Sensor
            )))
        );
        assert_eq!(
            runtime.send(Message::new(
                ModuleId::Kernel,
                ModuleId::Sensor,
                MessageKind::Command,
                1,
                0,
            )),
            Err(RuntimeError::Module(ModuleRuntimeError::Disabled(
                ModuleId::Sensor
            )))
        );
        assert_eq!(
            runtime.send(Message::new(
                ModuleId::Sensor,
                ModuleId::Kernel,
                MessageKind::Command,
                2,
                0,
            )),
            Err(RuntimeError::Module(ModuleRuntimeError::Disabled(
                ModuleId::Sensor
            )))
        );
        assert_eq!(
            runtime.record_ok(ModuleId::Sensor, 20),
            Err(RuntimeError::Module(ModuleRuntimeError::Disabled(
                ModuleId::Sensor
            )))
        );
        assert_eq!(
            runtime.register_watchdog(ModuleId::Sensor, 100, 20),
            Err(RuntimeError::Module(ModuleRuntimeError::Disabled(
                ModuleId::Sensor
            )))
        );
        assert_eq!(
            runtime.authorize(ModuleId::Sensor, Capability::SamplePool),
            Err(RuntimeError::Module(ModuleRuntimeError::Disabled(
                ModuleId::Sensor
            )))
        );
        assert_eq!(
            runtime.apply_recovery_step(
                RecoveryStep::new(ModuleId::Sensor, RecoveryStepKind::ResumeModule, 20, 500),
                20,
                &mut FakeHooks::default(),
            ),
            Err(RuntimeError::Module(ModuleRuntimeError::Disabled(
                ModuleId::Sensor
            )))
        );
        assert_eq!(runtime.mailbox().len(), 0);
        assert_eq!(runtime.alarms().len(), 0);
    }

    #[test]
    fn runtime_hot_reload_quiesces_cleans_and_resumes_module() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();
        runtime
            .send(Message::new(
                ModuleId::Kernel,
                ModuleId::Sensor,
                MessageKind::Command,
                1,
                0,
            ))
            .unwrap();
        runtime
            .schedule_once(AlarmId(8), ModuleId::Sensor, 100, 10)
            .unwrap();
        runtime
            .register_watchdog(ModuleId::Sensor, 1_000, 10)
            .unwrap();
        runtime
            .reserve_quota(ModuleId::Sensor, SystemBudget::new(256, 128, 1))
            .unwrap();
        let mut leases = FakeLeases {
            owner: 7,
            released: 2,
            calls: 0,
        };
        let mut hooks = FakeHooks::default();

        let outcome = runtime
            .reload_module::<5, _, _>(
                ModuleReloadRequest::new(ModuleId::Sensor, 7, 3, 20, HotReloadPolicy::DEFAULT),
                &mut leases,
                &mut hooks,
            )
            .unwrap();

        assert_eq!(outcome.module, ModuleId::Sensor);
        assert_eq!(outcome.released_leases, 2);
        assert_eq!(outcome.released_quota, SystemBudget::new(256, 128, 1));
        assert_eq!(outcome.plan.len, 5);
        assert_eq!(outcome.plan.new_revision, 3);
        assert_eq!(leases.calls, 1);
        assert_eq!(hooks.calls, 6);
        assert_eq!(hooks.mounted_revision, Some(3));
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Active)
        );
        assert_eq!(runtime.mailbox().len(), 0);
        assert_eq!(runtime.alarms().len(), 0);
        assert_eq!(runtime.watchdog_entry(ModuleId::Sensor), None);
        assert_eq!(
            runtime.quota_usage(ModuleId::Sensor),
            Some(SystemBudget::ZERO)
        );
    }

    #[test]
    fn runtime_hot_reload_rejects_disabled_and_kernel_modules() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();
        let mut leases = FakeLeases {
            owner: 1,
            released: 1,
            calls: 0,
        };
        let mut hooks = FakeHooks::default();

        assert_eq!(
            runtime.reload_module::<5, _, _>(
                ModuleReloadRequest::new(ModuleId::Kernel, 1, 2, 20, HotReloadPolicy::DEFAULT,),
                &mut leases,
                &mut hooks,
            ),
            Err(RuntimeError::HotReload(HotReloadError::CannotReloadKernel))
        );
        assert_eq!(leases.calls, 0);

        runtime.disable_module(ModuleId::Sensor, 30).unwrap();
        assert_eq!(
            runtime.reload_module::<5, _, _>(
                ModuleReloadRequest::new(ModuleId::Sensor, 1, 2, 40, HotReloadPolicy::DEFAULT,),
                &mut leases,
                &mut hooks,
            ),
            Err(RuntimeError::Module(ModuleRuntimeError::Disabled(
                ModuleId::Sensor
            )))
        );
        assert_eq!(leases.calls, 0);
    }

    #[test]
    fn runtime_reload_stays_suspended_when_mount_fails() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();
        let mut leases = FakeLeases {
            owner: 1,
            released: 1,
            calls: 0,
        };
        let mut hooks = FakeHooks {
            fail: Some(ModuleHookError::Mount),
            ..FakeHooks::default()
        };

        assert_eq!(
            runtime.reload_module::<5, _, _>(
                ModuleReloadRequest::new(ModuleId::Sensor, 1, 2, 20, HotReloadPolicy::DEFAULT,),
                &mut leases,
                &mut hooks,
            ),
            Err(RuntimeError::ModuleHook(ModuleHookError::Mount))
        );
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Suspended)
        );
        assert_eq!(hooks.mounted_revision, None);
    }

    #[test]
    fn recovery_hook_failure_does_not_report_restart_success() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();
        runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 20)
            .unwrap();
        runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 30)
            .unwrap();
        runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 40)
            .unwrap();
        let mut hooks = FakeHooks {
            fail: Some(ModuleHookError::Start),
            ..FakeHooks::default()
        };

        assert_eq!(
            runtime.apply_recovery_step(
                RecoveryStep::new(ModuleId::Sensor, RecoveryStepKind::RestartModule, 50, 5_000,),
                50,
                &mut hooks,
            ),
            Err(RuntimeError::ModuleHook(ModuleHookError::Start))
        );
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Recovering)
        );
        assert_eq!(runtime.state(), SystemState::Recovering);
    }

    #[test]
    fn concurrent_module_recovery_keeps_global_state_until_last_resume() {
        let manifest = SystemManifest::<3>::from_specs(&[
            kernel_module_spec(
                MemoryBudget::new(16 * 1024, 4 * 1024, 4),
                DeadlineContract::new(20_000, 10),
            ),
            ModuleSpec::new(ModuleId::Sensor, Criticality::Driver).memory(MemoryBudget::new(
                4 * 1024,
                1024,
                1,
            )),
            ModuleSpec::new(ModuleId::Radio, Criticality::Driver).memory(MemoryBudget::new(
                4 * 1024,
                1024,
                1,
            )),
        ])
        .unwrap();
        let startup = [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty()),
            StartupNode::new(ModuleId::Sensor, DependencySet::empty().with_index(0)),
            StartupNode::new(ModuleId::Radio, DependencySet::empty().with_index(0)),
        ];
        let mut runtime = Runtime::<3, 3, 4, 4, 4, 3, 16>::admit(
            &manifest,
            &startup,
            profile(),
            FaultThresholds {
                notify_after: 1,
                reboot_after: 3,
            },
        )
        .unwrap();
        runtime.boot_to_running(10).unwrap();
        for (module, base) in [(ModuleId::Sensor, 20), (ModuleId::Radio, 50)] {
            for offset in 0..3 {
                runtime
                    .record_error(module, KernelError::DeadlineMissed, base + offset)
                    .unwrap();
            }
        }

        let mut hooks = FakeHooks::default();
        runtime
            .apply_recovery_step(
                RecoveryStep::new(ModuleId::Sensor, RecoveryStepKind::ResumeModule, 100, 500),
                100,
                &mut hooks,
            )
            .unwrap();
        assert_eq!(runtime.state(), SystemState::Recovering);
        assert_eq!(
            runtime.module_state(ModuleId::Radio),
            Some(ModuleRunState::Recovering)
        );

        runtime
            .apply_recovery_step(
                RecoveryStep::new(ModuleId::Radio, RecoveryStepKind::ResumeModule, 110, 500),
                110,
                &mut hooks,
            )
            .unwrap();
        assert_eq!(runtime.state(), SystemState::Running);
    }

    #[test]
    fn runtime_rejects_disabled_quota_operations() {
        let mut runtime = runtime();
        runtime.disable_module(ModuleId::Sensor, 10).unwrap();

        assert_eq!(
            runtime.reserve_quota(ModuleId::Sensor, SystemBudget::new(1, 0, 0)),
            Err(RuntimeError::Module(ModuleRuntimeError::Disabled(
                ModuleId::Sensor
            )))
        );
        assert_eq!(
            runtime.release_quota(ModuleId::Sensor, SystemBudget::new(1, 0, 0)),
            Err(RuntimeError::Module(ModuleRuntimeError::Disabled(
                ModuleId::Sensor
            )))
        );
        assert_eq!(
            runtime.quota_usage(ModuleId::Sensor),
            Some(SystemBudget::ZERO)
        );
    }

    #[test]
    fn runtime_rejects_unknown_module_before_mutating_recovery() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();
        let before_events = runtime.recovery().events().len();

        assert_eq!(
            runtime.record_error(ModuleId::Radio, KernelError::RadioTxFail, 20),
            Err(RuntimeError::Module(ModuleRuntimeError::Missing(
                ModuleId::Radio
            )))
        );
        assert_eq!(runtime.recovery().events().len(), before_events);
        assert_eq!(runtime.health_report(ModuleId::Radio), None);
        assert_eq!(runtime.module_state(ModuleId::Radio), None);
    }

    #[test]
    fn runtime_records_fault_with_recovery_plan() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();

        let planning = runtime
            .record_error_with_plan::<2>(
                ModuleId::Sensor,
                KernelError::SensorReadFail,
                20,
                RecoveryPlanPolicy::DEFAULT,
            )
            .unwrap();

        assert_eq!(planning.outcome.action, Action::NotifyUserTask);
        assert_eq!(planning.outcome.state, SystemState::Degraded);
        assert_eq!(planning.plan.len, 1);
        assert_eq!(planning.plan.required_budget_us, 500);
        assert_eq!(
            planning.plan.first(),
            Some(RecoveryStep::new(
                ModuleId::Sensor,
                RecoveryStepKind::Notify,
                20,
                500
            ))
        );
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Faulted)
        );
    }

    #[test]
    fn runtime_records_watchdog_fault_with_reboot_plan() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();

        runtime
            .record_watchdog_expired(ModuleId::Sensor, 20)
            .unwrap();
        runtime
            .record_watchdog_expired(ModuleId::Sensor, 30)
            .unwrap();
        let planning = runtime
            .record_watchdog_expired_with_plan::<4>(
                ModuleId::Sensor,
                40,
                RecoveryPlanPolicy::DEFAULT,
            )
            .unwrap();

        assert_eq!(planning.outcome.action, Action::RebootModule);
        assert_eq!(planning.outcome.state, SystemState::Recovering);
        assert_eq!(planning.plan.len, 4);
        assert_eq!(planning.plan.deadline_us, 7_040);
        assert_eq!(
            planning.plan.last(),
            Some(RecoveryStep::new(
                ModuleId::Sensor,
                RecoveryStepKind::ResumeModule,
                6_540,
                500
            ))
        );
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Recovering)
        );
    }

    #[test]
    fn runtime_records_fault_with_dependency_impact_plan() {
        let manifest: SystemManifest<4> = SystemManifest::from_specs(&[
            kernel_module_spec(
                MemoryBudget::new(16 * 1024, 4 * 1024, 4),
                DeadlineContract::new(20_000, 10),
            ),
            ModuleSpec::new(ModuleId::Bus, Criticality::System).memory(MemoryBudget::new(
                4 * 1024,
                1024,
                1,
            )),
            ModuleSpec::new(ModuleId::Sensor, Criticality::Driver).memory(MemoryBudget::new(
                4 * 1024,
                1024,
                1,
            )),
            ModuleSpec::new(ModuleId::App(1), Criticality::User).memory(MemoryBudget::new(
                4 * 1024,
                1024,
                1,
            )),
        ])
        .unwrap();
        let mut graph = StartupGraph::<4>::from_modules(&[
            ModuleId::Kernel,
            ModuleId::Bus,
            ModuleId::Sensor,
            ModuleId::App(1),
        ])
        .unwrap();
        graph
            .add_dependency(ModuleId::Bus, ModuleId::Kernel)
            .unwrap();
        graph
            .add_dependency(ModuleId::Sensor, ModuleId::Bus)
            .unwrap();
        graph
            .add_dependency(ModuleId::App(1), ModuleId::Sensor)
            .unwrap();
        let impact = graph.dependency_impact::<2>(ModuleId::Bus).unwrap();
        let mut runtime = Runtime::<4, 4, 4, 4, 4, 4, 16>::admit_graph(
            &manifest,
            &graph,
            profile(),
            FaultThresholds {
                notify_after: 1,
                reboot_after: 3,
            },
        )
        .unwrap();
        runtime.boot_to_running(10).unwrap();
        runtime
            .record_error(ModuleId::Bus, KernelError::BusTimeout, 20)
            .unwrap();
        runtime
            .record_error(ModuleId::Bus, KernelError::BusTimeout, 30)
            .unwrap();

        let planning = runtime
            .record_error_with_plan_and_impact::<8, 2>(
                ModuleId::Bus,
                KernelError::BusTimeout,
                &impact,
                40,
                RecoveryPlanPolicy::DEFAULT,
            )
            .unwrap();

        assert_eq!(planning.outcome.action, Action::RebootModule);
        assert_eq!(planning.plan.len, 8);
        assert_eq!(
            planning.plan.steps[0],
            Some(RecoveryStep::new(
                ModuleId::App(1),
                RecoveryStepKind::QuiesceModule,
                40,
                500
            ))
        );
        assert_eq!(
            planning.plan.steps[7],
            Some(RecoveryStep::new(
                ModuleId::App(1),
                RecoveryStepKind::ResumeModule,
                8_540,
                500
            ))
        );
        assert_eq!(
            runtime.module_state(ModuleId::Bus),
            Some(ModuleRunState::Recovering)
        );

        runtime
            .apply_recovery_step(
                planning.plan.steps[0].unwrap(),
                50,
                &mut FakeHooks::default(),
            )
            .unwrap();
        runtime
            .apply_recovery_step(
                planning.plan.steps[1].unwrap(),
                60,
                &mut FakeHooks::default(),
            )
            .unwrap();
        runtime
            .apply_recovery_step(
                planning.plan.steps[2].unwrap(),
                70,
                &mut FakeHooks::default(),
            )
            .unwrap();
        assert_eq!(
            runtime.module_state(ModuleId::App(1)),
            Some(ModuleRunState::Suspended)
        );
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Suspended)
        );
        assert_eq!(
            runtime.module_state(ModuleId::Bus),
            Some(ModuleRunState::Recovering)
        );

        for step in planning.plan.steps[3..8].iter().flatten() {
            runtime
                .apply_recovery_step(*step, step.due_us, &mut FakeHooks::default())
                .unwrap();
        }
        assert_eq!(
            runtime.module_state(ModuleId::Bus),
            Some(ModuleRunState::Active)
        );
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Active)
        );
        assert_eq!(
            runtime.module_state(ModuleId::App(1)),
            Some(ModuleRunState::Active)
        );
        assert_eq!(runtime.state(), SystemState::Running);
    }

    #[test]
    fn runtime_surfaces_recovery_plan_capacity_errors() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();

        runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 20)
            .unwrap();
        runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 30)
            .unwrap();

        assert_eq!(
            runtime.record_error_with_plan::<3>(
                ModuleId::Sensor,
                KernelError::SensorReadFail,
                40,
                RecoveryPlanPolicy::DEFAULT,
            ),
            Err(RuntimeError::RecoveryPlan(RecoveryPlanError::Full))
        );
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Recovering)
        );
    }

    #[test]
    fn runtime_rejects_unknown_module_scheduling_and_watchdog() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();

        assert_eq!(
            runtime.schedule_once(AlarmId(9), ModuleId::Radio, 100, 10),
            Err(RuntimeError::Module(ModuleRuntimeError::Missing(
                ModuleId::Radio
            )))
        );
        assert_eq!(runtime.alarms().len(), 0);
        assert_eq!(
            runtime.register_watchdog(ModuleId::Radio, 100, 10),
            Err(RuntimeError::Module(ModuleRuntimeError::Missing(
                ModuleId::Radio
            )))
        );
        assert_eq!(runtime.watchdog_entry(ModuleId::Radio), None);
    }

    #[test]
    fn runtime_applies_degrade_decision_to_module_state() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();
        runtime
            .send(Message::new(
                ModuleId::Kernel,
                ModuleId::Sensor,
                MessageKind::Command,
                1,
                0,
            ))
            .unwrap();
        runtime
            .schedule_once(AlarmId(4), ModuleId::Sensor, 30, 0)
            .unwrap();
        runtime.register_watchdog(ModuleId::Sensor, 100, 0).unwrap();
        runtime
            .reserve_quota(ModuleId::Sensor, SystemBudget::new(128, 64, 1))
            .unwrap();
        let decision = DegradeDecision {
            enabled: [true, false],
            disabled: [Some(ModuleId::Sensor), None],
            disabled_count: 1,
            budget: SystemBudget::new(16 * 1024, 4 * 1024, 4),
            reason: Some(DegradeReason::RamBudget),
        };

        let application = runtime.apply_degrade_decision(&decision, 20).unwrap();

        assert_eq!(
            application,
            DegradeApplication {
                requested: 1,
                disabled: 1,
                already_disabled: 0,
                reason: Some(DegradeReason::RamBudget),
                applied_at_us: 20,
            }
        );
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Disabled)
        );
        assert_eq!(runtime.mailbox().len(), 0);
        assert_eq!(runtime.alarms().len(), 0);
        assert_eq!(runtime.watchdog_entry(ModuleId::Sensor), None);
        assert_eq!(
            runtime.quota_usage(ModuleId::Sensor),
            Some(SystemBudget::ZERO)
        );
        assert_eq!(runtime.state(), SystemState::Degraded);

        let application = runtime.apply_degrade_decision(&decision, 30).unwrap();
        assert_eq!(
            application,
            DegradeApplication {
                requested: 1,
                disabled: 0,
                already_disabled: 1,
                reason: Some(DegradeReason::RamBudget),
                applied_at_us: 30,
            }
        );
        assert_eq!(application.touched_modules(), 1);
    }

    #[test]
    fn runtime_exports_degrade_application_report() {
        let mut runtime = runtime();

        let report = runtime.degrade_application_report();
        assert!(report.verify_checksum());
        assert_eq!(report.requested_count, 0);
        assert_eq!(report.reason, crate::degrade_reason_code(None));

        runtime.boot_to_running(10).unwrap();
        let decision = DegradeDecision {
            enabled: [true, false],
            disabled: [Some(ModuleId::Sensor), None],
            disabled_count: 1,
            budget: SystemBudget::new(16 * 1024, 4 * 1024, 4),
            reason: Some(DegradeReason::PoolBudget),
        };
        runtime
            .apply_degrade_decision(&decision, 0x1_0000_0020)
            .unwrap();

        let report = runtime.degrade_application_report();
        assert!(report.verify_checksum());
        assert_eq!(report.requested_count, 1);
        assert_eq!(report.disabled_count, 1);
        assert_eq!(report.already_disabled_count, 0);
        assert_eq!(
            report.reason,
            crate::degrade_reason_code(Some(DegradeReason::PoolBudget))
        );
        assert_eq!(report.applied_at_us(), 0x1_0000_0020);
    }

    #[test]
    fn runtime_rejects_unknown_degrade_module_before_lifecycle_change() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();
        let before_events = runtime.recovery().events().len();
        let decision = DegradeDecision {
            enabled: [true, false],
            disabled: [Some(ModuleId::Radio), None],
            disabled_count: 1,
            budget: SystemBudget::new(16 * 1024, 4 * 1024, 4),
            reason: Some(DegradeReason::ModuleLimit),
        };

        assert_eq!(
            runtime.apply_degrade_decision(&decision, 20),
            Err(RuntimeError::Module(ModuleRuntimeError::Missing(
                ModuleId::Radio
            )))
        );
        assert_eq!(runtime.state(), SystemState::Running);
        assert_eq!(runtime.recovery().events().len(), before_events);
    }

    #[test]
    fn runtime_rejects_partial_degrade_application() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();
        let decision = DegradeDecision {
            enabled: [true, false, false],
            disabled: [Some(ModuleId::Sensor), Some(ModuleId::Radio), None],
            disabled_count: 2,
            budget: SystemBudget::new(16 * 1024, 4 * 1024, 4),
            reason: Some(DegradeReason::FlashBudget),
        };

        assert_eq!(
            runtime.apply_degrade_decision(&decision, 20),
            Err(RuntimeError::Module(ModuleRuntimeError::Missing(
                ModuleId::Radio
            )))
        );
        assert_eq!(
            runtime.module_state(ModuleId::Sensor),
            Some(ModuleRunState::Active)
        );
        assert_eq!(runtime.state(), SystemState::Running);
    }

    #[test]
    fn runtime_tracks_quota_reserve_and_release() {
        let mut runtime = runtime();

        runtime
            .reserve_quota(ModuleId::Sensor, SystemBudget::new(1024, 256, 1))
            .unwrap();
        runtime
            .release_quota(ModuleId::Sensor, SystemBudget::new(256, 64, 1))
            .unwrap();

        assert_eq!(
            runtime.quota_usage(ModuleId::Sensor),
            Some(SystemBudget::new(768, 192, 0))
        );
        assert_eq!(
            runtime.quota_available(ModuleId::Sensor),
            Some(SystemBudget::new(7 * 1024 + 256, 2 * 1024 - 192, 2))
        );
        assert_eq!(runtime.total_quota_used(), SystemBudget::new(768, 192, 0));
    }

    #[test]
    fn runtime_reports_quota_overrun_without_mutating_usage() {
        let mut runtime = runtime();

        assert_eq!(
            runtime.reserve_quota(ModuleId::Sensor, SystemBudget::new(9 * 1024, 0, 0)),
            Err(RuntimeError::Quota(QuotaError::Exceeded {
                module: ModuleId::Sensor,
                used: SystemBudget::new(9 * 1024, 0, 0),
                limit: SystemBudget::new(8 * 1024, 2 * 1024, 2),
            }))
        );
        assert_eq!(
            runtime.quota_usage(ModuleId::Sensor),
            Some(SystemBudget::ZERO)
        );
    }

    #[test]
    fn runtime_sweeps_watchdog_expiry_into_recovery() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();
        runtime
            .register_watchdog(ModuleId::Sensor, 100, 20)
            .unwrap();
        runtime.heartbeat(ModuleId::Sensor, 80).unwrap();

        assert!(runtime.sweep_watchdogs(150).unwrap().is_empty());
        assert_eq!(runtime.watchdog_expired_count(181), 1);
        assert_eq!(
            runtime
                .watchdog_entry(ModuleId::Sensor)
                .expect("watchdog")
                .missed,
            0
        );
        let sweep = runtime.sweep_watchdogs(181).unwrap();
        let report = runtime
            .health_report(ModuleId::Sensor)
            .expect("health report");

        assert_eq!(sweep.len, 1);
        assert_eq!(
            sweep.outcomes[0].map(|outcome| outcome.error),
            Some(KernelError::WatchdogExpired)
        );
        assert_eq!(runtime.state(), SystemState::Degraded);
        assert_eq!(
            runtime
                .watchdog_entry(ModuleId::Sensor)
                .expect("watchdog")
                .missed,
            1
        );
        assert!(runtime.sweep_watchdogs(250).unwrap().is_empty());
        assert_eq!(
            runtime
                .watchdog_entry(ModuleId::Sensor)
                .expect("watchdog")
                .missed,
            1
        );
        assert!(report.verify_checksum());
        assert_eq!(report.total_errors, 1);
    }

    #[test]
    fn runtime_completes_module_recovery_back_to_running() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();

        runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 20)
            .unwrap();
        runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 30)
            .unwrap();
        let outcome = runtime
            .record_error(ModuleId::Sensor, KernelError::SensorReadFail, 40)
            .unwrap();
        runtime
            .complete_module_recovery(ModuleId::Sensor, 50)
            .unwrap();
        let report = runtime
            .health_report(ModuleId::Sensor)
            .expect("health report");

        assert_eq!(outcome.state, SystemState::Recovering);
        assert_eq!(runtime.state(), SystemState::Running);
        assert_eq!(report.total_errors, 3);
        assert_eq!(report.consecutive_errors, 0);
        assert_eq!(report.last_seen_us(), 50);
    }

    #[test]
    fn runtime_rejects_recovery_completion_outside_recovering_state() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();

        assert_eq!(
            runtime.complete_module_recovery(ModuleId::Sensor, 20),
            Err(RuntimeError::RecoveryNotActive(ModuleId::Sensor))
        );
    }

    #[test]
    fn runtime_rejects_disabled_recovery_completion_before_lifecycle_change() {
        let mut runtime = runtime();
        runtime.boot_to_running(10).unwrap();
        runtime.disable_module(ModuleId::Sensor, 20).unwrap();

        assert_eq!(
            runtime.complete_module_recovery(ModuleId::Sensor, 30),
            Err(RuntimeError::Module(ModuleRuntimeError::Disabled(
                ModuleId::Sensor
            )))
        );
        assert_eq!(runtime.state(), SystemState::Running);
    }

    #[test]
    fn runtime_heartbeat_reports_missing_watchdog() {
        let mut runtime = runtime();

        assert_eq!(
            runtime.heartbeat(ModuleId::Sensor, 10),
            Err(RuntimeError::Watchdog(WatchdogError::Missing(
                ModuleId::Sensor
            )))
        );
    }
}
