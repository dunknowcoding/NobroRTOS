//! Dispatcher-owned module context: identity, authorization, quota, operation,
//! and trace as one inseparable path for general Rust modules.
//!
//! This is the Rust-module counterpart of `ForeignHostContext` (the C-ABI host
//! path). The dispatcher — the only code holding `&mut Runtime` — opens a scope
//! with [`Runtime::with_module`]; the module body receives a [`ModuleCtx`] whose
//! identity it cannot choose or forge (the field is private and only the runtime
//! constructs the context). Every protected operation on the context:
//!
//! 1. authorizes the module's manifest grant for the operation's capability,
//! 2. charges the corresponding object/pool quota,
//! 3. performs the operation,
//! 4. records an attempt/completion pair in the runtime capability trace
//!    (denials record a `Fault`), so absence from the trace is meaningful for
//!    everything routed through a context.
//!
//! Raw `Runtime` methods remain available to the trusted dispatcher only; module
//! code must never be handed `&mut Runtime`.

use crate::{
    AlarmId, Capability, CapabilityTraceInput, CapabilityTraceOp, KernelError, KvKey, KvValue,
    Message, MessageKind, ModuleId, PoolHandle, RecoveryOutcome, Runtime, RuntimeError, Sample,
    SampleKind, SamplePool, SystemBudget,
};

pub struct ModuleCtx<
    'r,
    const STARTUP: usize,
    const QUOTAS: usize,
    const MAILBOX: usize,
    const ALARMS: usize,
    const KV: usize,
    const HEALTH: usize,
    const LOG: usize,
