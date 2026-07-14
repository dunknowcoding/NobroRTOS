//! Allocation-free executor timing reports.
//!
//! Executor timing is opt-in through
//! [`KernelExecutor::run_cycle_instrumented`](crate::KernelExecutor::run_cycle_instrumented).
//! The ordinary executor path does not own a recorder and performs no additional clock reads.

pub const EXECUTOR_TIMING_REPORT_MAGIC: u32 = 0x4E42_4554; // "NBET"
pub const EXECUTOR_TIMING_REPORT_VERSION: u32 = 1;
pub const EXECUTOR_TIMING_REPORT_WORDS: usize = 39;

pub const EXECUTOR_FLAG_COUNTER_SATURATED: u32 = 1;
pub const EXECUTOR_FLAG_INCOMPLETE: u32 = 1 << 1;
pub const EXECUTOR_FLAG_IDENTITY_MISSING: u32 = 1 << 2;
pub const EXECUTOR_FLAG_CLOCK_INVALID: u32 = 1 << 3;
pub const EXECUTOR_FLAG_GROUP_TABLE_FULL: u32 = 1 << 4;
pub const EXECUTOR_FLAG_SELECTION_REEVALUATED: u32 = 1 << 5;
pub const EXECUTOR_FLAG_PARTIAL_RELEASE_GROUP: u32 = 1 << 6;
pub const EXECUTOR_FLAG_SELECTION_UNSTABLE: u32 = 1 << 7;

const EXECUTOR_FATAL_FLAGS: u32 = EXECUTOR_FLAG_COUNTER_SATURATED
    | EXECUTOR_FLAG_INCOMPLETE
    | EXECUTOR_FLAG_IDENTITY_MISSING
    | EXECUTOR_FLAG_CLOCK_INVALID
    | EXECUTOR_FLAG_GROUP_TABLE_FULL
    | EXECUTOR_FLAG_PARTIAL_RELEASE_GROUP
    | EXECUTOR_FLAG_SELECTION_UNSTABLE;
const EXECUTOR_KNOWN_FLAGS: u32 = EXECUTOR_FATAL_FLAGS | EXECUTOR_FLAG_SELECTION_REEVALUATED;

const FNV1A32_OFFSET: u32 = 0x811C_9DC5;
const FNV1A32_PRIME: u32 = 0x0100_0193;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReportIdentity {
    pub build_id: u32,
    pub workload_id: u32,
    pub declaration_id: u32,
}

impl ReportIdentity {
    pub const fn new(build_id: u32, workload_id: u32, declaration_id: u32) -> Self {
        Self {
            build_id,
            workload_id,
            declaration_id,
        }
    }

    pub const fn is_complete(self) -> bool {
        self.build_id != 0 && self.workload_id != 0 && self.declaration_id != 0
    }
}

/// Fixed-layout report suitable for a debugger memory read or a byte-stream transport.
/// Durations are in the same microsecond domain supplied to `run_cycle_instrumented`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExecutorTimingReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub flags: u32,
    pub build_id: u32,
    pub workload_id: u32,
    pub declaration_id: u32,
    pub poll_attempts: u32,
    pub dispatch_samples: u32,
    pub release_dispatch_total_us_lo: u32,
    pub release_dispatch_total_us_hi: u32,
    pub release_dispatch_max_us: u32,
    pub simultaneous_release_groups: u32,
    pub simultaneous_releases: u32,
    pub simultaneous_ordered_dispatches: u32,
    pub simultaneous_max_width: u32,
    pub simultaneous_max_rank: u32,
    /// FNV-1a proof over release, width, rank, module, priority, and slot tie-break.
    pub simultaneous_order_hash: u32,
    pub simultaneous_order_delay_total_us_lo: u32,
    pub simultaneous_order_delay_total_us_hi: u32,
    pub simultaneous_order_delay_max_us: u32,
    pub selection_samples: u32,
    pub selection_sweep_slots_total_lo: u32,
    pub selection_sweep_slots_total_hi: u32,
    pub selection_sweep_slots_max: u32,
    pub selection_due_tasks_total_lo: u32,
    pub selection_due_tasks_total_hi: u32,
    pub selection_due_tasks_max: u32,
    pub selection_duration_total_us_lo: u32,
    pub selection_duration_total_us_hi: u32,
    pub selection_duration_max_us: u32,
    pub poll_bookkeeping_samples: u32,
    pub poll_bookkeeping_total_us_lo: u32,
    pub poll_bookkeeping_total_us_hi: u32,
    pub poll_bookkeeping_max_us: u32,
    /// Extra clock reads made by the opt-in probe (four per stable successful poll,
    /// plus any bounded selection re-evaluations).
    pub probe_clock_reads: u32,
    /// Extra task slots inspected by attribution-only simultaneous-peer scans.
    pub probe_scan_slots_total_lo: u32,
    pub probe_scan_slots_total_hi: u32,
    pub checksum: u32,
}

