//! Fixed-capacity runtime control plane assembled after admission.

use crate::{
    AdmissionController, AdmissionError, AdmissionPlan, Alarm, AlarmError, AlarmId, AlarmQueue,
    Capability, CapabilityGrantError, EventSeverity, FaultThresholds, HealthReport, KernelError,
    KvError, KvKey, KvStore, KvValue, Mailbox, MailboxError, Message, MessageKind, ModuleId,
    RecoveryCoordinator, RecoveryError, RecoveryOutcome, StartupGraph, StartupNode, SystemManifest,
    SystemProfile, SystemState,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    Alarm(AlarmError),
    Capability(CapabilityGrantError),
    Kv(KvError),
    Mailbox(MailboxError),
    Recovery(RecoveryError),
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

impl From<RecoveryError> for RuntimeError {
    fn from(error: RecoveryError) -> Self {
        Self::Recovery(error)
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
}