> {
    runtime: &'r mut Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
    module: ModuleId,
    now_us: u64,
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
    /// Open a dispatcher-owned scope for one module. Fails if the module is not
    /// admitted or is disabled; inside the scope every protected operation is
    /// authorized, charged, and traced under the fixed module identity.
    pub fn with_module<R>(
        &mut self,
        module: ModuleId,
        now_us: u64,
        f: impl FnOnce(&mut ModuleCtx<'_, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>) -> R,
    ) -> Result<R, RuntimeError> {
        self.ensure_module_enabled(module)?;
        let mut ctx = ModuleCtx {
            runtime: self,
            module,
            now_us,
        };
        Ok(f(&mut ctx))
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
    > ModuleCtx<'_, STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>
{
    pub const fn module(&self) -> ModuleId {
        self.module
    }

    pub const fn now_us(&self) -> u64 {
        self.now_us
    }

    fn protected<T>(
        &mut self,
        capability: Capability,
        op: CapabilityTraceOp,
        args: (u32, u32),
        operation: impl FnOnce(
            &mut Runtime<STARTUP, QUOTAS, MAILBOX, ALARMS, KV, HEALTH, LOG>,
        ) -> Result<T, RuntimeError>,
    ) -> Result<T, RuntimeError> {
        self.runtime
            .authorize_traced(self.module, capability, self.now_us)?;
        self.runtime.trace_record(
            CapabilityTraceInput::new(self.module, capability, op, self.now_us)
                .args(args.0, args.1)
                .result(u32::MAX),
        );
        let result = operation(self.runtime);
        self.runtime.trace_record(
            CapabilityTraceInput::new(self.module, capability, op, self.now_us)
                .args(args.0, args.1)
                .result(result.is_err() as u32),
        );
        result
    }

    /// Send a message. The sender is the context identity — a module cannot
    /// forge another module's `from` field.
    pub fn send(
        &mut self,
        to: ModuleId,
        kind: MessageKind,
        arg0: u32,
        arg1: u32,
    ) -> Result<(), RuntimeError> {
        let message = Message::new(self.module, to, kind, arg0, arg1);
        self.protected(
            Capability::Mailbox,
            CapabilityTraceOp::Write,
            (arg0, arg1),
            |runtime| runtime.send(message),
        )
    }

    /// Receive the oldest message addressed to this module (identity-scoped:
    /// a module cannot read another module's mail).
    pub fn recv(&mut self) -> Result<Option<Message>, RuntimeError> {
        let module = self.module;
        self.protected(
            Capability::Mailbox,
            CapabilityTraceOp::Read,
            (0, 0),
            |runtime| Ok(runtime.recv_for(module)),
        )
    }

    pub fn schedule_once(&mut self, id: AlarmId, delay_us: u64) -> Result<(), RuntimeError> {
        let (module, now_us) = (self.module, self.now_us);
        self.protected(
            Capability::Alarm,
            CapabilityTraceOp::Acquire,
            (u32::from(id.0), delay_us as u32),
            |runtime| runtime.schedule_once(id, module, delay_us, now_us),
        )
    }

    pub fn schedule_periodic(&mut self, id: AlarmId, period_us: u32) -> Result<(), RuntimeError> {
        let (module, now_us) = (self.module, self.now_us);
        self.protected(
            Capability::Alarm,
            CapabilityTraceOp::Acquire,
            (u32::from(id.0), period_us),
            |runtime| runtime.schedule_periodic(id, module, period_us, now_us),
        )
    }

    /// Cancel an alarm. Only the owner may cancel: a foreign alarm is
    /// re-scheduled untouched and the call is denied.
    pub fn cancel_alarm(&mut self, id: AlarmId) -> Result<(), RuntimeError> {
        let module = self.module;
        self.protected(
            Capability::Alarm,
            CapabilityTraceOp::Release,
            (u32::from(id.0), 0),
            |runtime| {
                let alarm = runtime.cancel_alarm(id)?;
                if alarm.module != module {
                    // Put the foreign alarm back exactly as it was and deny.
                    runtime.restore_alarm(alarm)?;
                    return Err(RuntimeError::AlarmOwnedByOther {
                        id,
                        owner: alarm.module,
                    });
                }
                Ok(())
            },
        )
    }

    pub fn kv_set(&mut self, key: KvKey, value: KvValue) -> Result<(), RuntimeError> {
        let module = self.module;
        self.protected(
            Capability::KvStore,
            CapabilityTraceOp::Write,
            (u32::from(key.0), 0),
            |runtime| runtime.kv_set_owned(module, key, value),
        )
    }

    pub fn kv_get(&mut self, key: KvKey) -> Result<Option<KvValue>, RuntimeError> {
        self.protected(
            Capability::KvStore,
            CapabilityTraceOp::Read,
            (u32::from(key.0), 0),
            |runtime| Ok(runtime.kv_get(key)),
        )
    }

    pub fn kv_delete(&mut self, key: KvKey) -> Result<KvValue, RuntimeError> {
        let module = self.module;
        self.protected(
            Capability::KvStore,
            CapabilityTraceOp::Release,
            (u32::from(key.0), 0),
            |runtime| runtime.kv_delete_owned(module, key),
        )
    }

    /// Allocate a sample-pool ticket, charging one pool slot against the
    /// module's admitted memory budget.
    pub fn pool_alloc(
        &mut self,
        kind: SampleKind,
        len: u16,
        deadline_us: u64,
    ) -> Result<Sample, RuntimeError> {
        let (module, now_us) = (self.module, self.now_us);
        self.protected(
            Capability::SamplePool,
            CapabilityTraceOp::Acquire,
            (kind as u32, u32::from(len)),
            |runtime| {
                runtime.reserve_quota(module, SystemBudget::new(0, 0, 1))?;
                match SamplePool::alloc(kind, len, now_us, deadline_us) {
                    Some(sample) => Ok(sample),
                    None => {
                        runtime.release_quota(module, SystemBudget::new(0, 0, 1))?;
                        Err(RuntimeError::PoolExhausted)
                    }
                }
            },
        )
    }

    /// Release a sample-pool ticket allocated through this module's context,
    /// returning the pool slot to the module's budget.
    pub fn pool_release(&mut self, sample: Sample) -> Result<(), RuntimeError> {
        let module = self.module;
        let handle: PoolHandle = sample.handle;
        self.protected(
            Capability::SamplePool,
            CapabilityTraceOp::Release,
            (handle.index() as u32, 0),
            |runtime| {
                if !SamplePool::release(handle) {
                    return Err(RuntimeError::PoolStaleHandle);
                }
                runtime.release_quota(module, SystemBudget::new(0, 0, 1))
            },
        )
    }

    /// Report healthy progress (feeds the watchdog and health monitor).
    pub fn heartbeat(&mut self) -> Result<(), RuntimeError> {
        let (module, now_us) = (self.module, self.now_us);
        self.runtime.heartbeat(module, now_us)
    }

    /// Record a fault under the context identity.
    pub fn record_error(&mut self, error: KernelError) -> Result<RecoveryOutcome, RuntimeError> {
        let (module, now_us) = (self.module, self.now_us);
        self.runtime.record_error(module, error, now_us)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        kernel_module_spec, CapabilityReplayScope, CapabilitySet, CapabilityTraceRecord,
        Criticality, DeadlineContract, DependencySet, FaultThresholds, MemoryBudget, ModuleSpec,
        ObjectQuota, StartupNode, SystemManifest, SystemProfile,
    };

    type TestRuntime = Runtime<4, 4, 4, 4, 4, 4, 32>;

    fn runtime_with_sensor(granted: CapabilitySet, objects: ObjectQuota) -> TestRuntime {
        let mut manifest = SystemManifest::<4>::new();
        manifest
            .add(kernel_module_spec(
                MemoryBudget::new(4096, 1024, 1),
                DeadlineContract::new(1000, 10),
            ))
            .unwrap();
        manifest
            .add(
                ModuleSpec::new(ModuleId::Sensor, Criticality::User)
                    .requires(granted)
                    .memory(MemoryBudget::new(1024, 256, 4))
                    .objects(objects),
            )
            .unwrap();
        // Someone must own the granted capabilities for the manifest to validate.
        manifest
            .add(
                ModuleSpec::new(ModuleId::Bus, Criticality::System)
                    .owns(granted)
                    .memory(MemoryBudget::new(512, 128, 0)),
            )
            .unwrap();
        let nodes = [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty()),
            StartupNode::new(ModuleId::Bus, DependencySet::empty()),
            StartupNode::new(ModuleId::Sensor, DependencySet::empty()),
        ];
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

    fn caps(list: &[Capability]) -> CapabilitySet {
        list.iter()
            .fold(CapabilitySet::empty(), |set, cap| set.with(*cap))
    }

    #[test]
    fn ctx_combines_identity_authorization_quota_and_trace() {
        let mut runtime = runtime_with_sensor(
            caps(&[Capability::Mailbox, Capability::Alarm]),
            ObjectQuota::new(1, 8, 8),
        );

        runtime
            .with_module(ModuleId::Sensor, 10, |ctx| {
                assert_eq!(ctx.module(), ModuleId::Sensor);
                // Authorized + within quota.
                ctx.send(ModuleId::Bus, MessageKind::Command, 1, 2).unwrap();
                // Second send exceeds the 1-slot mailbox object quota.
                assert!(matches!(
                    ctx.send(ModuleId::Bus, MessageKind::Command, 3, 4),
                    Err(RuntimeError::Object(_))
                ));
                // KvStore capability was never granted: denied and trace-faulted.
                assert!(matches!(
                    ctx.kv_set(KvKey(9), KvValue::U32(1)),
                    Err(RuntimeError::Capability(_))
                ));
            })
            .unwrap();

        // The trace holds the attempt/completion pair, the quota fault path, and
        // the capability denial fault — absence is meaningful for ctx operations.
        let mut records = [CapabilityTraceRecord::EMPTY; 32];
        let copied = runtime.copy_capability_trace(
            CapabilityReplayScope::module(ModuleId::Sensor),
            &mut records,
        );
        assert!(copied >= 3);
        assert!(records[..copied]
            .iter()
            .any(|record| record.op == CapabilityTraceOp::Fault
                && record.capability == Capability::KvStore));
        // The successful send released nothing: one mailbox slot is held.
        assert_eq!(
            runtime
                .object_usage(ModuleId::Sensor)
                .unwrap()
                .mailbox_slots,
            1
        );
    }

    #[test]
    fn ctx_identity_cannot_touch_foreign_alarms_or_kv() {
        let mut runtime = runtime_with_sensor(
            caps(&[Capability::Alarm, Capability::KvStore]),
            ObjectQuota::DEFAULT,
        );
        // Kernel (dispatcher) schedules its own alarm and owns a KV key.
        runtime
            .schedule_once(AlarmId(1), ModuleId::Kernel, 100, 0)
            .unwrap();
        runtime.kv_set(KvKey(1), KvValue::U32(7)).unwrap();

        runtime
            .with_module(ModuleId::Sensor, 10, |ctx| {
                assert!(matches!(
                    ctx.cancel_alarm(AlarmId(1)),
                    Err(RuntimeError::AlarmOwnedByOther { .. })
                ));
                assert!(matches!(
                    ctx.kv_delete(KvKey(1)),
                    Err(RuntimeError::KvOwnedByOther { .. })
                ));
                assert!(matches!(
                    ctx.kv_set(KvKey(1), KvValue::U32(9)),
                    Err(RuntimeError::KvOwnedByOther { .. })
                ));
                // Own key lifecycle works and reconciles.
                ctx.kv_set(KvKey(2), KvValue::U32(1)).unwrap();
                assert_eq!(ctx.kv_get(KvKey(2)).unwrap(), Some(KvValue::U32(1)));
                ctx.kv_delete(KvKey(2)).unwrap();
            })
            .unwrap();

        // The kernel alarm survived the denied cancel.
        assert_eq!(runtime.alarms().len(), 1);
        assert_eq!(
            runtime.object_usage(ModuleId::Sensor).unwrap().kv_entries,
            0
        );
    }

    #[test]
    fn overutilized_manifest_is_rejected_at_admission() {
        use crate::ManifestError;
        let mut manifest = SystemManifest::<3>::new();
        manifest
            .add(kernel_module_spec(
                MemoryBudget::new(4096, 1024, 1),
                DeadlineContract::new(1000, 10).execution_budget(600),
            ))
            .unwrap();
        manifest
            .add(
                ModuleSpec::new(ModuleId::Sensor, Criticality::HardRealtime)
                    .memory(MemoryBudget::new(1024, 256, 0))
                    .deadline(DeadlineContract::new(1000, 10).execution_budget(500)),
            )
            .unwrap();
        // 60% + 50% > 100%: the declared costs cannot be scheduled.
        assert!(matches!(
            manifest.validate_profile(SystemProfile::NRF52840_CORE),
            Err(ManifestError::Overutilized {
                utilization_permyriad: 11_000
            })
        ));
    }

    #[test]
    fn incoherent_runtime_capacities_are_rejected() {
        use crate::{CapacityError, FaultThresholds};
        let mut manifest = SystemManifest::<3>::new();
        manifest
            .add(kernel_module_spec(
                MemoryBudget::new(4096, 1024, 1),
                DeadlineContract::new(1000, 10),
            ))
            .unwrap();
        manifest
            .add(
                ModuleSpec::new(ModuleId::Sensor, Criticality::User)
                    .memory(MemoryBudget::new(1024, 256, 0)),
            )
            .unwrap();
        let nodes = [
            StartupNode::new(ModuleId::Kernel, DependencySet::empty()),
            StartupNode::new(ModuleId::Sensor, DependencySet::empty()),
        ];
        // HEALTH = 1 cannot track two admitted modules.
        let result = Runtime::<2, 2, 4, 4, 4, 1, 16>::admit(
            &manifest,
            &nodes,
            SystemProfile::NRF52840_CORE,
            FaultThresholds::DEFAULT,
        );
        assert!(matches!(
            result.err(),
            Some(RuntimeError::Capacity(
                CapacityError::ModuleTablesTooSmall { modules: 2, .. }
            ))
        ));
    }

    #[test]
    fn disabled_module_cannot_open_a_context() {
        let mut runtime = runtime_with_sensor(caps(&[Capability::Mailbox]), ObjectQuota::DEFAULT);
        runtime.disable_module(ModuleId::Sensor, 5).unwrap();
        assert!(runtime
            .with_module(ModuleId::Sensor, 10, |_ctx| ())
            .is_err());
    }

    #[test]
    fn cleanup_reconciles_ctx_held_objects() {
        let mut runtime = runtime_with_sensor(
            caps(&[Capability::Mailbox, Capability::Alarm, Capability::KvStore]),
            ObjectQuota::DEFAULT,
        );
        runtime
            .with_module(ModuleId::Sensor, 10, |ctx| {
                ctx.send(ModuleId::Bus, MessageKind::Command, 1, 0).unwrap();
                ctx.schedule_once(AlarmId(3), 500).unwrap();
                ctx.kv_set(KvKey(4), KvValue::Bool(true)).unwrap();
            })
            .unwrap();
        let usage = runtime.object_usage(ModuleId::Sensor).unwrap();
        assert_eq!(
            (usage.mailbox_slots, usage.alarms, usage.kv_entries),
            (1, 1, 1)
        );

        // Disable cleans up and the post-cleanup invariant proves zero residue.
        runtime.disable_module(ModuleId::Sensor, 20).unwrap();
        let usage = runtime.object_usage(ModuleId::Sensor).unwrap();
        assert_eq!(
            (usage.mailbox_slots, usage.alarms, usage.kv_entries),
            (0, 0, 0)
        );
        assert_eq!(runtime.kv_get(KvKey(4)), None);
    }
}