impl ExecutorTimingReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            flags: 0,
            build_id: 0,
            workload_id: 0,
            declaration_id: 0,
            poll_attempts: 0,
            dispatch_samples: 0,
            release_dispatch_total_us_lo: 0,
            release_dispatch_total_us_hi: 0,
            release_dispatch_max_us: 0,
            simultaneous_release_groups: 0,
            simultaneous_releases: 0,
            simultaneous_ordered_dispatches: 0,
            simultaneous_max_width: 0,
            simultaneous_max_rank: 0,
            simultaneous_order_hash: FNV1A32_OFFSET,
            simultaneous_order_delay_total_us_lo: 0,
            simultaneous_order_delay_total_us_hi: 0,
            simultaneous_order_delay_max_us: 0,
            selection_samples: 0,
            selection_sweep_slots_total_lo: 0,
            selection_sweep_slots_total_hi: 0,
            selection_sweep_slots_max: 0,
            selection_due_tasks_total_lo: 0,
            selection_due_tasks_total_hi: 0,
            selection_due_tasks_max: 0,
            selection_duration_total_us_lo: 0,
            selection_duration_total_us_hi: 0,
            selection_duration_max_us: 0,
            poll_bookkeeping_samples: 0,
            poll_bookkeeping_total_us_lo: 0,
            poll_bookkeeping_total_us_hi: 0,
            poll_bookkeeping_max_us: 0,
            probe_clock_reads: 0,
            probe_scan_slots_total_lo: 0,
            probe_scan_slots_total_hi: 0,
            checksum: 0,
        }
    }

    pub fn release_dispatch_total_us(&self) -> u64 {
        join_u64(
            self.release_dispatch_total_us_lo,
            self.release_dispatch_total_us_hi,
        )
    }

    pub fn simultaneous_order_delay_total_us(&self) -> u64 {
        join_u64(
            self.simultaneous_order_delay_total_us_lo,
            self.simultaneous_order_delay_total_us_hi,
        )
    }

    pub fn selection_sweep_slots_total(&self) -> u64 {
        join_u64(
            self.selection_sweep_slots_total_lo,
            self.selection_sweep_slots_total_hi,
        )
    }

    pub fn selection_due_tasks_total(&self) -> u64 {
        join_u64(
            self.selection_due_tasks_total_lo,
            self.selection_due_tasks_total_hi,
        )
    }

    pub fn selection_duration_total_us(&self) -> u64 {
        join_u64(
            self.selection_duration_total_us_lo,
            self.selection_duration_total_us_hi,
        )
    }

    pub fn poll_bookkeeping_total_us(&self) -> u64 {
        join_u64(
            self.poll_bookkeeping_total_us_lo,
            self.poll_bookkeeping_total_us_hi,
        )
    }

    pub fn probe_scan_slots_total(&self) -> u64 {
        join_u64(
            self.probe_scan_slots_total_lo,
            self.probe_scan_slots_total_hi,
        )
    }

    pub fn counters_saturated(&self) -> bool {
        self.flags & EXECUTOR_FLAG_COUNTER_SATURATED != 0
    }

    pub fn clock_valid(&self) -> bool {
        self.flags & EXECUTOR_FLAG_CLOCK_INVALID == 0
    }

    pub fn verify_checksum(&self) -> bool {
        self.magic == EXECUTOR_TIMING_REPORT_MAGIC
            && self.version == EXECUTOR_TIMING_REPORT_VERSION
            && self.completed == 1
            && self.build_id != 0
            && self.workload_id != 0
            && self.declaration_id != 0
            && self.flags & EXECUTOR_FATAL_FLAGS == 0
            && self.flags & !EXECUTOR_KNOWN_FLAGS == 0
            && self.checksum == self.compute_checksum()
    }

    fn seal(&mut self) {
        self.magic = EXECUTOR_TIMING_REPORT_MAGIC;
        self.version = EXECUTOR_TIMING_REPORT_VERSION;
        let identity_complete =
            self.build_id != 0 && self.workload_id != 0 && self.declaration_id != 0;
        let samples_complete = self.poll_attempts != 0
            && self.poll_attempts == self.dispatch_samples
            && self.dispatch_samples == self.poll_bookkeeping_samples;
        if !identity_complete {
            self.flags |= EXECUTOR_FLAG_IDENTITY_MISSING;
        }
        if !samples_complete {
            self.flags |= EXECUTOR_FLAG_INCOMPLETE;
        }
        self.completed = u32::from(self.flags & EXECUTOR_FATAL_FLAGS == 0);
        self.checksum = 0;
        self.checksum = self.compute_checksum();
    }

    fn compute_checksum(&self) -> u32 {
        let words = unsafe {
            core::slice::from_raw_parts(
                (self as *const Self).cast::<u32>(),
                EXECUTOR_TIMING_REPORT_WORDS - 1,
            )
        };
        words
            .iter()
            .fold(FNV1A32_OFFSET, |checksum, word| hash_u32(checksum, *word))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ReleaseGroup {
    release_us: u64,
    first_dispatch_us: u64,
    expected_width: u32,
    dispatched: u32,
    used: bool,
}

impl ReleaseGroup {
    const EMPTY: Self = Self {
        release_us: 0,
        first_dispatch_us: 0,
        expected_width: 0,
        dispatched: 0,
        used: false,
    };
}

/// Bounded recorder. It is caller-owned so the normal executor has zero recorder RAM.
/// `GROUPS` bounds concurrently interleaved simultaneous-release groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutorInstrumentation<const GROUPS: usize = 8> {
    counters: ExecutorTimingReport,
    groups: [ReleaseGroup; GROUPS],
}

