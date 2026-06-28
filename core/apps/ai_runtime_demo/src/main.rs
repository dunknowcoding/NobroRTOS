//! AI runtime on a development board: a `ModelRegistry` multiplexes three models and
//! `AiRoutePolicy` routes each across a scenario matrix on real silicon (M35/M36); then
//! the trained NN's inference latency is measured against its deadline (M41). Reports
//! via J-Link mem32.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use cortex_m::peripheral::DWT;
use nobro_adapter_nn_motion_ai::{NnMotionClassifier, MODEL_ID};
use nobro_sal::{
    AiBackendKind, AiInferenceRequest, AiInferenceSal, AiModelContract, AiRegistryError,
    AiRoutePolicy, AiRoutePreference, AiRouteTarget, AiRuntimeState, ModelRegistry,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    registry_len: u32,
    routes_ok: u32,
    nn_latency_us: u32,
    nn_class: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E41_4952; // "NAIR"

#[no_mangle]
#[used]
static mut NOBRO_AI_RUNTIME_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    registry_len: 0,
    routes_ok: 0,
    nn_latency_us: 0,
    nn_class: 0,
    checksum: 0,
};

const LLM_ID: u32 = 0x4C4C_4D31; // "LLM1"
const VIS_ID: u32 = 0x5649_5331; // "VIS1"

fn classify(clf: &mut NnMotionClassifier, window: &[u8]) -> u8 {
    let mut out = [0u8; 4];
    match clf.infer(AiInferenceRequest::new(MODEL_ID, window, 2_000), &mut out) {
        Ok(_) => out[0],
        Err(_) => 0xFF,
    }
}

#[entry]
fn main() -> ! {
    // start HFXO so the clock advances at a known rate
    unsafe {
        core::ptr::write_volatile(0x4000_0000 as *mut u32, 1);
        while core::ptr::read_volatile(0x4000_0100 as *const u32) == 0 {}
    }

    let mut clf = NnMotionClassifier::new();

    // --- M36: register three models, multiplexed by model_id ---
    let mut reg = ModelRegistry::<4>::new();
    let _ = reg.register(clf.contract()); // on-device NN
    let _ = reg.register(AiModelContract::new(
        AiBackendKind::Hybrid,
        LLM_ID,
        256,
        256,
        0,
        50_000,
    ));
    let _ = reg.register(AiModelContract::new(
        AiBackendKind::EdgeSidecar,
        VIS_ID,
        256,
        64,
        0,
        30_000,
    ));
    let registry_len = reg.len() as u32;

    // --- M35: route each across a scenario matrix on real silicon ---
    let policy = AiRoutePolicy::new(AiRoutePreference::HybridFallback, 50_000, 2);
    let mut routes_ok = 0u32;

    // 1. NN on-device, local ready -> OnDevice
    let r1 = reg.route(
        &AiInferenceRequest::new(MODEL_ID, &[0u8; 12], 2_000),
        &policy,
        AiRuntimeState::new(true, false, 0, 0),
        4_000,
    );
    if matches!(r1, Ok(d) if d.target == AiRouteTarget::OnDevice) {
        routes_ok += 1;
    }
    // 2. hybrid LLM, local down + endpoint up -> RemoteApi
    let r2 = reg.route(
        &AiInferenceRequest::new(LLM_ID, &[0u8; 8], 50_000),
        &policy,
        AiRuntimeState::new(false, true, 0, 0),
        100_000,
    );
    if matches!(r2, Ok(d) if d.target == AiRouteTarget::RemoteApi) {
        routes_ok += 1;
    }
    // 3. hybrid LLM, both down but a fresh snapshot -> StaleSnapshot
    let r3 = reg.route(
        &AiInferenceRequest::new(LLM_ID, &[0u8; 8], 50_000),
        &policy,
        AiRuntimeState::new(false, false, 1_000, 3),
        100_000,
    );
    if matches!(r3, Ok(d) if d.target == AiRouteTarget::StaleSnapshot && d.uses_stale_snapshot) {
        routes_ok += 1;
    }
    // 4. edge-sidecar vision, endpoint up -> EdgeSidecar
    let r4 = reg.route(
        &AiInferenceRequest::new(VIS_ID, &[0u8; 16], 30_000),
        &policy,
        AiRuntimeState::new(false, true, 0, 0),
        100_000,
    );
    if matches!(r4, Ok(d) if d.target == AiRouteTarget::EdgeSidecar) {
        routes_ok += 1;
    }
    // 5. unknown model id -> rejected
    let r5 = reg.route(
        &AiInferenceRequest::new(0xDEAD, &[], 1_000),
        &policy,
        AiRuntimeState::new(true, false, 0, 0),
        4_000,
    );
    if matches!(r5, Err(AiRegistryError::UnknownModel(0xDEAD))) {
        routes_ok += 1;
    }

    // --- M41: time the on-device NN inference (avg of 100) vs its 2 ms deadline ---
    let mut active = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        active[i] = if i % 2 == 0 { 40 } else { 200 };
        i += 1;
    }
    let nn_class = classify(&mut clf, &active); // warm + result
    // Cycle-accurate timing via the DWT counter (64 MHz core -> 64 cycles/us).
    let mut cp = unsafe { cortex_m::Peripherals::steal() };
    cp.DCB.enable_trace();
    cp.DWT.enable_cycle_counter();
    let start = DWT::cycle_count();
    let mut sink = 0u8;
    for _ in 0..100u32 {
        sink ^= classify(&mut clf, core::hint::black_box(&active));
    }
    core::hint::black_box(sink);
    let cycles = DWT::cycle_count().wrapping_sub(start);
    let nn_latency_us = cycles / 6_400; // avg per inference over 100 runs
    let deadline_ok = nn_latency_us < 2_000; // within the contract timeout

    let pass = registry_len == 3 && routes_ok == 5 && deadline_ok && nn_class != 0xFF;
    let ap = u32::from(pass);
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ registry_len ^ routes_ok ^ nn_latency_us ^ u32::from(nn_class);
    unsafe {
        NOBRO_AI_RUNTIME_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            registry_len,
            routes_ok,
            nn_latency_us,
            nn_class: u32::from(nn_class),
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
