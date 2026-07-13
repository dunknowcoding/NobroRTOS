//! Kernel control-plane self-test on hardware: exercise the quota ledger, event log,
//! mailbox IPC, key-value store, alarm queue, watchdog, degrade planner, admission,
//! capability grants, retry/backoff, lifecycle state machine, health monitor, and the
//! sample pool on the MCU, recording a per-subsystem pass bit in NOBRO_SELFTEST_REPORT.
//! These subsystems are host-tested in CI; this proves they run identically on real
//! hardware. Pure kernel logic - no HAL.
#![no_std]
#![no_main]

use cortex_m::asm;
use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_kernel::{
    alarm::{Alarm, AlarmId},
    event_log::{EventKind, EventPayload, EventRecord, EventSeverity},
    kernel_module_spec,
    kv::{KvKey, KvValue},
    mailbox::{Message, MessageKind},
    manifest::{
        Criticality, DeadlineContract, MemoryBudget, ModuleSpec, SystemBudget, SystemProfile,
    },
    Action, AlarmQueue, BackoffKind, BootAssembly, Capability, CapabilityGrantTable, CapabilitySet,
    CompactImuPayload, DegradePlanner, EventLog, FaultThresholds, HealthMonitor, KernelError,
    KvStore, Lifecycle, Mailbox, ModuleId, QuotaLedger, RetryPolicy, RetryState, SampleKind,
    SamplePool, StartupDependency, SystemState, Watchdog,
};
use nobro_sal::{
    preflight_ai_invocation, AiBackendKind, AiInferenceRequest, AiInvocationLimits,
    AiModelContract, AiRoutePolicy, AiRoutePreference, AiRouteTarget, AiRuntimeState,
    AI_PREFLIGHT_INPUT_TOO_LARGE, AI_PREFLIGHT_MODEL_ID_MISMATCH,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct SelftestReport {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    quota_pass: u32,
    eventlog_pass: u32,
    mailbox_pass: u32,
    kv_pass: u32,
    alarm_pass: u32,
    watchdog_pass: u32,
    degrade_pass: u32,
    admission_pass: u32,
    capability_pass: u32,
    retry_pass: u32,
    lifecycle_pass: u32,
    health_pass: u32,
    pool_pass: u32,
    ai_route_pass: u32,
    ai_preflight_pass: u32,
    ros_service_pass: u32,
    checksum: u32,
}
const ST_MAGIC: u32 = 0x4E42_5354; // "NBST"

#[no_mangle]
#[used]
static mut NOBRO_SELFTEST_REPORT: SelftestReport = SelftestReport {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    quota_pass: 0,
    eventlog_pass: 0,
    mailbox_pass: 0,
    kv_pass: 0,
    alarm_pass: 0,
    watchdog_pass: 0,
    degrade_pass: 0,
    admission_pass: 0,
    capability_pass: 0,
    retry_pass: 0,
    lifecycle_pass: 0,
    health_pass: 0,
    pool_pass: 0,
    ai_route_pass: 0,
    ai_preflight_pass: 0,
    ros_service_pass: 0,
    checksum: 0,
};

fn test_quota() -> bool {
    let mut ledger = QuotaLedger::<2>::new();
    if ledger
        .register(ModuleId::Sensor, SystemBudget::new(1024, 256, 2))
        .is_err()
    {
        return false;
    }
    if ledger
        .reserve(ModuleId::Sensor, SystemBudget::new(512, 128, 1))
        .is_err()
    {
        return false;
    }
    if ledger.usage(ModuleId::Sensor) != Some(SystemBudget::new(512, 128, 1)) {
        return false;
    }
    if ledger
        .release(ModuleId::Sensor, SystemBudget::new(512, 128, 1))
        .is_err()
    {
        return false;
    }
    ledger.usage(ModuleId::Sensor) == Some(SystemBudget::new(0, 0, 0))
}

fn event(seq: u64, sev: EventSeverity) -> EventRecord {
    EventRecord::new(
        seq,
        ModuleId::Kernel,
        sev,
        EventKind::Boot,
        EventPayload::Counter(seq as u32),
    )
}

fn test_eventlog() -> bool {
    let mut log = EventLog::<3>::new();
    log.push(event(10, EventSeverity::Info));
    log.push(event(20, EventSeverity::Warn));
    log.push(event(30, EventSeverity::Error));
    log.push(event(40, EventSeverity::Fatal)); // overflows the ring of 3
    log.len() == 3 && log.dropped() == 1 && log.is_full()
}

fn test_mailbox() -> bool {
    let mut mb = Mailbox::<3>::new();
    let a = Message::new(
        ModuleId::Kernel,
        ModuleId::Sensor,
        MessageKind::Command,
        1,
        0,
    );
    let b = Message::new(
        ModuleId::Sensor,
        ModuleId::Kernel,
        MessageKind::Command,
        2,
        0,
    );
    if mb.push(a).is_err() || mb.push(b).is_err() {
        return false;
    }
    mb.pop() == Some(a) && mb.pop() == Some(b) && mb.is_empty()
}

fn test_kv() -> bool {
    let mut kv = KvStore::<2>::new();
    if kv.set(KvKey(1), KvValue::U32(42)).is_err() {
        return false;
    }
    if kv.set(KvKey(1), KvValue::U32(84)).is_err() {
        return false;
    }
    kv.get(KvKey(1)) == Some(KvValue::U32(84)) && kv.len() == 1
}

fn test_alarm() -> bool {
    let mut q = AlarmQueue::<3>::new();
    if q.schedule_once(AlarmId(1), ModuleId::Sensor, 100, 0)
        .is_err()
    {
        return false;
    }
    if q.schedule_once(AlarmId(2), ModuleId::Radio, 50, 0).is_err() {
        return false;
    }
    if q.next_due_us() != Some(50) {
        return false;
    }
    q.pop_due(50) == Some(Alarm::once(AlarmId(2), ModuleId::Radio, 50))
}

fn test_watchdog() -> bool {
    let mut wd = Watchdog::<2>::new();
    if wd.register(ModuleId::Sensor, 100, 0).is_err() {
        return false;
    }
    if wd.register(ModuleId::Radio, 500, 0).is_err() {
        return false;
    }
    let mut expired = [ModuleId::Kernel; 2];
    // At t=150 only Sensor (timeout 100) has expired, not Radio (timeout 500).
    wd.expired(150, &mut expired) == 1 && expired[0] == ModuleId::Sensor
}

fn test_degrade() -> bool {
    let modules = [
        ModuleSpec::new(ModuleId::Kernel, Criticality::HardRealtime)
            .memory(MemoryBudget::new(20, 4, 0))
            .deadline(DeadlineContract::new(20_000, 10)),
        ModuleSpec::new(ModuleId::Sensor, Criticality::Driver).memory(MemoryBudget::new(20, 4, 0)),
        ModuleSpec::new(ModuleId::App(1), Criticality::BestEffort)
            .memory(MemoryBudget::new(50, 4, 0)),
        ModuleSpec::new(ModuleId::App(2), Criticality::User).memory(MemoryBudget::new(20, 4, 0)),
    ];
    // Flash budget 70 cannot fit all (20+20+50+20=110): the planner drops the
    // best-effort App(1) first and keeps the higher-criticality modules.
    let profile = SystemProfile::new(70, 32, 8, 16);
    match DegradePlanner::fit::<4>(&modules, profile) {
        Ok(d) => d.disabled_count == 1 && d.disabled[0] == Some(ModuleId::App(1)),
        Err(_) => false,
    }
}

fn test_admission() -> bool {
    // Assemble + admit a kernel + Sensor manifest, exactly as a generated app does.
    type AppBoot = BootAssembly<4, 4, 4, 4, 4, 4, 4, 4, 16>;
    let specs = [
        kernel_module_spec(
            MemoryBudget::new(16 * 1024, 4 * 1024, 4),
            DeadlineContract::new(20_000, 10),
        ),
        ModuleSpec::new(ModuleId::Sensor, Criticality::Driver).memory(MemoryBudget::new(
            8 * 1024,
            2 * 1024,
            2,
        )),
    ];
    let deps = [StartupDependency::new(ModuleId::Sensor, ModuleId::Kernel)];
    match AppBoot::build(
        &specs,
        &deps,
        SystemProfile::NRF52840_CORE,
        FaultThresholds::DEFAULT,
        0,
    ) {
        Ok(boot) => {
            boot.manifest_report.verify_checksum() && boot.admission_report.verify_checksum()
        }
        Err(_) => false,
    }
}

fn default_action(_e: &KernelError) -> Action {
    Action::RetryNow
}

fn test_ros_service() -> bool {
    // M22: a RosBridgeSal request/response service. Publish two IMU messages, then call
    // the STATS service over RosBridgeSal::request and decode [published, transmitted,
    // dropped]; an unknown service hash must be rejected.
    use nobro_adapter_ros_imu_bridge::{RosImuBridge, SERVICE_STATS, TOPIC_IMU};
    use nobro_sal::RosBridgeSal;
    let mut bridge = RosImuBridge::new();
    if bridge.publish(TOPIC_IMU, &[1, 2, 3], 0).is_err()
        || bridge.publish(TOPIC_IMU, &[4, 5, 6], 0).is_err()
    {
        return false;
    }
    let mut resp = [0u8; 12];
    let n = match bridge.request(SERVICE_STATS, &[], &mut resp, 0) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let published = u32::from_le_bytes([resp[0], resp[1], resp[2], resp[3]]);
    let dropped = u32::from_le_bytes([resp[8], resp[9], resp[10], resp[11]]);
    let unknown_rejected = bridge.request(0xDEAD_BEEF, &[], &mut resp, 0).is_err();
    n == 12 && published == 2 && dropped == 0 && unknown_rejected
}

fn test_ai_route() -> bool {
    // M21: AiRoutePolicy routes by backend + runtime state, opening the endpoint circuit
    // after repeated failures and falling back off the budget.
    let policy = AiRoutePolicy::new(AiRoutePreference::PreferLocal, 50_000, 2);
    let local = AiModelContract::new(AiBackendKind::OnDevice, 1, 8, 8, 4096, 10_000);
    let remote = AiModelContract::new(AiBackendKind::RemoteApi, 2, 8, 8, 0, 10_000);
    let ready = AiRuntimeState::new(true, true, 1_000, 0);
    // Local backend + local ready + fits budget -> OnDevice.
    if policy.decide(local, ready, 25_000).target != AiRouteTarget::OnDevice {
        return false;
    }
    // Remote backend + endpoint ready -> RemoteApi.
    if policy.decide(remote, ready, 25_000).target != AiRouteTarget::RemoteApi {
        return false;
    }
    // Repeated endpoint failures open the circuit -> no longer routes to RemoteApi.
    let failing = AiRuntimeState::new(true, true, 1_000, 5);
    let d = policy.decide(remote, failing, 25_000);
    if d.target == AiRouteTarget::RemoteApi || !d.endpoint_circuit_open {
        return false;
    }
    // Budget too small for the contract timeout -> falls back off OnDevice.
    policy.decide(local, ready, 1_000).target != AiRouteTarget::OnDevice
}

fn test_ai_preflight() -> bool {
    // M24: preflight_ai_invocation admits a bounded local call and rejects a mismatched,
    // over-sized one with the right error bits.
    let contract = AiModelContract::new(AiBackendKind::OnDevice, 42, 8, 8, 4096, 20_000);
    let policy = AiRoutePolicy::new(AiRoutePreference::LocalOnly, 50_000, 2);
    let state = AiRuntimeState::new(true, false, 1_000, 0);
    let limits = AiInvocationLimits::new(8, 128, 8 * 1024, 25_000);

    let input = [1u8, 2, 3, 4];
    let ok = preflight_ai_invocation(
        contract,
        policy,
        state,
        AiInferenceRequest::new(42, &input, 10_000),
        limits,
    );
    if !ok.passing() || ok.route.target != AiRouteTarget::OnDevice {
        return false;
    }

    let big = [0u8; 12];
    let bad = preflight_ai_invocation(
        contract,
        policy,
        state,
        AiInferenceRequest::new(7, &big, 10_000),
        limits,
    );
    !bad.passing()
        && bad.has_error(AI_PREFLIGHT_MODEL_ID_MISMATCH)
        && bad.has_error(AI_PREFLIGHT_INPUT_TOO_LARGE)
}

fn test_capability() -> bool {
    // A module is granted a capability set; authorize() must allow what was granted,
    // deny what was not, and report a missing grant for an unregistered module.
    let mut table = CapabilityGrantTable::<2>::new();
    let granted = CapabilitySet::empty()
        .with(Capability::Bus0)
        .with(Capability::SamplePool);
    if table.register(ModuleId::Sensor, granted).is_err() {
        return false;
    }
    table.authorize(ModuleId::Sensor, Capability::Bus0).is_ok()
        && table
            .authorize(ModuleId::Sensor, Capability::Radio)
            .is_err()
        && table.authorize(ModuleId::Radio, Capability::Bus0).is_err()
}

fn test_retry() -> bool {
    let policy = RetryPolicy::new(3, 1_000, 20_000, BackoffKind::Exponential);
    // Exponential backoff grows with the attempt number.
    if policy.delay_for_attempt(2) <= policy.delay_for_attempt(1) {
        return false;
    }
    // The first 3 failures are retried; the 4th exhausts the budget and escalates.
    let mut state = RetryState::new();
    let a1 = state.fail(0, policy);
    let a2 = state.fail(0, policy);
    let a3 = state.fail(0, policy);
    let a4 = state.fail(0, policy);
    matches!(a1, Action::RetryDelay(_) | Action::RetryNow)
        && matches!(a2, Action::RetryDelay(_) | Action::RetryNow)
        && matches!(a3, Action::RetryDelay(_) | Action::RetryNow)
        && a4 == Action::NotifyUserTask
}

fn test_lifecycle() -> bool {
    let mut lc = Lifecycle::new();
    if lc.state() != SystemState::ColdBoot {
        return false;
    }
    // Walk the nominal boot path; each edge is a valid transition.
    if lc.transition(SystemState::ValidateManifest, 1).is_err()
        || lc.transition(SystemState::InitDrivers, 2).is_err()
        || lc.transition(SystemState::Running, 3).is_err()
    {
        return false;
    }
    // An illegal jump (Running -> ColdBoot) is rejected and the state is preserved.
    if lc.transition(SystemState::ColdBoot, 4).is_ok() {
        return false;
    }
    lc.state() == SystemState::Running && lc.transitions() == 3
}

fn test_health() -> bool {
    // FaultThresholds::DEFAULT = notify_after 3, reboot_after 8. Escalation must step
    // policy -> NotifyUserTask -> RebootModule as consecutive errors accumulate.
    let mut hm = HealthMonitor::<2>::new();
    let th = FaultThresholds::DEFAULT;
    let mut actions = [Action::Ignore; 8];
    for (i, slot) in actions.iter_mut().enumerate() {
        *slot = hm.record_error(
            ModuleId::Sensor,
            KernelError::BusTimeout,
            i as u64,
            th,
            default_action,
        );
    }
    actions[2] == Action::NotifyUserTask && actions[7] == Action::RebootModule
}

fn test_pool() -> bool {
    // Allocate a pool ticket, round-trip an IMU payload through it, then release.
    let payload = CompactImuPayload {
        accel_mg: [0, 0, 1000],
        gyro_mdps: [1000, 2000, 3000],
        temperature_centi_c: 2500,
    };
    let Some(sample) = SamplePool::alloc(SampleKind::Imu, CompactImuPayload::LEN, 100, 200) else {
        return false;
    };
    if sample.kind != SampleKind::Imu || !sample.handle.is_valid() {
        SamplePool::release(sample.handle);
        return false;
    }
    if !CompactImuPayload::write_to_handle(sample.handle, &payload) {
        SamplePool::release(sample.handle);
        return false;
    }
    let ok = CompactImuPayload::read_from_handle(sample.handle)
        .map(|p| p.accel_mg[2] == 1000 && p.gyro_mdps[2] == 3000)
        .unwrap_or(false);
    SamplePool::release(sample.handle);
    ok
}

#[entry]
fn main() -> ! {
    let quota = test_quota();
    let eventlog = test_eventlog();
    let mailbox = test_mailbox();
    let kv = test_kv();
    let alarm = test_alarm();
    let watchdog = test_watchdog();
    let degrade = test_degrade();
    let admission = test_admission();
    let capability = test_capability();
    let retry = test_retry();
    let lifecycle = test_lifecycle();
    let health = test_health();
    let pool = test_pool();
    let ai_route = test_ai_route();
    let ai_preflight = test_ai_preflight();
    let ros_service = test_ros_service();
    let all = quota
        && eventlog
        && mailbox
        && kv
        && alarm
        && watchdog
        && degrade
        && admission
        && capability
        && retry
        && lifecycle
        && health
        && pool
        && ai_route
        && ai_preflight
        && ros_service;

    let q = u32::from(quota);
    let e = u32::from(eventlog);
    let m = u32::from(mailbox);
    let k = u32::from(kv);
    let a = u32::from(alarm);
    let w = u32::from(watchdog);
    let d = u32::from(degrade);
    let adm = u32::from(admission);
    let cap = u32::from(capability);
    let ret = u32::from(retry);
    let lif = u32::from(lifecycle);
    let hea = u32::from(health);
    let poo = u32::from(pool);
    let air = u32::from(ai_route);
    let aip = u32::from(ai_preflight);
    let ros = u32::from(ros_service);
    let ap = u32::from(all);
    let cs = ST_MAGIC
        ^ 4
        ^ 1
        ^ ap
        ^ q
        ^ e
        ^ m
        ^ k
        ^ a
        ^ w
        ^ d
        ^ adm
        ^ cap
        ^ ret
        ^ lif
        ^ hea
        ^ poo
        ^ air
        ^ aip
        ^ ros;
    unsafe {
        NOBRO_SELFTEST_REPORT = SelftestReport {
            magic: ST_MAGIC,
            version: 4,
            completed: 1,
            all_pass: ap,
            quota_pass: q,
            eventlog_pass: e,
            mailbox_pass: m,
            kv_pass: k,
            alarm_pass: a,
            watchdog_pass: w,
            degrade_pass: d,
            admission_pass: adm,
            capability_pass: cap,
            retry_pass: ret,
            lifecycle_pass: lif,
            health_pass: hea,
            pool_pass: poo,
            ai_route_pass: air,
            ai_preflight_pass: aip,
            ros_service_pass: ros,
            checksum: cs,
        };
    }

    loop {
        asm::delay(16_000_000);
    }
}
