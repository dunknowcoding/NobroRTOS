//! Fixed-capacity runtime control plane assembled after admission.

use crate::{
    AdmissionController, AdmissionError, AdmissionPlan, Alarm, AlarmError, AlarmId, AlarmQueue,
    Capability, CapabilityGrantError, DegradeApplicationReport, DegradeDecision, DegradeReason,
    EventLogReport, EventSeverity, FaultThresholds, HealthReport, KernelError, KvError, KvKey,
    KvStore, KvValue, Mailbox, MailboxError, Message, MessageKind, ModuleId, ModuleRunState,
    ModuleRuntimeEntry, ModuleRuntimeError, ModuleRuntimeGuard, ModuleRuntimeReport, QuotaError,
    RecoveryCoordinator, RecoveryError, RecoveryOutcome, RuntimeReport, RuntimeReportInput,
    StartupGraph, StartupNode, SystemBudget, SystemManifest, SystemProfile, SystemState, Watchdog,
    WatchdogEntry, WatchdogError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    Admission(AdmissionError),
    Alarm(AlarmError),
    Capability(CapabilityGrantError),
    Kv(KvError),
    Mailbox(MailboxError),
    Module(ModuleRuntimeError),
    Quota(QuotaError),
    Recovery(RecoveryError),
    Watchdog(WatchdogError),
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
    recovery: RecoveryCoordinator<HEALTH, LOG>,
    watchdog: Watchdog<HEALTH>,
    modules: ModuleRuntimeGuard<QUOTAS>,
    degrade: DegradeApplication,
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
    pub fn from_plan(
        plan: AdmissionPlan<STARTUP, QUOTAS>,
        thresholds: FaultThresholds,
    ) -> Result<Self, RuntimeError> {
        let modules = ModuleRuntimeGuard::try_from_startup_plan(&plan.startup)?;
        Ok(Self {
            plan,
            mailbox: Mailbox::new(),
            alarms: AlarmQueue::new(),
            kv: KvStore::new(),
            recovery: RecoveryCoordinator::new(thresholds),
            watchdog: Watchdog::new(),
            modules,
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
        Self::from_plan(plan, thresholds)
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
        self.plan.grants.authorize(module, capability)?;
        Ok(())
    }

    pub fn send(&mut self, message: Message) -> Result<(), RuntimeError> {
        self.ensure_message_endpoints_enabled(message)?;
        self.mailbox.push(message)?;
        Ok(())
    }

    pub fn recv(&mut self) -> Option<Message> {
        self.mailbox.pop()
    }

    pub fn recv_for(&mut self, module: ModuleId) -> Option<Message> {
        self.mailbox.pop_for(module)
    }

    pub fn schedule_once(
        &mut self,
        id: AlarmId,
        module: ModuleId,
        delay_us: u64,
        now_us: u64,
    ) -> Result<(), RuntimeError> {
        self.ensure_module_enabled(module)?;
        self.alarms.schedule_once(id, module, delay_us, now_us)?;
        Ok(())
    }

    pub fn schedule_periodic(
        &mut self,
        id: AlarmId,
        module: ModuleId,
        period_us: u32,
        now_us: u64,
    ) -> Result<(), RuntimeError> {
        self.ensure_module_enabled(module)?;
        self.alarms
            .schedule_periodic(id, module, period_us, now_us)?;
        Ok(())
    }

    pub fn cancel_alarm(&mut self, id: AlarmId) -> Result<Alarm, RuntimeError> {
        self.alarms.cancel(id).map_err(RuntimeError::from)
    }

    pub fn dispatch_due_alarms(&mut self, now_us: u64) -> Result<usize, RuntimeError> {
        let dispatch = self.try_dispatch_due_alarms(now_us);
        match dispatch.error {
            Some(error) => Err(error),
            None => Ok(dispatch.dispatched),
        }
    }

    pub fn try_dispatch_due_alarms(&mut self, now_us: u64) -> AlarmDispatch {
        let mut dispatched = 0;
        while let Some(alarm) = self.alarms.next_due(now_us) {
            if let Err(error) = self.mailbox.push(alarm_message(alarm)) {
                return AlarmDispatch::blocked(dispatched, alarm, RuntimeError::Mailbox(error));
            }
            self.alarms.pop_due(now_us);
            dispatched += 1;
        }
        AlarmDispatch::completed(dispatched)
    }

    pub fn dispatch_due_alarms_with_recovery(
        &mut self,
        now_us: u64,
    ) -> Result<AlarmDispatch, RuntimeError> {
        let dispatch = self.try_dispatch_due_alarms(now_us);
        if let Some(alarm) = dispatch.blocked {
            self.record_error(alarm.module, KernelError::DeadlineMissed, now_us)?;
        }
        Ok(dispatch)
    }

    pub fn kv_set(&mut self, key: KvKey, value: KvValue) -> Result<(), RuntimeError> {
        self.kv.set(key, value)?;
        Ok(())
    }

    pub fn kv_get(&self, key: KvKey) -> Option<KvValue> {
        self.kv.get(key)
    }

    pub fn kv_delete(&mut self, key: KvKey) -> Result<KvValue, RuntimeError> {
        self.kv.delete(key).map_err(RuntimeError::from)
    }

    pub fn reserve_quota(
        &mut self,
        module: ModuleId,
        amount: SystemBudget,
    ) -> Result<(), RuntimeError> {
        self.plan.quotas.reserve(module, amount)?;
        Ok(())
    }

    pub fn release_quota(
        &mut self,
        module: ModuleId,
        amount: SystemBudget,
    ) -> Result<(), RuntimeError> {
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
        self.modules.note_recovery_outcome(outcome, now_us)?;
        Ok(outcome)
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
        self.modules.note_recovery_outcome(outcome, now_us)?;
        Ok(outcome)
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
        let expired = self.watchdog.expired(now_us, &mut modules);
        let mut sweep = WatchdogSweep::new();

        for module in modules.iter().copied().take(expired) {
            sweep.push(self.record_watchdog_expired(module, now_us)?);
        }

        Ok(sweep)
    }

    pub fn complete_module_recovery(
        &mut self,
        module: ModuleId,
        now_us: u64,
    ) -> Result<(), RuntimeError> {
        self.ensure_module_admitted(module)?;
        self.recovery.transition(SystemState::InitDrivers, now_us)?;
        self.recovery.transition(SystemState::Running, now_us)?;
        self.record_ok(module, now_us)?;
        self.modules.complete_recovery(module, now_us)?;
        Ok(())
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
        self.modules.disable(module, now_us)?;
        self.alarms.remove_for(module);
        self.mailbox.remove_for(module);
        Ok(())
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
            self.alarms.remove_for(module);
            self.mailbox.remove_for(module);
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

    fn ensure_module_admitted(&self, module: ModuleId) -> Result<(), RuntimeError> {
        if self.modules.entry(module).is_some() {
            Ok(())
        } else {
            Err(RuntimeError::Module(ModuleRuntimeError::Missing(module)))
        }
    }

    fn ensure_module_enabled(&self, module: ModuleId) -> Result<(), RuntimeError> {
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
        kernel_module_spec, CapabilitySet, Criticality, DeadlineContract, DependencySet,
        FaultThresholds, MemoryBudget, ModuleSpec,
    };

    type TestRuntime = Runtime<4, 4, 4, 4, 4, 4, 16>;

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
        runtime
            .send(Message::new(
                ModuleId::Kernel,
                ModuleId::Sensor,
                MessageKind::Command,
                3,
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
        assert_eq!(runtime.mailbox().len(), 0);
        assert_eq!(runtime.alarms().len(), 0);
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
        let sweep = runtime.sweep_watchdogs(181).unwrap();
        let report = runtime
            .health_report(ModuleId::Sensor)
            .expect("health report");

        assert_eq!(sweep.len, 1);
        assert_eq!(
            sweep.outcomes[0].map(|outcome| outcome.error),
            Some(KernelError::DeadlineMissed)
        );
        assert_eq!(runtime.state(), SystemState::Degraded);
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

        assert!(matches!(
            runtime.complete_module_recovery(ModuleId::Sensor, 20),
            Err(RuntimeError::Recovery(RecoveryError::Lifecycle(_)))
        ));
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