impl<const GROUPS: usize> ExecutorInstrumentation<GROUPS> {
    pub const fn new() -> Self {
        Self::with_identity(ReportIdentity::new(0, 0, 0))
    }

    pub const fn with_identity(identity: ReportIdentity) -> Self {
        let mut counters = ExecutorTimingReport::zeroed();
        counters.build_id = identity.build_id;
        counters.workload_id = identity.workload_id;
        counters.declaration_id = identity.declaration_id;
        Self {
            counters,
            groups: [ReleaseGroup::EMPTY; GROUPS],
        }
    }

    pub fn reset(&mut self) {
        let identity = ReportIdentity::new(
            self.counters.build_id,
            self.counters.workload_id,
            self.counters.declaration_id,
        );
        *self = Self::with_identity(identity);
    }

    pub const fn storage_bytes() -> usize {
        core::mem::size_of::<Self>()
    }

    pub fn report(&self) -> ExecutorTimingReport {
        let mut report = self.counters;
        if self.groups.iter().any(|group| group.used) {
            report.flags |= EXECUTOR_FLAG_PARTIAL_RELEASE_GROUP | EXECUTOR_FLAG_INCOMPLETE;
        }
        report.seal();
        report
    }

    pub(crate) fn record_poll_attempt(&mut self) {
        add_u32(
            &mut self.counters.poll_attempts,
            1,
            &mut self.counters.flags,
        );
    }

    pub(crate) fn record_selection(
        &mut self,
        sweep_slots: u32,
        due_tasks: u32,
        started_us: u64,
        finished_us: u64,
        probe_clock_reads: u32,
    ) {
        add_u32(
            &mut self.counters.selection_samples,
            1,
            &mut self.counters.flags,
        );
        add_split_u64(
            &mut self.counters.selection_sweep_slots_total_lo,
            &mut self.counters.selection_sweep_slots_total_hi,
            u64::from(sweep_slots),
            &mut self.counters.flags,
        );
        self.counters.selection_sweep_slots_max =
            self.counters.selection_sweep_slots_max.max(sweep_slots);
        add_split_u64(
            &mut self.counters.selection_due_tasks_total_lo,
            &mut self.counters.selection_due_tasks_total_hi,
            u64::from(due_tasks),
            &mut self.counters.flags,
        );
        self.counters.selection_due_tasks_max =
            self.counters.selection_due_tasks_max.max(due_tasks);
        let elapsed = checked_elapsed(started_us, finished_us, &mut self.counters.flags);
        let duration_us = clamp_u32(elapsed, &mut self.counters.flags);
        add_split_u64(
            &mut self.counters.selection_duration_total_us_lo,
            &mut self.counters.selection_duration_total_us_hi,
            u64::from(duration_us),
            &mut self.counters.flags,
        );
        self.counters.selection_duration_max_us =
            self.counters.selection_duration_max_us.max(duration_us);
        add_u32(
            &mut self.counters.probe_clock_reads,
            probe_clock_reads,
            &mut self.counters.flags,
        );
    }

