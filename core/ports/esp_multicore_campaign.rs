//! Shared, allocation-free secondary-core campaign used by the connected ESP
//! ports. Platform code owns startup, inter-processor wake, hard stop, and
//! restart; this module owns the generation-safe work and recovery protocol.

use core::sync::atomic::{AtomicU32, Ordering};

use nobro_kernel::{
    CrossCoreDataPlane, CrossCoreMessage, CrossCoreReceive, ModuleId, MulticoreExecutorLifecycle,
};

const STRESS_ITEMS: u32 = 4_096;
const CANCELLED_VALUE: u32 = 0xcace_1100;
const LIVE_VALUE: u32 = 0xcace_2200;
const FALLBACK_VALUE: u32 = 0xfa11_bacc;
const STALE_VALUE: u32 = 0x5a1e_0001;
const RECOVERY_VALUE: u32 = 0x5a5a_a5a5;
const TIMEOUT_US: u64 = 2_000_000;

static WORK: CrossCoreDataPlane<u32, 8> = CrossCoreDataPlane::new();
static GENERATION: AtomicU32 = AtomicU32::new(0);
static PROCESSED: AtomicU32 = AtomicU32::new(0);
static ACCUMULATOR: AtomicU32 = AtomicU32::new(0);
static PAUSE: AtomicU32 = AtomicU32::new(0);
static PAUSED: AtomicU32 = AtomicU32::new(0);
static WEDGE: AtomicU32 = AtomicU32::new(0);
static WEDGED: AtomicU32 = AtomicU32::new(0);
static CANCELLED: AtomicU32 = AtomicU32::new(0);
static STALE: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy)]
pub struct PreRecovery {
    generation: u32,
    accepted: u32,
    rejected: u32,
    base_processed: u32,
    base_accumulator: u32,
    base_cancelled: u32,
    base_stale: u32,
    expected_delta: u32,
    fallback: u32,
}

#[derive(Clone, Copy, Default)]
pub struct CampaignReport {
    pub accepted: u32,
    pub rejected: u32,
    pub processed: u32,
    pub expected: u32,
    pub actual: u32,
    pub cancelled: u32,
    pub stale: u32,
    pub fallback: u32,
    pub restart: u32,
    pub passed: bool,
}

pub fn begin_generation(generation: u32) -> bool {
    if WORK.begin_generation(generation).is_err() {
        return false;
    }
    PAUSE.store(0, Ordering::Release);
    PAUSED.store(0, Ordering::Release);
    WEDGE.store(0, Ordering::Release);
    WEDGED.store(0, Ordering::Release);
    GENERATION.store(generation, Ordering::Release);
    true
}

pub fn send(generation: u32, sequence: u32, payload: u32) -> bool {
    WORK.try_send(CrossCoreMessage {
        generation,
        sequence,
        payload,
    })
    .is_ok()
}

/// Run on the admitted secondary core. `idle` must enter the target's shallow
/// interrupt/event wait state; the primary core wakes it after publication.
pub fn secondary_core(mut idle: impl FnMut()) -> ! {
    loop {
        let generation = GENERATION.load(Ordering::Acquire);
        if generation == 0 {
            idle();
            continue;
        }
        if WEDGE.load(Ordering::Acquire) == generation {
            WEDGED.store(generation, Ordering::Release);
            while WEDGE.load(Ordering::Acquire) == generation {
                core::hint::spin_loop();
            }
        }
        if PAUSE.load(Ordering::Acquire) == generation {
            PAUSED.store(generation, Ordering::Release);
            while PAUSE.load(Ordering::Acquire) == generation {
                if WEDGE.load(Ordering::Acquire) == generation {
                    WEDGED.store(generation, Ordering::Release);
                    while WEDGE.load(Ordering::Acquire) == generation {
                        core::hint::spin_loop();
                    }
                }
                idle();
            }
            PAUSED.store(0, Ordering::Release);
        }
        while let Some(disposition) = WORK.try_receive() {
            match disposition {
                CrossCoreReceive::Work(message) => {
                    ACCUMULATOR.fetch_add(message.payload.wrapping_mul(3), Ordering::AcqRel);
                    PROCESSED.fetch_add(1, Ordering::AcqRel);
                }
                CrossCoreReceive::Cancelled(_) => {
                    CANCELLED.fetch_add(1, Ordering::AcqRel);
                }
                CrossCoreReceive::Stale(_) => {
                    STALE.fetch_add(1, Ordering::AcqRel);
                }
            }
        }
        idle();
    }
}

fn wait_until(
    mut condition: impl FnMut() -> bool,
    now_us: &mut impl FnMut() -> u64,
    wake: &mut impl FnMut(),
) -> bool {
    let deadline = now_us().saturating_add(TIMEOUT_US);
    while !condition() {
        if now_us() >= deadline {
            return false;
        }
        wake();
        core::hint::spin_loop();
    }
    true
}

