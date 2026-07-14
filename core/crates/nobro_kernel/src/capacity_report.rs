//! Opt-in, fail-closed capacity campaign reports.
//!
//! The producer observes only resources with real kernel evidence today:
//! guarded stack high-water marks, the kernel mailbox, and the static sample
//! pool.  Other async queues are deliberately outside this report version.
//! Reports are caller-owned and this module is available only with the
//! `capacity-report` feature, keeping ordinary kernel builds unchanged.

use portable_atomic::{AtomicU32, Ordering};

use crate::{
    mailbox::MailboxCapacitySnapshot, pool::PoolCapacitySnapshot, Mailbox, ModuleId, SamplePool,
    StackGuardTable, SAMPLE_POOL_SIZE,
};

pub const CAPACITY_REPORT_MAGIC: u32 = 0x4E42_5243; // "NBRC"
pub const CAPACITY_REPORT_VERSION: u32 = 1;
pub const CAPACITY_RESOURCE_RECORD_BYTES: usize = 60;
pub const CAPACITY_REPORT_FIXED_BYTES: usize = 184;

pub const CAPACITY_FLAG_IDENTITY_MISSING: u32 = 1;
pub const CAPACITY_FLAG_SESSION_MISMATCH: u32 = 1 << 1;
pub const CAPACITY_FLAG_RESOURCE_MISSING: u32 = 1 << 2;
pub const CAPACITY_FLAG_DECLARATION_MISMATCH: u32 = 1 << 3;
pub const CAPACITY_FLAG_UNEXPECTED_PATH: u32 = 1 << 4;
pub const CAPACITY_FLAG_INCOMPLETE: u32 = 1 << 5;
pub const CAPACITY_FLAG_SIZE_OVERFLOW: u32 = 1 << 6;