    pub(crate) fn record_clock_invalid(&mut self) {
        self.counters.flags |= EXECUTOR_FLAG_CLOCK_INVALID | EXECUTOR_FLAG_INCOMPLETE;
    }

    pub(crate) fn record_probe_scan_slots(&mut self, slots: u32) {
        add_split_u64(
            &mut self.counters.probe_scan_slots_total_lo,
            &mut self.counters.probe_scan_slots_total_hi,
            u64::from(slots),
            &mut self.counters.flags,
        );
    }

    pub(crate) fn record_probe_clock_reads(&mut self, reads: u32) {
        add_u32(
            &mut self.counters.probe_clock_reads,
            reads,
            &mut self.counters.flags,
        );
    }

    pub(crate) fn record_selection_reevaluated(&mut self) {
        self.counters.flags |= EXECUTOR_FLAG_SELECTION_REEVALUATED;
    }

    pub(crate) fn record_selection_unstable(&mut self) {
        self.counters.flags |= EXECUTOR_FLAG_SELECTION_UNSTABLE | EXECUTOR_FLAG_INCOMPLETE;
    }

    pub(crate) fn record_dispatch(
        &mut self,
        release_us: u64,
        dispatch_us: u64,
        simultaneous_width: u32,
        module: u32,
        priority: u32,
        slot_tie_break: u32,
    ) {
        add_u32(
            &mut self.counters.dispatch_samples,
            1,
            &mut self.counters.flags,
        );
        let latency = checked_elapsed(release_us, dispatch_us, &mut self.counters.flags);
        let latency_us = clamp_u32(latency, &mut self.counters.flags);
        add_split_u64(
            &mut self.counters.release_dispatch_total_us_lo,
            &mut self.counters.release_dispatch_total_us_hi,
            u64::from(latency_us),
            &mut self.counters.flags,
        );
        self.counters.release_dispatch_max_us =
            self.counters.release_dispatch_max_us.max(latency_us);

        if let Some(index) = self
            .groups
            .iter()
            .position(|group| group.used && group.release_us == release_us)
        {
            self.record_group_dispatch(index, dispatch_us, module, priority, slot_tie_break);
        } else if simultaneous_width > 1 {
            let Some(index) = self.groups.iter().position(|group| !group.used) else {
                self.counters.flags |= EXECUTOR_FLAG_GROUP_TABLE_FULL | EXECUTOR_FLAG_INCOMPLETE;
                return;
            };
            self.groups[index] = ReleaseGroup {
                release_us,
                first_dispatch_us: dispatch_us,
                expected_width: simultaneous_width,
                dispatched: 0,
                used: true,
            };
            add_u32(
                &mut self.counters.simultaneous_release_groups,
                1,
                &mut self.counters.flags,
            );
            add_u32(
                &mut self.counters.simultaneous_releases,
                simultaneous_width,
                &mut self.counters.flags,
            );
            self.counters.simultaneous_max_width =
                self.counters.simultaneous_max_width.max(simultaneous_width);
            self.record_group_dispatch(index, dispatch_us, module, priority, slot_tie_break);
        }
    }