/// Exercise production, saturation, cancellation, ownership fallback, and an
/// induced secondary-core wedge. On success the caller must stop the old core,
/// advance the lifecycle, start a replacement, and call `finish_recovery`.
pub fn run_until_wedge(
    lifecycle: &mut MulticoreExecutorLifecycle<2, 2>,
    mut now_us: impl FnMut() -> u64,
    mut wake: impl FnMut(),
) -> Option<PreRecovery> {
    let generation = GENERATION.load(Ordering::Acquire);
    if generation == 0 {
        return None;
    }
    let base_processed = PROCESSED.load(Ordering::Acquire);
    let base_accumulator = ACCUMULATOR.load(Ordering::Acquire);
    let base_cancelled = CANCELLED.load(Ordering::Acquire);
    let base_stale = STALE.load(Ordering::Acquire);
    let mut expected_delta = 0u32;
    let mut accepted = 0u32;
    let mut rejected = 0u32;
    let mut sequence = 1u32;
    let feed_deadline = now_us().saturating_add(TIMEOUT_US);

    while accepted < STRESS_ITEMS {
        let value = accepted.wrapping_add(1);
        if send(generation, sequence, value) {
            sequence = sequence.wrapping_add(1);
            accepted = accepted.wrapping_add(1);
            expected_delta = expected_delta.wrapping_add(value.wrapping_mul(3));
            wake();
        } else {
            rejected = rejected.wrapping_add(1);
            wake();
            if now_us() >= feed_deadline {
                return None;
            }
        }
    }
    if !wait_until(
        || {
            PROCESSED
                .load(Ordering::Acquire)
                .wrapping_sub(base_processed)
                == STRESS_ITEMS
        },
        &mut now_us,
        &mut wake,
    ) {
        return None;
    }

    PAUSE.store(generation, Ordering::Release);
    wake();
    if !wait_until(
        || PAUSED.load(Ordering::Acquire) == generation,
        &mut now_us,
        &mut wake,
    ) {
        return None;
    }
    let cancelled_sequence = sequence;
    if !send(generation, cancelled_sequence, CANCELLED_VALUE) {
        return None;
    }
    sequence = sequence.wrapping_add(1);
    if !send(generation, sequence, LIVE_VALUE)
        || !WORK.cancel_through(generation, cancelled_sequence)
    {
        return None;
    }
    sequence = sequence.wrapping_add(1);
    expected_delta = expected_delta.wrapping_add(LIVE_VALUE.wrapping_mul(3));
    PAUSE.store(0, Ordering::Release);
    wake();
    if !wait_until(
        || {
            PROCESSED
                .load(Ordering::Acquire)
                .wrapping_sub(base_processed)
                == STRESS_ITEMS + 1
                && CANCELLED
                    .load(Ordering::Acquire)
                    .wrapping_sub(base_cancelled)
                    == 1
        },
        &mut now_us,
        &mut wake,
    ) {
        return None;
    }

    PAUSE.store(generation, Ordering::Release);
    wake();
    if !wait_until(
        || PAUSED.load(Ordering::Acquire) == generation,
        &mut now_us,
        &mut wake,
    ) {
        return None;
    }
    let moved_to_primary = lifecycle.transfer(ModuleId::App(1), 1, 0).is_ok();
    let fallback_sent = send(generation, sequence, FALLBACK_VALUE);
    sequence = sequence.wrapping_add(1);
    let fallback_consumed = matches!(
        WORK.try_receive(),
        Some(CrossCoreReceive::Work(CrossCoreMessage { payload, .. }))
            if payload == FALLBACK_VALUE
    );
    let moved_back = lifecycle.transfer(ModuleId::App(1), 0, 1).is_ok();
    if !(moved_to_primary && fallback_sent && fallback_consumed && moved_back) {
        return None;
    }
    ACCUMULATOR.fetch_add(FALLBACK_VALUE.wrapping_mul(3), Ordering::AcqRel);
    expected_delta = expected_delta.wrapping_add(FALLBACK_VALUE.wrapping_mul(3));

    if !send(generation, sequence, STALE_VALUE) {
        return None;
    }
    WEDGE.store(generation, Ordering::Release);
    PAUSE.store(0, Ordering::Release);
    wake();
    if !wait_until(
        || WEDGED.load(Ordering::Acquire) == generation,
        &mut now_us,
        &mut wake,
    ) {
        return None;
    }

    Some(PreRecovery {
        generation,
        accepted,
        rejected,
        base_processed,
        base_accumulator,
        base_cancelled,
        base_stale,
        expected_delta,
        fallback: 1,
    })
}

pub fn finish_recovery(
    pre: PreRecovery,
    replacement_generation: u32,
    mut now_us: impl FnMut() -> u64,
    mut wake: impl FnMut(),
) -> CampaignReport {
    let mut report = CampaignReport {
        accepted: pre.accepted,
        rejected: pre.rejected,
        fallback: pre.fallback,
        restart: u32::from(replacement_generation > pre.generation),
        ..CampaignReport::default()
    };
    if replacement_generation <= pre.generation {
        return report;
    }
    if WORK.active_generation() != replacement_generation
        && !begin_generation(replacement_generation)
    {
        return report;
    }
    if !send(replacement_generation, 1, RECOVERY_VALUE) {
        return report;
    }
    wake();
    if !wait_until(
        || {
            STALE.load(Ordering::Acquire).wrapping_sub(pre.base_stale) == 1
                && PROCESSED
                    .load(Ordering::Acquire)
                    .wrapping_sub(pre.base_processed)
                    == STRESS_ITEMS + 2
        },
        &mut now_us,
        &mut wake,
    ) {
        return report;
    }

    report.processed = PROCESSED
        .load(Ordering::Acquire)
        .wrapping_sub(pre.base_processed);
    report.expected = pre
        .expected_delta
        .wrapping_add(RECOVERY_VALUE.wrapping_mul(3));
    report.actual = ACCUMULATOR
        .load(Ordering::Acquire)
        .wrapping_sub(pre.base_accumulator);
    report.cancelled = CANCELLED
        .load(Ordering::Acquire)
        .wrapping_sub(pre.base_cancelled);
    report.stale = STALE.load(Ordering::Acquire).wrapping_sub(pre.base_stale);
    report.passed = report.accepted == STRESS_ITEMS
        && report.processed == STRESS_ITEMS + 2
        && report.expected == report.actual
        && report.cancelled == 1
        && report.stale == 1
        && report.fallback == 1
        && report.restart == 1;
    report
}
