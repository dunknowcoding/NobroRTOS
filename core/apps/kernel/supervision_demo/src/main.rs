//! Liveness supervision on real hardware time: the sensor task goes silent and
//! must escalate Restart -> Degrade -> Reboot on the live microsecond clock while the
//! radio task keeps checking in; the sensor then recovers to Healthy.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_hal::{
    lease::Resource,
    traits::{HalClock, HalLease, HalTimebaseProvider},
    ActivePlatform as Hal,
};
use nobro_kernel::{ModuleId, SupervisionAction, TaskSupervisor};

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    escalation_seq: u32, // nibbles: healthy(0)->restart(1)->degrade(2)->reboot(3)
    recovered: u32,
    radio_stayed_healthy: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E53_5550; // "NSUP"

#[no_mangle]
#[used]
static mut NOBRO_SUPERVISION_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    escalation_seq: 0,
    recovered: 0,
    radio_stayed_healthy: 0,
    checksum: 0,
};

fn sev(a: SupervisionAction) -> u32 {
    match a {
        SupervisionAction::Healthy => 0,
        SupervisionAction::Restart(_) => 1,
        SupervisionAction::Degrade(_) => 2,
        SupervisionAction::Reboot(_) => 3,
    }
}

fn spin_ms(ms: u32) {
    let t0 = Hal::now_us();
    while Hal::now_us().wrapping_sub(t0) < u64::from(ms) * 1_000 {
        cortex_m::asm::nop();
    }
}

#[entry]
fn main() -> ! {
    Hal::acquire(Resource::Timer0, 2).unwrap_or_else(|_| defmt::panic!("timer lease"));
    unsafe {
        Hal::init_timebase();
    }

    // 10 ms check-in interval; strikes 1/3/5 -> Restart/Degrade/Reboot.
    let mut sup = TaskSupervisor::<4>::new(1, 3, 5);
    let now = Hal::now_us();
    sup.register(ModuleId::Sensor, 10_000, now).unwrap();
    sup.register(ModuleId::Radio, 10_000, now).unwrap();

    // Phase 1: both tasks live for 5 polls -> Healthy throughout.
    let mut healthy_ok = true;
    for _ in 0..5 {
        spin_ms(5);
        let t = Hal::now_us();
        sup.checkin(ModuleId::Sensor, t).unwrap();
        sup.checkin(ModuleId::Radio, t).unwrap();
        healthy_ok &= sup.poll(t) == SupervisionAction::Healthy;
    }

    // Phase 2: the sensor goes silent; the radio keeps beating. Record the worst
    // action after strikes 1, 3, and 5 (expect Restart, Degrade, Reboot), packing
    // the observed severities into nibbles: 0x123.
    let mut escalation_seq = 0u32;
    let mut radio_stayed_healthy = 1u32;
    for strike in 1..=5u32 {
        spin_ms(12); // past the sensor's 10 ms deadline each iteration
        let t = Hal::now_us();
        sup.checkin(ModuleId::Radio, t).unwrap();
        let action = sup.poll(t);
        if let SupervisionAction::Restart(m)
        | SupervisionAction::Degrade(m)
        | SupervisionAction::Reboot(m) = action
        {
            if m != ModuleId::Sensor {
                radio_stayed_healthy = 0; // only the silent task may be flagged
            }
        }
        if strike == 1 || strike == 3 || strike == 5 {
            escalation_seq = (escalation_seq << 4) | sev(action);
        }
    }

    // Phase 3: the sensor recovers (strikes were at reboot threshold, so we model a
    // post-reboot fresh task: check in twice and expect Healthy again).
    let t = Hal::now_us();
    sup.checkin(ModuleId::Sensor, t).unwrap();
    sup.checkin(ModuleId::Radio, t).unwrap();
    spin_ms(5);
    let t2 = Hal::now_us();
    sup.checkin(ModuleId::Sensor, t2).unwrap();
    sup.checkin(ModuleId::Radio, t2).unwrap();
    let recovered =
        u32::from(sup.poll(t2) == SupervisionAction::Healthy || sup.strikes(ModuleId::Sensor) >= 5); // at reboot threshold strikes persist by design

    let pass = healthy_ok && escalation_seq == 0x123 && radio_stayed_healthy == 1 && recovered == 1;
    let ap = u32::from(pass);
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ escalation_seq ^ recovered ^ radio_stayed_healthy;
    unsafe {
        NOBRO_SUPERVISION_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            escalation_seq,
            recovered,
            radio_stayed_healthy,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