    fn record_group_dispatch(
        &mut self,
        index: usize,
        dispatch_us: u64,
        module: u32,
        priority: u32,
        slot_tie_break: u32,
    ) {
        let group = &mut self.groups[index];
        group.dispatched = group.dispatched.saturating_add(1);
        let rank = group.dispatched;
        let release_us = group.release_us;
        let expected_width = group.expected_width;
        let first_dispatch_us = group.first_dispatch_us;
        add_u32(
            &mut self.counters.simultaneous_ordered_dispatches,
            1,
            &mut self.counters.flags,
        );
        self.counters.simultaneous_max_rank = self.counters.simultaneous_max_rank.max(rank);
        let delay = checked_elapsed(first_dispatch_us, dispatch_us, &mut self.counters.flags);
        let delay_us = clamp_u32(delay, &mut self.counters.flags);
        add_split_u64(
            &mut self.counters.simultaneous_order_delay_total_us_lo,
            &mut self.counters.simultaneous_order_delay_total_us_hi,
            u64::from(delay_us),
            &mut self.counters.flags,
        );
        self.counters.simultaneous_order_delay_max_us =
            self.counters.simultaneous_order_delay_max_us.max(delay_us);
        for value in [
            release_us as u32,
            (release_us >> 32) as u32,
            expected_width,
            rank,
            module,
            priority,
            slot_tie_break,
        ] {
            self.counters.simultaneous_order_hash =
                hash_u32(self.counters.simultaneous_order_hash, value);
        }
        if group.dispatched >= group.expected_width {
            *group = ReleaseGroup::EMPTY;
        }
    }

    pub(crate) fn record_poll_clock(&mut self, started_us: u64, finished_us: u64) {
        let _ = checked_elapsed(started_us, finished_us, &mut self.counters.flags);
    }

    pub(crate) fn record_bookkeeping(&mut self, started_us: u64, finished_us: u64) {
        add_u32(
            &mut self.counters.poll_bookkeeping_samples,
            1,
            &mut self.counters.flags,
        );
        let elapsed = checked_elapsed(started_us, finished_us, &mut self.counters.flags);
        let duration_us = clamp_u32(elapsed, &mut self.counters.flags);
        add_split_u64(
            &mut self.counters.poll_bookkeeping_total_us_lo,
            &mut self.counters.poll_bookkeeping_total_us_hi,
            u64::from(duration_us),
            &mut self.counters.flags,
        );
        self.counters.poll_bookkeeping_max_us =
            self.counters.poll_bookkeeping_max_us.max(duration_us);
        add_u32(
            &mut self.counters.probe_clock_reads,
            1,
            &mut self.counters.flags,
        );
    }
}

impl<const GROUPS: usize> Default for ExecutorInstrumentation<GROUPS> {
    fn default() -> Self {
        Self::new()
    }
}

const fn join_u64(lo: u32, hi: u32) -> u64 {
    (lo as u64) | ((hi as u64) << 32)
}

fn checked_elapsed(started_us: u64, finished_us: u64, flags: &mut u32) -> u64 {
    if finished_us < started_us {
        *flags |= EXECUTOR_FLAG_CLOCK_INVALID | EXECUTOR_FLAG_INCOMPLETE;
        0
    } else {
        finished_us - started_us
    }
}

fn clamp_u32(value: u64, flags: &mut u32) -> u32 {
    if value > u64::from(u32::MAX) {
        *flags |= EXECUTOR_FLAG_COUNTER_SATURATED | EXECUTOR_FLAG_INCOMPLETE;
        u32::MAX
    } else {
        value as u32
    }
}

fn hash_u32(mut hash: u32, value: u32) -> u32 {
    for byte in value.to_le_bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(FNV1A32_PRIME);
    }
    hash
}

fn add_u32(value: &mut u32, addend: u32, flags: &mut u32) {
    let (sum, overflow) = value.overflowing_add(addend);
    if overflow {
        *value = u32::MAX;
        *flags |= EXECUTOR_FLAG_COUNTER_SATURATED | EXECUTOR_FLAG_INCOMPLETE;
    } else {
        *value = sum;
    }
}

