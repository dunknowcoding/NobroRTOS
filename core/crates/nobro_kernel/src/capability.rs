//! Runtime capability grants derived from the static system manifest.

use crate::{Capability, CapabilitySet, ModuleId, SystemManifest};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityGrant {
    pub module: ModuleId,
    pub granted: CapabilitySet,
}

impl CapabilityGrant {
    pub const fn new(module: ModuleId, granted: CapabilitySet) -> Self {
        Self { module, granted }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityGrantError {
    Full,
    Duplicate(ModuleId),
    Missing(ModuleId),
    Denied {
        module: ModuleId,
        capability: Capability,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CapabilityTraceOp {
    Acquire = 1,
    Release = 2,
    Read = 3,
    Write = 4,
    Invoke = 5,
    Fault = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityTraceRecord {
    pub seq: u32,
    pub at_us: u64,
    pub module: ModuleId,
    pub capability: Capability,
    pub op: CapabilityTraceOp,
    pub arg0: u32,
    pub arg1: u32,
    pub result: u32,
}

impl CapabilityTraceRecord {
    pub const EMPTY: Self = Self {
        seq: 0,
        at_us: 0,
        module: ModuleId::Kernel,
        capability: Capability::HostReport,
        op: CapabilityTraceOp::Fault,
        arg0: 0,
        arg1: 0,
        result: 0,
    };

    const fn from_input(seq: u32, input: CapabilityTraceInput) -> Self {
        Self {
            seq,
            at_us: input.at_us,
            module: input.module,
            capability: input.capability,
            op: input.op,
            arg0: input.arg0,
            arg1: input.arg1,
            result: input.result,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityTraceInput {
    pub at_us: u64,
    pub module: ModuleId,
    pub capability: Capability,
    pub op: CapabilityTraceOp,
    pub arg0: u32,
    pub arg1: u32,
    pub result: u32,
}

impl CapabilityTraceInput {
    pub const fn new(
        module: ModuleId,
        capability: Capability,
        op: CapabilityTraceOp,
        at_us: u64,
    ) -> Self {
        Self {
            at_us,
            module,
            capability,
            op,
            arg0: 0,
            arg1: 0,
            result: 0,
        }
    }

    pub const fn args(mut self, arg0: u32, arg1: u32) -> Self {
        self.arg0 = arg0;
        self.arg1 = arg1;
        self
    }

    pub const fn result(mut self, result: u32) -> Self {
        self.result = result;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilityReplayScope {
    pub module: Option<ModuleId>,
    pub capability: Option<Capability>,
}

impl CapabilityReplayScope {
    pub const fn all() -> Self {
        Self {
            module: None,
            capability: None,
        }
    }

    pub const fn module(module: ModuleId) -> Self {
        Self {
            module: Some(module),
            capability: None,
        }
    }

    pub const fn capability(capability: Capability) -> Self {
        Self {
            module: None,
            capability: Some(capability),
        }
    }

    pub const fn exact(module: ModuleId, capability: Capability) -> Self {
        Self {
            module: Some(module),
            capability: Some(capability),
        }
    }

    pub fn matches(self, record: CapabilityTraceRecord) -> bool {
        self.module
            .map(|module| module == record.module)
            .unwrap_or(true)
            && self
                .capability
                .map(|capability| capability == record.capability)
                .unwrap_or(true)
    }
}

impl Default for CapabilityReplayScope {
    fn default() -> Self {
        Self::all()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityTraceError {
    Unauthorized(CapabilityGrantError),
}

#[derive(Debug)]
pub struct CapabilityTrace<const N: usize> {
    records: [Option<CapabilityTraceRecord>; N],
    next: usize,
    len: usize,
    seq: u32,
    dropped: u32,
}

impl<const N: usize> CapabilityTrace<N> {
    pub const fn new() -> Self {
        Self {
            records: [None; N],
            next: 0,
            len: 0,
            seq: 0,
            dropped: 0,
        }
    }

    pub fn record_authorized<const G: usize>(
        &mut self,
        grants: &CapabilityGrantTable<G>,
        input: CapabilityTraceInput,
    ) -> Result<CapabilityTraceRecord, CapabilityTraceError> {
        grants
            .authorize(input.module, input.capability)
            .map_err(CapabilityTraceError::Unauthorized)?;

        Ok(self.record(input))
    }

    pub(crate) fn record(&mut self, input: CapabilityTraceInput) -> CapabilityTraceRecord {
        let record = CapabilityTraceRecord::from_input(self.seq, input);
        self.seq = self.seq.wrapping_add(1);

        if N == 0 {
            self.dropped = self.dropped.saturating_add(1);
            return record;
        }

        if self.len == N {
            self.dropped = self.dropped.saturating_add(1);
        } else {
            self.len += 1;
        }

        self.records[self.next] = Some(record);
        self.next = (self.next + 1) % N;
        record
    }

    pub fn copy_replay(
        &self,
        scope: CapabilityReplayScope,
        out: &mut [CapabilityTraceRecord],
    ) -> usize {
        let mut copied = 0;
        for i in 0..self.len {
            if copied == out.len() {
                break;
            }
            let index = self.index_in_replay_order(i);
            if let Some(record) = self.records[index] {
                if scope.matches(record) {
                    out[copied] = record;
                    copied += 1;
                }
            }
        }
        copied
    }

    pub fn matching_count(&self, scope: CapabilityReplayScope) -> usize {
        let mut count = 0;
        for i in 0..self.len {
            let index = self.index_in_replay_order(i);
            if let Some(record) = self.records[index] {
                if scope.matches(record) {
                    count += 1;
                }
            }
        }
        count
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub const fn dropped(&self) -> u32 {
        self.dropped
    }

    pub const fn next_sequence(&self) -> u32 {
        self.seq
    }

    fn index_in_replay_order(&self, offset: usize) -> usize {
        if self.len == N {
            (self.next + offset) % N
        } else {
            offset
        }
    }
}

impl<const N: usize> Default for CapabilityTrace<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct CapabilityGrantTable<const N: usize> {
    grants: [Option<CapabilityGrant>; N],
}

impl<const N: usize> CapabilityGrantTable<N> {
    pub const fn new() -> Self {
        Self { grants: [None; N] }
    }

    /// Initialize final grant storage from an already validated manifest.
    ///
    /// # Safety
    ///
    /// `destination` must be aligned, writable storage for one uninitialized
    /// `CapabilityGrantTable<N>`, and `manifest.len()` must not exceed `N`.
    pub(crate) unsafe fn init_from_manifest_in_place<const M: usize>(
        destination: *mut Self,
        manifest: &SystemManifest<M>,
    ) {
        let grants =
            core::ptr::addr_of_mut!((*destination).grants).cast::<Option<CapabilityGrant>>();
        for index in 0..N {
            grants.add(index).write(None);
        }
        for (index, spec) in manifest.iter().enumerate() {
            grants.add(index).write(Some(CapabilityGrant::new(
                spec.id,
                spec.requires.union(spec.owns),
            )));
        }
    }

    pub fn register(
        &mut self,
        module: ModuleId,
        granted: CapabilitySet,
    ) -> Result<(), CapabilityGrantError> {
        if self.find(module).is_some() {
            return Err(CapabilityGrantError::Duplicate(module));
        }

        let Some(slot) = self.grants.iter_mut().find(|slot| slot.is_none()) else {
            return Err(CapabilityGrantError::Full);
        };
        *slot = Some(CapabilityGrant::new(module, granted));
        Ok(())
    }

    pub fn from_manifest<const M: usize>(
        manifest: &SystemManifest<M>,
    ) -> Result<Self, CapabilityGrantError> {
        let mut table = Self::new();
        for spec in manifest.iter() {
            table.register(spec.id, spec.requires.union(spec.owns))?;
        }
        Ok(table)
    }

    pub fn authorize(
        &self,
        module: ModuleId,
        capability: Capability,
    ) -> Result<(), CapabilityGrantError> {
        let Some(grant) = self.find(module) else {
            return Err(CapabilityGrantError::Missing(module));
        };
        if grant.granted.contains(capability) {
            Ok(())
        } else {
            Err(CapabilityGrantError::Denied { module, capability })
        }
    }

    pub fn granted(&self, module: ModuleId) -> Option<CapabilitySet> {
        self.find(module).map(|grant| grant.granted)
    }

    pub fn len(&self) -> usize {
        self.grants.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn find(&self, module: ModuleId) -> Option<&CapabilityGrant> {
        self.grants
            .iter()
            .flatten()
            .find(|grant| grant.module == module)
    }
}

impl<const N: usize> Default for CapabilityGrantTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        kernel_owned_capabilities, Criticality, DeadlineContract, FaultThresholds, MemoryBudget,
        ModuleSpec, SystemManifest,
    };

    fn kernel_spec() -> ModuleSpec {
        ModuleSpec::new(ModuleId::Kernel, Criticality::HardRealtime)
            .owns(kernel_owned_capabilities())
            .memory(MemoryBudget::new(16 * 1024, 4 * 1024, 4))
            .deadline(DeadlineContract::new(20_000, 10))
            .fault_thresholds(FaultThresholds {
                notify_after: 2,
                reboot_after: 4,
            })
    }

    fn sensor_spec() -> ModuleSpec {
        ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
            .requires(
                CapabilitySet::empty()
                    .with(Capability::Bus0)
                    .with(Capability::SamplePool),
            )
            .owns(CapabilitySet::empty().with(Capability::Bus0))
            .memory(MemoryBudget::new(8 * 1024, 2 * 1024, 2))
    }

    #[test]
    fn capability_gated_multi_module_admission() {
        // Two modules admitted with different grants: each may use what it owns and is
        // denied what it does not; an unregistered module is denied outright.
        let mut table = CapabilityGrantTable::<2>::new();
        table
            .register(
                ModuleId::Sensor,
                CapabilitySet::empty().with(Capability::Bus0),
            )
            .unwrap();
        table
            .register(
                ModuleId::Radio,
                CapabilitySet::empty().with(Capability::Radio),
            )
            .unwrap();

        assert_eq!(table.authorize(ModuleId::Sensor, Capability::Bus0), Ok(()));
        assert!(table
            .authorize(ModuleId::Sensor, Capability::Radio)
            .is_err());
        assert_eq!(table.authorize(ModuleId::Radio, Capability::Radio), Ok(()));
        assert!(table.authorize(ModuleId::Radio, Capability::Bus0).is_err());
        assert_eq!(
            table.authorize(ModuleId::Actuator, Capability::Bus0),
            Err(CapabilityGrantError::Missing(ModuleId::Actuator))
        );
    }

    #[test]
    fn grant_table_authorizes_declared_capabilities() {
        let manifest = SystemManifest::<2>::from_specs(&[kernel_spec(), sensor_spec()]).unwrap();
        let grants = CapabilityGrantTable::<2>::from_manifest(&manifest).unwrap();

        assert_eq!(grants.len(), 2);
        assert_eq!(grants.authorize(ModuleId::Sensor, Capability::Bus0), Ok(()));
        assert_eq!(
            grants.authorize(ModuleId::Sensor, Capability::SamplePool),
            Ok(())
        );
    }

    #[test]
    fn grant_table_denies_undeclared_capability() {
        let manifest = SystemManifest::<2>::from_specs(&[kernel_spec(), sensor_spec()]).unwrap();
        let grants = CapabilityGrantTable::<2>::from_manifest(&manifest).unwrap();

        assert_eq!(
            grants.authorize(ModuleId::Sensor, Capability::Radio),
            Err(CapabilityGrantError::Denied {
                module: ModuleId::Sensor,
                capability: Capability::Radio,
            })
        );
    }

    #[test]
    fn grant_table_reports_missing_module() {
        let grants = CapabilityGrantTable::<1>::new();

        assert_eq!(
            grants.authorize(ModuleId::App(4), Capability::HostReport),
            Err(CapabilityGrantError::Missing(ModuleId::App(4)))
        );
    }

    #[test]
    fn grant_table_preserves_duplicate_errors() {
        let mut grants = CapabilityGrantTable::<2>::new();
        grants
            .register(ModuleId::Kernel, kernel_owned_capabilities())
            .unwrap();

        assert_eq!(
            grants.register(ModuleId::Kernel, CapabilitySet::empty()),
            Err(CapabilityGrantError::Duplicate(ModuleId::Kernel))
        );
    }

    #[test]
    fn capability_trace_records_only_authorized_operations() {
        let mut grants = CapabilityGrantTable::<1>::new();
        grants
            .register(
                ModuleId::Sensor,
                CapabilitySet::empty().with(Capability::Bus0),
            )
            .unwrap();
        let mut trace = CapabilityTrace::<4>::new();

        let record = trace
            .record_authorized(
                &grants,
                CapabilityTraceInput::new(
                    ModuleId::Sensor,
                    Capability::Bus0,
                    CapabilityTraceOp::Read,
                    100,
                )
                .args(0x68, 6),
            )
            .unwrap();

        assert_eq!(record.seq, 0);
        assert_eq!(trace.len(), 1);
        assert_eq!(
            trace.record_authorized(
                &grants,
                CapabilityTraceInput::new(
                    ModuleId::Sensor,
                    Capability::Radio,
                    CapabilityTraceOp::Invoke,
                    120,
                ),
            ),
            Err(CapabilityTraceError::Unauthorized(
                CapabilityGrantError::Denied {
                    module: ModuleId::Sensor,
                    capability: Capability::Radio,
                }
            ))
        );
        assert_eq!(trace.len(), 1);
    }

    #[test]
    fn capability_trace_replays_in_sequence_order() {
        let mut grants = CapabilityGrantTable::<2>::new();
        grants
            .register(
                ModuleId::Sensor,
                CapabilitySet::empty().with(Capability::Bus0),
            )
            .unwrap();
        grants
            .register(
                ModuleId::Radio,
                CapabilitySet::empty().with(Capability::Radio),
            )
            .unwrap();
        let mut trace = CapabilityTrace::<3>::new();

        for i in 0..5 {
            let (module, capability) = if i % 2 == 0 {
                (ModuleId::Sensor, Capability::Bus0)
            } else {
                (ModuleId::Radio, Capability::Radio)
            };
            trace
                .record_authorized(
                    &grants,
                    CapabilityTraceInput::new(
                        module,
                        capability,
                        CapabilityTraceOp::Invoke,
                        1_000 + u64::from(i),
                    )
                    .args(i, i + 10),
                )
                .unwrap();
        }

        let mut out = [CapabilityTraceRecord::EMPTY; 3];
        let copied = trace.copy_replay(CapabilityReplayScope::all(), &mut out);

        assert_eq!(copied, 3);
        assert_eq!(trace.dropped(), 2);
        assert_eq!(out[0].seq, 2);
        assert_eq!(out[1].seq, 3);
        assert_eq!(out[2].seq, 4);
    }

    #[test]
    fn capability_trace_filters_replay_scope() {
        let mut grants = CapabilityGrantTable::<2>::new();
        grants
            .register(
                ModuleId::Sensor,
                CapabilitySet::empty()
                    .with(Capability::Bus0)
                    .with(Capability::SamplePool),
            )
            .unwrap();
        grants
            .register(
                ModuleId::Radio,
                CapabilitySet::empty().with(Capability::Radio),
            )
            .unwrap();
        let mut trace = CapabilityTrace::<4>::new();
        trace
            .record_authorized(
                &grants,
                CapabilityTraceInput::new(
                    ModuleId::Sensor,
                    Capability::Bus0,
                    CapabilityTraceOp::Read,
                    10,
                )
                .args(1, 2),
            )
            .unwrap();
        trace
            .record_authorized(
                &grants,
                CapabilityTraceInput::new(
                    ModuleId::Sensor,
                    Capability::SamplePool,
                    CapabilityTraceOp::Write,
                    20,
                )
                .args(3, 4),
            )
            .unwrap();
        trace
            .record_authorized(
                &grants,
                CapabilityTraceInput::new(
                    ModuleId::Radio,
                    Capability::Radio,
                    CapabilityTraceOp::Invoke,
                    30,
                )
                .args(5, 6),
            )
            .unwrap();

        let mut out = [CapabilityTraceRecord::EMPTY; 2];

        assert_eq!(
            trace.matching_count(CapabilityReplayScope::module(ModuleId::Sensor)),
            2
        );
        let copied = trace.copy_replay(
            CapabilityReplayScope::exact(ModuleId::Sensor, Capability::SamplePool),
            &mut out,
        );
        assert_eq!(copied, 1);
        assert_eq!(out[0].module, ModuleId::Sensor);
        assert_eq!(out[0].capability, Capability::SamplePool);
        assert_eq!(out[0].op, CapabilityTraceOp::Write);
    }

    #[test]
    fn zero_capacity_capability_trace_keeps_drop_accounting() {
        let mut grants = CapabilityGrantTable::<1>::new();
        grants
            .register(
                ModuleId::Sensor,
                CapabilitySet::empty().with(Capability::Bus0),
            )
            .unwrap();
        let mut trace = CapabilityTrace::<0>::new();

        let record = trace
            .record_authorized(
                &grants,
                CapabilityTraceInput::new(
                    ModuleId::Sensor,
                    Capability::Bus0,
                    CapabilityTraceOp::Read,
                    1,
                )
                .args(2, 3)
                .result(4),
            )
            .unwrap();

        assert_eq!(record.seq, 0);
        assert_eq!(trace.len(), 0);
        assert_eq!(trace.dropped(), 1);
        assert_eq!(trace.next_sequence(), 1);
    }
}