const FNV1A32_OFFSET: u32 = 0x811C_9DC5;
const FNV1A32_PRIME: u32 = 0x0100_0193;
const OBSERVATION_CLOSED: u32 = 1 << 31;
const OBSERVATION_COUNT_MASK: u32 = OBSERVATION_CLOSED - 1;

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityResourceKind {
    StackBytes = 1,
    QueueSlots = 2,
    PoolSlots = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapacityResourceSource {
    Stack(ModuleId),
    Mailbox,
    SamplePool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapacityResource {
    resource_id: [u8; 32],
    kind: CapacityResourceKind,
    declared: u32,
    granularity: u32,
    source: CapacityResourceSource,
}

impl CapacityResource {
    pub const fn stack(
        resource_id: [u8; 32],
        module: ModuleId,
        declared_bytes: u32,
        granularity_bytes: u32,
    ) -> Self {
        Self {
            resource_id,
            kind: CapacityResourceKind::StackBytes,
            declared: declared_bytes,
            granularity: granularity_bytes,
            source: CapacityResourceSource::Stack(module),
        }
    }

    pub const fn mailbox(
        resource_id: [u8; 32],
        declared_slots: u32,
        granularity_slots: u32,
    ) -> Self {
        Self {
            resource_id,
            kind: CapacityResourceKind::QueueSlots,
            declared: declared_slots,
            granularity: granularity_slots,
            source: CapacityResourceSource::Mailbox,
        }
    }

    pub const fn sample_pool(
        resource_id: [u8; 32],
        declared_slots: u32,
        granularity_slots: u32,
    ) -> Self {
        Self {
            resource_id,
            kind: CapacityResourceKind::PoolSlots,
            declared: declared_slots,
            granularity: granularity_slots,
            source: CapacityResourceSource::SamplePool,
        }
    }

    pub const fn resource_id(&self) -> &[u8; 32] {
        &self.resource_id
    }

    pub const fn kind(&self) -> CapacityResourceKind {
        self.kind
    }

    pub const fn declared(&self) -> u32 {
        self.declared
    }

    pub const fn granularity(&self) -> u32 {
        self.granularity
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityRegistryError {
    Full,
    ZeroIdentity,
    ZeroDeclaration,
    ZeroGranularity,
    DuplicateResource,
    DuplicateSource,
}

pub struct CapacityRegistry<const N: usize> {
    entries: [Option<CapacityResource>; N],
    len: usize,
}

impl<const N: usize> CapacityRegistry<N> {
    pub const fn new() -> Self {
        Self {
            entries: [None; N],
            len: 0,
        }
    }

    pub fn register(&mut self, resource: CapacityResource) -> Result<(), CapacityRegistryError> {
        if resource.resource_id.iter().all(|byte| *byte == 0) {
            return Err(CapacityRegistryError::ZeroIdentity);
        }
        if resource.declared == 0 {
            return Err(CapacityRegistryError::ZeroDeclaration);
        }
        if resource.granularity == 0 {
            return Err(CapacityRegistryError::ZeroGranularity);
        }
        if self
            .entries
            .iter()
            .flatten()
            .any(|entry| entry.resource_id == resource.resource_id)
        {
            return Err(CapacityRegistryError::DuplicateResource);
        }
        if self
            .entries
            .iter()
            .flatten()
            .any(|entry| entry.source == resource.source)
        {
            return Err(CapacityRegistryError::DuplicateSource);
        }
        let Some(slot) = self.entries.iter_mut().find(|entry| entry.is_none()) else {
            return Err(CapacityRegistryError::Full);
        };
        *slot = Some(resource);
        self.len += 1;
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = CapacityResource> + '_ {
        self.entries.iter().flatten().copied()
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<const N: usize> Default for CapacityRegistry<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapacityIdentity {
    pub build_id: [u8; 32],
    pub workload_id: [u8; 32],
    pub declaration_id: [u8; 32],
}

impl CapacityIdentity {
    pub const fn new(build_id: [u8; 32], workload_id: [u8; 32], declaration_id: [u8; 32]) -> Self {
        Self {
            build_id,
            workload_id,
            declaration_id,
        }
    }

    pub fn is_complete(self) -> bool {
        !is_zero_identity(&self.build_id)
            && !is_zero_identity(&self.workload_id)
            && !is_zero_identity(&self.declaration_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapacityCampaignConfig {
    pub identity: CapacityIdentity,
    pub session_id: u32,
    pub margin_percent: u32,
    pub unseen_path_reserve_percent: u32,
    pub required_paths: u64,
    pub isr_paths: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityCampaignError {
    ZeroSession,
    ReusedSession,
    IdentityMissing,
    InvalidMargin,
    InvalidReserve,
    NoRequiredPaths,
    IsrPathNotRequired,
    InvalidPath,
    UndeclaredPath,
    CampaignFinished,
    SessionStartRejected,
}

/// One real observation session. Path marks are atomic so ISR sites can record
/// their own execution without borrowing campaign state from the main loop.
pub struct CapacityCampaign {
    identity: CapacityIdentity,
    session_id: u32,
    margin_percent: u32,
    unseen_path_reserve_percent: u32,
    required_paths: u64,
    isr_paths: u64,
    observed_paths_lo: AtomicU32,
    observed_paths_hi: AtomicU32,
    observed_isr_paths_lo: AtomicU32,
    observed_isr_paths_hi: AtomicU32,
    observation_state: AtomicU32,
    flags: AtomicU32,
}

impl CapacityCampaign {
    pub fn start<const MAILBOX: usize>(
        config: CapacityCampaignConfig,
        mailbox: &mut Mailbox<MAILBOX>,
    ) -> Result<Self, CapacityCampaignError> {
        validate_config(config)?;
        if !mailbox.can_start_capacity_session(config.session_id)
            || !SamplePool::try_start_capacity_session(config.session_id)
        {
            return Err(CapacityCampaignError::SessionStartRejected);
        }
        // The exclusive mailbox borrow makes the successful preflight stable;
        // claiming the shared pool first keeps startup transactional when
        // another campaign already owns that global recorder.
        mailbox.start_capacity_session(config.session_id);
        Ok(Self {
            identity: config.identity,
            session_id: config.session_id,
            margin_percent: config.margin_percent,
            unseen_path_reserve_percent: config.unseen_path_reserve_percent,
            required_paths: config.required_paths,
            isr_paths: config.isr_paths,
            observed_paths_lo: AtomicU32::new(0),
            observed_paths_hi: AtomicU32::new(0),
            observed_isr_paths_lo: AtomicU32::new(0),
            observed_isr_paths_hi: AtomicU32::new(0),
            observation_state: AtomicU32::new(0),
            flags: AtomicU32::new(0),
        })
    }

    pub fn reset<const MAILBOX: usize>(
        &mut self,
        session_id: u32,
        mailbox: &mut Mailbox<MAILBOX>,
    ) -> Result<(), CapacityCampaignError> {
        if session_id == 0 {
            return Err(CapacityCampaignError::ZeroSession);
        }
        if session_id <= self.session_id {
            return Err(CapacityCampaignError::ReusedSession);
        }
        if !mailbox.can_restart_capacity_session(self.session_id, session_id)
            || !SamplePool::try_restart_capacity_session(self.session_id, session_id)
        {
            return Err(CapacityCampaignError::SessionStartRejected);
        }
        mailbox.restart_capacity_session(session_id);
        self.session_id = session_id;
        self.observed_paths_lo.store(0, Ordering::Relaxed);
        self.observed_paths_hi.store(0, Ordering::Relaxed);
        self.observed_isr_paths_lo.store(0, Ordering::Relaxed);
        self.observed_isr_paths_hi.store(0, Ordering::Relaxed);
        self.observation_state.store(0, Ordering::Release);
        self.flags.store(0, Ordering::Relaxed);
        Ok(())
    }

    pub fn observe_path(&self, path: u8) -> Result<(), CapacityCampaignError> {
        self.observe(path, false)
    }

    pub fn observe_isr_path(&self, path: u8) -> Result<(), CapacityCampaignError> {
        self.observe(path, true)
    }

    /// Seal one campaign before its report is collected.
    ///
    /// # Safety
    /// Every registered stack context and every sample-pool producer must be
    /// quiesced before this call and remain quiesced until `report` returns.
    /// The mailbox is exclusively borrowed and both mutation metrics are
    /// sealed here; any later safe mailbox/pool mutation before collection is
    /// detected and makes the report incomplete.
    pub unsafe fn finish<const MAILBOX: usize>(
        &self,
        mailbox: &mut Mailbox<MAILBOX>,
    ) -> Result<(), CapacityCampaignError> {
        let previous = self
            .observation_state
            .fetch_or(OBSERVATION_CLOSED, Ordering::AcqRel);
        if previous & OBSERVATION_CLOSED != 0 {
            self.flags
                .fetch_or(CAPACITY_FLAG_INCOMPLETE, Ordering::Relaxed);
            return Err(CapacityCampaignError::CampaignFinished);
        }
        if previous & OBSERVATION_COUNT_MASK != 0 {
            self.flags
                .fetch_or(CAPACITY_FLAG_INCOMPLETE, Ordering::Relaxed);
        }
        if !mailbox.finish_capacity_session(self.session_id)
            || !SamplePool::finish_capacity_session(self.session_id)
        {
            self.flags
                .fetch_or(CAPACITY_FLAG_SESSION_MISMATCH, Ordering::Relaxed);
        }
        Ok(())
    }

    pub fn report<const RESOURCES: usize, const MAILBOX: usize, const GUARDS: usize>(
        &self,
        registry: &CapacityRegistry<RESOURCES>,
        mailbox: &Mailbox<MAILBOX>,
        guards: &StackGuardTable<GUARDS>,
    ) -> CapacityReport<RESOURCES> {
        CapacityReport::collect(self, registry, mailbox, guards)
    }

    fn observe(&self, path: u8, isr: bool) -> Result<(), CapacityCampaignError> {
        if self.observation_state.load(Ordering::Acquire) & OBSERVATION_CLOSED != 0 {
            self.flags
                .fetch_or(CAPACITY_FLAG_UNEXPECTED_PATH, Ordering::Relaxed);
            return Err(CapacityCampaignError::CampaignFinished);
        }
        if path >= 64 {
            self.flags
                .fetch_or(CAPACITY_FLAG_UNEXPECTED_PATH, Ordering::Relaxed);
            return Err(CapacityCampaignError::InvalidPath);
        }
        let bit = 1_u64 << path;
        if self.required_paths & bit == 0 || (isr && self.isr_paths & bit == 0) {
            self.flags
                .fetch_or(CAPACITY_FLAG_UNEXPECTED_PATH, Ordering::Relaxed);
            return Err(CapacityCampaignError::UndeclaredPath);
        }
        if !begin_observation(&self.observation_state) {
            self.flags
                .fetch_or(CAPACITY_FLAG_INCOMPLETE, Ordering::Relaxed);
            return Err(CapacityCampaignError::CampaignFinished);
        }
        set_mask_bit(&self.observed_paths_lo, &self.observed_paths_hi, path);
        if isr {
            set_mask_bit(
                &self.observed_isr_paths_lo,
                &self.observed_isr_paths_hi,
                path,
            );
        }
        self.observation_state.fetch_sub(1, Ordering::Release);
        Ok(())
    }
}

fn validate_config(config: CapacityCampaignConfig) -> Result<(), CapacityCampaignError> {
    if config.session_id == 0 {
        return Err(CapacityCampaignError::ZeroSession);
    }
    if !config.identity.is_complete() {
        return Err(CapacityCampaignError::IdentityMissing);
    }
    if config.margin_percent > 1_000 {
        return Err(CapacityCampaignError::InvalidMargin);
    }
    if config.unseen_path_reserve_percent == 0 || config.unseen_path_reserve_percent > 1_000 {
        return Err(CapacityCampaignError::InvalidReserve);
    }
    if config.required_paths == 0 {
        return Err(CapacityCampaignError::NoRequiredPaths);
    }
    if config.isr_paths & !config.required_paths != 0 {
        return Err(CapacityCampaignError::IsrPathNotRequired);
    }
    Ok(())
}

fn set_mask_bit(lo: &AtomicU32, hi: &AtomicU32, path: u8) {
    if path < 32 {
        lo.fetch_or(1_u32 << path, Ordering::Relaxed);
    } else {
        hi.fetch_or(1_u32 << (path - 32), Ordering::Relaxed);
    }
}

fn begin_observation(state: &AtomicU32) -> bool {
    let mut current = state.load(Ordering::Acquire);
    loop {
        if current & OBSERVATION_CLOSED != 0
            || current & OBSERVATION_COUNT_MASK == OBSERVATION_COUNT_MASK
        {
            return false;
        }
        match state.compare_exchange_weak(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => return true,
            Err(actual) => current = actual,
        }
    }
}

fn atomic_mask(lo: &AtomicU32, hi: &AtomicU32) -> u64 {
    u64::from(lo.load(Ordering::Acquire)) | (u64::from(hi.load(Ordering::Acquire)) << 32)
}

fn is_zero_identity(identity: &[u8; 32]) -> bool {
    identity.iter().all(|byte| *byte == 0)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapacityResourceRecord {
    pub resource_id: [u8; 32],
    pub kind: u32,
    pub declared: u32,
    pub observed_peak: u32,
    pub granularity: u32,
    pub saturated: u32,
    pub dropped: u32,
    pub failure_count: u32,
}

impl CapacityResourceRecord {
    pub const fn zeroed() -> Self {
        Self {
            resource_id: [0; 32],
            kind: 0,
            declared: 0,
            observed_peak: 0,
            granularity: 0,
            saturated: 0,
            dropped: 0,
            failure_count: 0,
        }
    }

    fn from_resource(resource: CapacityResource) -> Self {
        Self {
            resource_id: resource.resource_id,
            kind: resource.kind as u32,
            declared: resource.declared,
            observed_peak: 0,
            granularity: resource.granularity,
            saturated: 0,
            dropped: 0,
            failure_count: 0,
        }
    }
}

/// Little-endian fixed-layout report. `resource_capacity` and `report_bytes`
/// make the const-generic tail self-describing to a host decoder.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapacityReport<const N: usize> {
    pub magic: u32,
    pub version: u32,
    pub report_bytes: u32,
    pub completed: u32,
    pub flags: u32,
    pub session_id: u32,
    pub resource_count: u32,
    pub resource_capacity: u32,
    pub margin_percent: u32,
    pub unseen_path_reserve_percent: u32,
    pub coverage_finished: u32,
    pub workload_complete: u32,
    pub isr_paths_covered: u32,
    pub required_paths_lo: u32,
    pub required_paths_hi: u32,
    pub observed_paths_lo: u32,
    pub observed_paths_hi: u32,
    pub required_isr_paths_lo: u32,
    pub required_isr_paths_hi: u32,
    pub observed_isr_paths_lo: u32,
    pub observed_isr_paths_hi: u32,
    pub build_id: [u8; 32],
    pub workload_id: [u8; 32],
    pub declaration_id: [u8; 32],
    pub resources: [CapacityResourceRecord; N],
    pub checksum: u32,
}

impl<const N: usize> CapacityReport<N> {
    fn collect<const MAILBOX: usize, const GUARDS: usize>(
        campaign: &CapacityCampaign,
        registry: &CapacityRegistry<N>,
        mailbox: &Mailbox<MAILBOX>,
        guards: &StackGuardTable<GUARDS>,
    ) -> Self {
        let observed_paths = atomic_mask(&campaign.observed_paths_lo, &campaign.observed_paths_hi);
        let observed_isr_paths = atomic_mask(
            &campaign.observed_isr_paths_lo,
            &campaign.observed_isr_paths_hi,
        );
        let observation_state = campaign.observation_state.load(Ordering::Acquire);
        let finished = observation_state & OBSERVATION_CLOSED != 0;
        let mut flags = campaign.flags.load(Ordering::Acquire);
        if !campaign.identity.is_complete() {
            flags |= CAPACITY_FLAG_IDENTITY_MISSING;
        }
        if !finished || registry.is_empty() {
            flags |= CAPACITY_FLAG_INCOMPLETE;
        }
        if observation_state & OBSERVATION_COUNT_MASK != 0 {
            flags |= CAPACITY_FLAG_INCOMPLETE;
        }
        let mailbox_snapshot = mailbox.capacity_snapshot();
        let pool_snapshot = SamplePool::capacity_snapshot();
        if mailbox_snapshot.session_id != campaign.session_id
            || pool_snapshot.session_id != campaign.session_id
        {
            flags |= CAPACITY_FLAG_SESSION_MISMATCH;
        }
        if !mailbox_snapshot.sealed
            || mailbox_snapshot.activity_after_finish
            || !pool_snapshot.sealed
            || pool_snapshot.activity_after_finish
        {
            flags |= CAPACITY_FLAG_INCOMPLETE;
        }

        let (report_bytes, size_overflow) = bounded_u32(core::mem::size_of::<Self>());
        let (resource_count, count_overflow) = bounded_u32(registry.len());
        let (resource_capacity, capacity_overflow) = bounded_u32(N);
        if size_overflow || count_overflow || capacity_overflow {
            flags |= CAPACITY_FLAG_SIZE_OVERFLOW;
        }

        let mut report = Self {
            magic: CAPACITY_REPORT_MAGIC,
            version: CAPACITY_REPORT_VERSION,
            report_bytes,
            completed: 0,
            flags,
            session_id: campaign.session_id,
            resource_count,
            resource_capacity,
            margin_percent: campaign.margin_percent,
            unseen_path_reserve_percent: campaign.unseen_path_reserve_percent,
            coverage_finished: u32::from(finished),
            workload_complete: u32::from(
                finished && observed_paths & campaign.required_paths == campaign.required_paths,
            ),
            isr_paths_covered: u32::from(
                finished && observed_isr_paths & campaign.isr_paths == campaign.isr_paths,
            ),
            required_paths_lo: campaign.required_paths as u32,
            required_paths_hi: (campaign.required_paths >> 32) as u32,
            observed_paths_lo: observed_paths as u32,
            observed_paths_hi: (observed_paths >> 32) as u32,
            required_isr_paths_lo: campaign.isr_paths as u32,
            required_isr_paths_hi: (campaign.isr_paths >> 32) as u32,
            observed_isr_paths_lo: observed_isr_paths as u32,
            observed_isr_paths_hi: (observed_isr_paths >> 32) as u32,
            build_id: campaign.identity.build_id,
            workload_id: campaign.identity.workload_id,
            declaration_id: campaign.identity.declaration_id,
            resources: [CapacityResourceRecord::zeroed(); N],
            checksum: 0,
        };

        // Never inspect a live stack (or any other resource source) from a
        // premature report call. Only a successfully sealed, quiescent,
        // exact-session campaign may proceed to resource collection.
        let preflight_failures = CAPACITY_FLAG_IDENTITY_MISSING
            | CAPACITY_FLAG_SESSION_MISMATCH
            | CAPACITY_FLAG_UNEXPECTED_PATH
            | CAPACITY_FLAG_INCOMPLETE
            | CAPACITY_FLAG_SIZE_OVERFLOW;
        if report.flags & preflight_failures != 0 {
            report.checksum = report.compute_checksum();
            return report;
        }

        for (index, resource) in registry.iter().enumerate() {
            let mut record = CapacityResourceRecord::from_resource(resource);
            match resource.source {
                CapacityResourceSource::Stack(module) => {
                    let Some(status) = guards.status(module) else {
                        report.flags |= CAPACITY_FLAG_RESOURCE_MISSING;
                        report.resources[index] = record;
                        continue;
                    };
                    let (stack_len, len_saturated) = bounded_u32(status.len);
                    let (used, used_saturated) = bounded_u32(status.used_bytes);
                    record.observed_peak = used;
                    record.saturated =
                        u32::from(len_saturated || used_saturated || !status.canary_intact);
                    record.failure_count = u32::from(!status.canary_intact);
                    if stack_len != resource.declared {
                        report.flags |= CAPACITY_FLAG_DECLARATION_MISMATCH;
                    }
                }
                CapacityResourceSource::Mailbox => {
                    apply_snapshot(&mut record, mailbox_snapshot);
                    let (capacity, saturated) = bounded_u32(MAILBOX);
                    record.saturated |= u32::from(saturated);
                    if capacity != resource.declared {
                        report.flags |= CAPACITY_FLAG_DECLARATION_MISMATCH;
                    }
                }
                CapacityResourceSource::SamplePool => {
                    apply_pool_snapshot(&mut record, pool_snapshot);
                    let (capacity, saturated) = bounded_u32(SAMPLE_POOL_SIZE);
                    record.saturated |= u32::from(saturated);
                    if capacity != resource.declared {
                        report.flags |= CAPACITY_FLAG_DECLARATION_MISMATCH;
                    }
                }
            }
            report.resources[index] = record;
        }

        let structural_failures = CAPACITY_FLAG_IDENTITY_MISSING
            | CAPACITY_FLAG_SESSION_MISMATCH
            | CAPACITY_FLAG_RESOURCE_MISSING
            | CAPACITY_FLAG_DECLARATION_MISMATCH
            | CAPACITY_FLAG_UNEXPECTED_PATH
            | CAPACITY_FLAG_INCOMPLETE
            | CAPACITY_FLAG_SIZE_OVERFLOW;
        report.completed = u32::from(report.flags & structural_failures == 0);
        report.checksum = report.compute_checksum();
        report
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == CAPACITY_REPORT_MAGIC
            && self.version == CAPACITY_REPORT_VERSION
            && usize::try_from(self.report_bytes).ok() == Some(core::mem::size_of::<Self>())
            && self.checksum == self.compute_checksum()
    }

    pub fn is_convertible(&self) -> bool {
        self.verify_checksum() && self.completed == 1
    }

    /// View the initialized, padding-free report as its transport bytes.
    /// All supported NobroRTOS targets are little-endian; host decoders must
    /// reject any other byte order rather than guessing.
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: every field is initialized, every field is four-byte aligned
        // or a byte array with a four-byte-multiple length, and both fixed and
        // resource-record size tests guard the padding-free layout.
        unsafe {
            core::slice::from_raw_parts(
                (self as *const Self).cast::<u8>(),
                core::mem::size_of::<Self>(),
            )
        }
    }

    fn compute_checksum(&self) -> u32 {
        let bytes = self.as_bytes();
        bytes[..bytes.len() - core::mem::size_of::<u32>()]
            .iter()
            .fold(FNV1A32_OFFSET, |hash, byte| {
                (hash ^ u32::from(*byte)).wrapping_mul(FNV1A32_PRIME)
            })
    }
}

fn apply_snapshot(record: &mut CapacityResourceRecord, snapshot: MailboxCapacitySnapshot) {
    record.observed_peak = snapshot.observed_peak;
    record.failure_count = snapshot.failures;
    record.dropped = u32::from(snapshot.failures != 0);
    record.saturated = u32::from(snapshot.saturated);
}

fn apply_pool_snapshot(record: &mut CapacityResourceRecord, snapshot: PoolCapacitySnapshot) {
    record.observed_peak = snapshot.observed_peak;
    record.failure_count = snapshot.failures;
    record.dropped = u32::from(snapshot.failures != 0);
    record.saturated = u32::from(snapshot.saturated);
}

fn bounded_u32(value: usize) -> (u32, bool) {
    match u32::try_from(value) {
        Ok(value) => (value, false),
        Err(_) => (u32::MAX, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        pool::{reset_test_pool, test_pool_guard},
        Message, MessageKind, SampleKind, StackRegion,
    };

    fn identity() -> CapacityIdentity {
        CapacityIdentity::new([0x11; 32], [0x22; 32], [0x33; 32])
    }

    fn config(session_id: u32) -> CapacityCampaignConfig {
        CapacityCampaignConfig {
            identity: identity(),
            session_id,
            margin_percent: 25,
            unseen_path_reserve_percent: 10,
            required_paths: 0b11,
            isr_paths: 0b10,
        }
    }

    fn message(value: u32) -> Message {
        Message::new(
            ModuleId::Sensor,
            ModuleId::App(1),
            MessageKind::Notification,
            value,
            0,
        )
    }

    #[test]
    fn report_contains_real_stack_mailbox_and_pool_peaks() {
        let _pool_guard = test_pool_guard();
        reset_test_pool();

        let mut mailbox = Mailbox::<2>::new();
        let campaign = CapacityCampaign::start(config(7), &mut mailbox).unwrap();

        mailbox.push(message(1)).unwrap();
        mailbox.push(message(2)).unwrap();
        assert_eq!(mailbox.push(message(3)), Err(crate::MailboxError::Full));

        let mut handles = [crate::PoolHandle::INVALID; SAMPLE_POOL_SIZE];
        for handle in &mut handles {
            *handle = SamplePool::alloc(SampleKind::Raw, 1, 0, 0).unwrap().handle;
        }
        assert!(SamplePool::alloc(SampleKind::Raw, 1, 0, 0).is_none());

        let mut stack = [0_u8; 128];
        let mut guards = StackGuardTable::<1>::new();
        unsafe {
            guards
                .register(
                    ModuleId::Sensor,
                    StackRegion {
                        base: stack.as_mut_ptr() as usize,
                        len: stack.len(),
                        canary_bytes: 16,
                    },
                )
                .unwrap();
            core::ptr::write_volatile(stack.as_mut_ptr().add(120), 0xA5);
        }

        campaign.observe_path(0).unwrap();
        campaign.observe_isr_path(1).unwrap();
        unsafe { campaign.finish(&mut mailbox) }.unwrap();

        let mut registry = CapacityRegistry::<3>::new();
        registry
            .register(CapacityResource::stack([1; 32], ModuleId::Sensor, 128, 8))
            .unwrap();
        registry
            .register(CapacityResource::mailbox([2; 32], 2, 1))
            .unwrap();
        registry
            .register(CapacityResource::sample_pool(
                [3; 32],
                SAMPLE_POOL_SIZE as u32,
                1,
            ))
            .unwrap();

        let report = campaign.report(&registry, &mailbox, &guards);
        assert!(report.is_convertible());
        assert_eq!(report.workload_complete, 1);
        assert_eq!(report.isr_paths_covered, 1);
        assert_eq!(report.resources[0].observed_peak, 8);
        assert_eq!(report.resources[1].observed_peak, 2);
        assert_eq!(report.resources[1].failure_count, 1);
        assert_eq!(report.resources[1].dropped, 1);
        assert_eq!(report.resources[2].observed_peak, SAMPLE_POOL_SIZE as u32);
        assert_eq!(report.resources[2].failure_count, 1);
        assert_eq!(
            report.as_bytes().len(),
            CAPACITY_REPORT_FIXED_BYTES + 3 * CAPACITY_RESOURCE_RECORD_BYTES
        );

        for handle in handles {
            assert!(SamplePool::release(handle));
        }
        reset_test_pool();
    }

    #[test]
    fn session_mismatch_and_unexpected_path_fail_before_resource_collection() {
        let _pool_guard = test_pool_guard();
        reset_test_pool();
        let mut mailbox = Mailbox::<1>::new();
        let campaign = CapacityCampaign::start(config(10), &mut mailbox).unwrap();
        assert_eq!(
            campaign.observe_isr_path(0),
            Err(CapacityCampaignError::UndeclaredPath)
        );
        unsafe { campaign.finish(&mut mailbox) }.unwrap();
        mailbox.push(message(9)).unwrap();
        assert!(SamplePool::try_start_capacity_session(11));

        let mut registry = CapacityRegistry::<2>::new();
        registry
            .register(CapacityResource::stack([1; 32], ModuleId::Sensor, 128, 8))
            .unwrap();
        registry
            .register(CapacityResource::sample_pool(
                [2; 32],
                SAMPLE_POOL_SIZE as u32,
                1,
            ))
            .unwrap();
        let guards = StackGuardTable::<1>::new();
        let report = campaign.report(&registry, &mailbox, &guards);
        assert!(report.verify_checksum());
        assert!(!report.is_convertible());
        assert_ne!(report.flags & CAPACITY_FLAG_UNEXPECTED_PATH, 0);
        assert_ne!(report.flags & CAPACITY_FLAG_SESSION_MISMATCH, 0);
        assert_ne!(report.flags & CAPACITY_FLAG_INCOMPLETE, 0);
        assert!(report
            .resources
            .iter()
            .all(|record| *record == CapacityResourceRecord::zeroed()));
    }

    #[test]
    fn missing_stack_resource_fails_after_a_valid_preflight() {
        let _pool_guard = test_pool_guard();
        reset_test_pool();
        let mut mailbox = Mailbox::<1>::new();
        let campaign = CapacityCampaign::start(config(12), &mut mailbox).unwrap();
        campaign.observe_path(0).unwrap();
        campaign.observe_isr_path(1).unwrap();
        unsafe { campaign.finish(&mut mailbox) }.unwrap();

        let mut registry = CapacityRegistry::<1>::new();
        registry
            .register(CapacityResource::stack([1; 32], ModuleId::Sensor, 128, 8))
            .unwrap();
        let guards = StackGuardTable::<0>::new();
        let report = campaign.report(&registry, &mailbox, &guards);
        assert!(report.verify_checksum());
        assert!(!report.is_convertible());
        assert_ne!(report.flags & CAPACITY_FLAG_RESOURCE_MISSING, 0);
        assert_eq!(report.resources[0].resource_id, [1; 32]);
        assert_eq!(report.resources[0].observed_peak, 0);
    }

    #[test]
    fn registry_and_campaign_reject_ambiguous_or_weak_declarations() {
        let mut registry = CapacityRegistry::<2>::new();
        assert_eq!(
            registry.register(CapacityResource::mailbox([0; 32], 1, 1)),
            Err(CapacityRegistryError::ZeroIdentity)
        );
        registry
            .register(CapacityResource::mailbox([1; 32], 1, 1))
            .unwrap();
        assert_eq!(
            registry.register(CapacityResource::mailbox([2; 32], 1, 1)),
            Err(CapacityRegistryError::DuplicateSource)
        );
        assert_eq!(
            registry.register(CapacityResource::sample_pool([1; 32], 8, 1)),
            Err(CapacityRegistryError::DuplicateResource)
        );

        let mut mailbox = Mailbox::<1>::new();
        let mut invalid = config(1);
        invalid.identity.build_id = [0; 32];
        assert!(matches!(
            CapacityCampaign::start(invalid, &mut mailbox),
            Err(CapacityCampaignError::IdentityMissing)
        ));
        let mut invalid = config(1);
        invalid.isr_paths = 0b100;
        assert!(matches!(
            CapacityCampaign::start(invalid, &mut mailbox),
            Err(CapacityCampaignError::IsrPathNotRequired)
        ));
    }

    #[test]
    fn reset_requires_a_fresh_session_and_clears_evidence() {
        let _pool_guard = test_pool_guard();
        reset_test_pool();
        let mut mailbox = Mailbox::<1>::new();
        let mut campaign = CapacityCampaign::start(config(20), &mut mailbox).unwrap();
        mailbox.push(message(1)).unwrap();
        assert_eq!(
            campaign.reset(20, &mut mailbox),
            Err(CapacityCampaignError::ReusedSession)
        );
        campaign.reset(21, &mut mailbox).unwrap();
        assert_eq!(
            campaign.reset(20, &mut mailbox),
            Err(CapacityCampaignError::ReusedSession)
        );
        campaign.observe_path(0).unwrap();
        campaign.observe_isr_path(1).unwrap();
        unsafe { campaign.finish(&mut mailbox) }.unwrap();

        let mut registry = CapacityRegistry::<1>::new();
        registry
            .register(CapacityResource::mailbox([1; 32], 1, 1))
            .unwrap();
        let guards = StackGuardTable::<0>::new();
        let mut report = campaign.report(&registry, &mailbox, &guards);
        assert!(report.is_convertible());
        assert_eq!(report.session_id, 21);
        assert_eq!(report.resources[0].observed_peak, 1);
        report.resources[0].observed_peak ^= 1;
        assert!(!report.verify_checksum());
    }

    #[test]
    fn campaign_start_never_overwrites_a_live_or_reused_global_session() {
        let _pool_guard = test_pool_guard();
        reset_test_pool();
        let mut first_mailbox = Mailbox::<1>::new();
        let first = CapacityCampaign::start(config(50), &mut first_mailbox).unwrap();
        let mut second_mailbox = Mailbox::<1>::new();

        assert!(matches!(
            CapacityCampaign::start(config(51), &mut second_mailbox),
            Err(CapacityCampaignError::SessionStartRejected)
        ));
        assert_eq!(second_mailbox.capacity_snapshot().session_id, 0);

        unsafe { first.finish(&mut first_mailbox) }.unwrap();
        assert!(matches!(
            CapacityCampaign::start(config(50), &mut second_mailbox),
            Err(CapacityCampaignError::SessionStartRejected)
        ));
        let second = CapacityCampaign::start(config(51), &mut second_mailbox).unwrap();
        assert_eq!(second.session_id, 51);
        assert_eq!(SamplePool::capacity_snapshot().session_id, 51);
    }

    #[test]
    fn finish_fails_closed_when_a_path_observer_is_active() {
        let _pool_guard = test_pool_guard();
        reset_test_pool();
        let mut mailbox = Mailbox::<1>::new();
        let campaign = CapacityCampaign::start(config(30), &mut mailbox).unwrap();
        campaign.observation_state.store(1, Ordering::Release);
        unsafe { campaign.finish(&mut mailbox) }.unwrap();
        campaign
            .observation_state
            .store(OBSERVATION_CLOSED, Ordering::Release);

        let mut registry = CapacityRegistry::<1>::new();
        registry
            .register(CapacityResource::mailbox([1; 32], 1, 1))
            .unwrap();
        let guards = StackGuardTable::<0>::new();
        let report = campaign.report(&registry, &mailbox, &guards);
        assert!(report.verify_checksum());
        assert!(!report.is_convertible());
        assert_ne!(report.flags & CAPACITY_FLAG_INCOMPLETE, 0);
        assert_eq!(report.resources, [CapacityResourceRecord::zeroed(); 1]);
    }

    #[test]
    fn premature_report_emits_zero_records_without_collecting_sources() {
        let _pool_guard = test_pool_guard();
        reset_test_pool();
        let mut mailbox = Mailbox::<1>::new();
        let campaign = CapacityCampaign::start(config(40), &mut mailbox).unwrap();
        mailbox.push(message(1)).unwrap();

        let mut stack = [0_u8; 128];
        let mut guards = StackGuardTable::<1>::new();
        unsafe {
            guards
                .register(
                    ModuleId::Sensor,
                    StackRegion {
                        base: stack.as_mut_ptr() as usize,
                        len: stack.len(),
                        canary_bytes: 16,
                    },
                )
                .unwrap();
            core::ptr::write_volatile(stack.as_mut_ptr().add(120), 0xA5);
        }
        let mut registry = CapacityRegistry::<2>::new();
        registry
            .register(CapacityResource::stack([1; 32], ModuleId::Sensor, 128, 8))
            .unwrap();
        registry
            .register(CapacityResource::mailbox([2; 32], 1, 1))
            .unwrap();

        let report = campaign.report(&registry, &mailbox, &guards);
        assert!(report.verify_checksum());
        assert!(!report.is_convertible());
        assert_ne!(report.flags & CAPACITY_FLAG_INCOMPLETE, 0);
        assert_eq!(report.resources, [CapacityResourceRecord::zeroed(); 2]);
    }

    #[test]
    fn fixed_layout_has_no_hidden_padding() {
        assert_eq!(
            core::mem::size_of::<CapacityResourceRecord>(),
            CAPACITY_RESOURCE_RECORD_BYTES
        );
        assert_eq!(
            core::mem::size_of::<CapacityReport<0>>(),
            CAPACITY_REPORT_FIXED_BYTES
        );
        assert_eq!(
            core::mem::size_of::<CapacityReport<3>>(),
            CAPACITY_REPORT_FIXED_BYTES + 3 * CAPACITY_RESOURCE_RECORD_BYTES
        );
    }
}