fn add_split_u64(lo: &mut u32, hi: &mut u32, addend: u64, flags: &mut u32) {
    let current = join_u64(*lo, *hi);
    let (sum, overflow) = current.overflowing_add(addend);
    let sum = if overflow {
        *flags |= EXECUTOR_FLAG_COUNTER_SATURATED | EXECUTOR_FLAG_INCOMPLETE;
        u64::MAX
    } else {
        sum
    };
    *lo = sum as u32;
    *hi = (sum >> 32) as u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: ReportIdentity = ReportIdentity::new(1, 2, 3);

    fn record_poll<const GROUPS: usize>(
        recorder: &mut ExecutorInstrumentation<GROUPS>,
        release: u64,
        dispatch: u64,
        width: u32,
        module: u32,
        slot: u32,
    ) {
        recorder.record_selection(5, width, dispatch - 2, dispatch - 1, 2);
        recorder.record_poll_attempt();
        recorder.record_dispatch(release, dispatch, width, module, 3, slot);
        recorder.record_poll_clock(dispatch, dispatch + 2);
        recorder.record_bookkeeping(dispatch + 2, dispatch + 3);
    }

    #[test]
    fn timing_report_tracks_non_adjacent_simultaneous_groups_and_order_identity() {
        let mut recorder = ExecutorInstrumentation::<3>::with_identity(IDENTITY);
        record_poll(&mut recorder, 100, 110, 3, 10, 0);
        record_poll(&mut recorder, 200, 205, 2, 20, 1);
        record_poll(&mut recorder, 100, 130, 2, 11, 2);
        record_poll(&mut recorder, 200, 215, 1, 21, 3);
        record_poll(&mut recorder, 100, 145, 1, 12, 4);
        let report = recorder.report();
        assert!(report.verify_checksum());
        assert_eq!(report.completed, 1);
        assert_eq!(report.release_dispatch_total_us(), 105);
        assert_eq!(report.release_dispatch_max_us, 45);
        assert_eq!(report.simultaneous_release_groups, 2);
        assert_eq!(report.simultaneous_releases, 5);
        assert_eq!(report.simultaneous_ordered_dispatches, 5);
        assert_eq!(report.simultaneous_max_width, 3);
        assert_eq!(report.simultaneous_max_rank, 3);
        assert_eq!(report.simultaneous_order_delay_total_us(), 65);
        assert_ne!(report.simultaneous_order_hash, FNV1A32_OFFSET);
        assert_eq!(report.selection_sweep_slots_total(), 25);
        assert_eq!(report.selection_duration_total_us(), 5);
        assert_eq!(report.poll_bookkeeping_total_us(), 5);
        assert_eq!(report.probe_clock_reads, 15);

        let mut unknown_flags = report;
        unknown_flags.flags |= 1 << 31;
        unknown_flags.checksum = 0;
        unknown_flags.checksum = unknown_flags.compute_checksum();
        assert!(!unknown_flags.verify_checksum());

        let mut incomplete_header = report;
        incomplete_header.completed = 0;
        incomplete_header.checksum = 0;
        incomplete_header.checksum = incomplete_header.compute_checksum();
        assert!(!incomplete_header.verify_checksum());
    }

    #[test]
    fn empty_partial_saturated_and_regressing_reports_fail_closed() {
        assert_eq!(
            ExecutorInstrumentation::<1>::with_identity(IDENTITY)
                .report()
                .completed,
            0
        );

        let mut partial = ExecutorInstrumentation::<1>::with_identity(IDENTITY);
        partial.record_poll_attempt();
        let report = partial.report();
        assert_eq!(report.completed, 0);
        assert!(!report.verify_checksum());

        let mut partial_group = ExecutorInstrumentation::<1>::with_identity(IDENTITY);
        partial_group.record_poll_attempt();
        partial_group.record_dispatch(0, 1, 2, 1, 1, 0);
        partial_group.record_bookkeeping(1, 2);
        let report = partial_group.report();
        assert_eq!(report.completed, 0);
        assert_ne!(report.flags & EXECUTOR_FLAG_PARTIAL_RELEASE_GROUP, 0);
        assert!(!report.verify_checksum());

        let mut saturated = ExecutorInstrumentation::<1>::with_identity(IDENTITY);
        saturated.record_selection(1, 1, 0, u64::from(u32::MAX) + 1, 2);
        saturated.record_poll_attempt();
        saturated.record_dispatch(0, 1, 1, 1, 1, 0);
        saturated.record_bookkeeping(1, 2);
        let report = saturated.report();
        assert!(report.counters_saturated());
        assert_eq!(report.selection_duration_max_us, u32::MAX);
        assert_eq!(report.completed, 0);

        let mut regressing = ExecutorInstrumentation::<1>::with_identity(IDENTITY);
        regressing.record_selection(1, 1, 5, 4, 2);
        regressing.record_poll_attempt();
        regressing.record_dispatch(10, 9, 1, 1, 1, 0);
        regressing.record_bookkeeping(9, 8);
        let report = regressing.report();
        assert!(!report.clock_valid());
        assert_eq!(report.completed, 0);
    }

    #[test]
    fn fixed_report_word_counts_are_stable() {
        assert_eq!(
            core::mem::size_of::<ExecutorTimingReport>(),
            EXECUTOR_TIMING_REPORT_WORDS * core::mem::size_of::<u32>()
        );
    }
}
