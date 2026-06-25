//! Kernel control-plane self-test on hardware: exercise the quota ledger, event log,
//! mailbox IPC, key-value store, and alarm queue on the MCU and record a per-subsystem
//! pass bit in NOBRO_SELFTEST_REPORT. These subsystems are host-tested in CI; this
//! proves they run identically on real hardware. Pure kernel logic - no HAL.
#![no_std]
#![no_main]

use cortex_m::asm;
use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_kernel::{
    AlarmQueue, BootAssembly, DegradePlanner, EventLog, FaultThresholds, KvStore, Mailbox,
    QuotaLedger, StartupDependency, Watchdog, kernel_module_spec,
    alarm::{Alarm, AlarmId},
    event_log::{EventKind, EventPayload, EventRecord, EventSeverity},
    kv::{KvKey, KvValue},
    mailbox::{Message, MessageKind},
    manifest::{Criticality, DeadlineContract, MemoryBudget, ModuleSpec, SystemBudget, SystemProfile},
    ModuleId,
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
    let a = Message::new(ModuleId::Kernel, ModuleId::Sensor, MessageKind::Command, 1, 0);
    let b = Message::new(ModuleId::Sensor, ModuleId::Kernel, MessageKind::Command, 2, 0);
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
    if q.schedule_once(AlarmId(1), ModuleId::Sensor, 100, 0).is_err() {
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
        ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
            .memory(MemoryBudget::new(8 * 1024, 2 * 1024, 2)),
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
    let all = quota && eventlog && mailbox && kv && alarm && watchdog && degrade && admission;

    let q = u32::from(quota);
    let e = u32::from(eventlog);
    let m = u32::from(mailbox);
    let k = u32::from(kv);
    let a = u32::from(alarm);
    let w = u32::from(watchdog);
    let d = u32::from(degrade);
    let adm = u32::from(admission);
    let ap = u32::from(all);
    let cs = ST_MAGIC ^ 1 ^ 1 ^ ap ^ q ^ e ^ m ^ k ^ a ^ w ^ d ^ adm;
    unsafe {
        NOBRO_SELFTEST_REPORT = SelftestReport {
            magic: ST_MAGIC,
            version: 1,
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
            checksum: cs,
        };
    }

    loop {
        asm::delay(16_000_000);
    }
}
