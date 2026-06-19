//! Fixed-capacity runtime control plane assembled after admission.

use crate::{
    AdmissionController, AdmissionError, AdmissionPlan, Alarm, AlarmError, AlarmId, AlarmQueue,
    Capability, CapabilityGrantError, EventSeverity, FaultThresholds, HealthReport, KernelError,
    KvError, KvKey, KvStore, KvValue, Mailbox, MailboxError, Message, MessageKind, ModuleId,
    QuotaError, RecoveryCoordinator, RecoveryError, RecoveryOutcome, StartupGraph, StartupNode,
    SystemBudget, SystemManifest, SystemProfile, SystemState, Watchdog, WatchdogEntry,
    WatchdogError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    Alarm(AlarmError),
    Capability(CapabilityGrantError),
    Kv(KvError),
    Mailbox(MailboxError),
    Quota(QuotaError),
    Recovery(RecoveryError),
    Watchdog(WatchdogError),
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
    pub const fn from_plan(
        plan: AdmissionPlan<STARTUP, QUOTAS>,
        thresholds: FaultThresholds,
    ) -> Self {
        Self {
            plan,
            mailbox: Mailbox::new(),
            alarms: AlarmQueue::new(),
            kv: KvStore::new(),
            recovery: RecoveryCoordinator::new(thresholds),
            watchdog: Watchdog::new(),
        }
    }

    pub fn admit<const MODULES: usize>(
        manifest: &SystemManifest<MODULES>,
        startup_nodes: &[StartupNode],
        profile: SystemProfile,
        thresholds: FaultThresholds,
    ) -> Result<Self, AdmissionError> {
        let plan = AdmissionController::admit::<MODULES, STARTUP, QUOTAS>(
            manifest,
            startup_nodes,
            profile,
        )?;
        Ok(Self::from_plan(plan, thresholds))
    }

    pub fn admit_graph<const MODULES: usize, const GRAPH: usize>(
        manifest: &SystemManifest<MODULES>,
        startup: &StartupGraph<GRAPH>,
        profile: SystemProfile,
        thresholds: FaultThresholds,
    ) -> Result<Self, AdmissionError> {
        Self::admit(manifest, startup.as_slice(), profile, thresholds)
    }

    pub fn boot_to_running(&mut self, now_us: u64) -> Result<(), RuntimeError> {
        self.recovery
            .transition(SystemState::ValidateManifest, now_us)?;
        self.recovery.transition(SystemState::InitDrivers, now_us)?;
        self.recovery.transition(SystemState::Running, now_us)?;
        Ok(())
    }

    pub fn authorize(&self, module: ModuleId, capability: Capability) -> Result<(), RuntimeError> {
        self.plan.grants.authorize(module, capability)?;
        Ok(())
    }

    pub fn send(&mut self, message: Message) -> Result<(), RuntimeError> {
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
        self.alarms
            .schedule_periodic(id, module, period_us, now_us)?;
        Ok(())
    }

    pub fn cancel_alarm(&mut self, id: AlarmId) -> Result<Alarm, RuntimeError> {
        self.alarms.cancel(id).map_err(RuntimeError::from)
    }

    pub fn dispatch_due_alarms(&mut self, now_us: u64) -> Result<usize, RuntimeError> {
        let mut dispatched = 0;
        while let Some(alarm) = self.alarms.next_due(now_us) {
            self.mailbox.push(alarm_message(alarm))?;
            self.alarms.pop_due(now_us);
            dispatched += 1;
        }
        Ok(dispatched)
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

    pub fn record_ok(&mut self, module: ModuleId, now_us: u64) {
        self.recovery.record_ok(module, now_us);
    }

    pub fn record_error(
        &mut self,
        module: ModuleId,
        error: KernelError,
        now_us: u64,
    ) -> Result<RecoveryOutcome, RuntimeError> {
        self.recovery
            .record_error(module, error, now_us)
            .map_err(RuntimeError::from)
    }

    pub fn record_watchdog_expired(
        &mut self,
        module: ModuleId,
        now_us: u64,
    ) -> Result<RecoveryOutcome, RuntimeError> {
        self.recovery
            .record_watchdog_expired(module, now_us)
            .map_err(RuntimeError::from)
    }

    pub fn register_watchdog(
        &mut self,
        module: ModuleId,
        timeout_us: u64,
        now_us: u64,
    ) -> Result<(), RuntimeError> {
        self.watchdog.register(module, timeout_us, now_us)?;
        Ok(())
    }

    pub fn heartbeat(&mut self, module: ModuleId, now_us: u64) -> Result<(), RuntimeError> {
        self.watchdog.beat(module, now_us)?;
        self.record_ok(module, now_us);
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
        self.recovery.transition(SystemState::InitDrivers, now_us)?;
        self.recovery.transition(SystemState::Running, now_us)?;
        self.record_ok(module, now_us);
        Ok(())
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
            runtime.heartbeat(ModuleId::Radio, 10),
            Err(RuntimeError::Watchdog(WatchdogError::Missing(
                ModuleId::Radio
            )))
        );
    }
}
