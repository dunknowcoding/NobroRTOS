//! Recovery pillar on hardware: drive the kernel's RecoveryCoordinator through a
//! fault scenario and record the escalation in NOBRO_RECOVERY_REPORT.
//!
//! Scenario (deterministic, synthetic faults - no real failure needed): from Running,
//! an Actuator watchdog expiry routes to Degraded + NotifyUserTask; then three Radio
//! errors (reboot_after = 3) escalate to Recovering + RebootModule. The report
//! captures each action + system state so a host can confirm the bounded fault state
//! machine runs correctly on the MCU.
#![no_std]
#![no_main]

use cortex_m::asm;
use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_kernel::{
    Action, FaultThresholds, KernelError, ModuleId, RecoveryCoordinator, SystemState,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct RecoveryReport {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    wd_action: u32,
    wd_state: u32,
    err_action: u32,
    err_state: u32,
    final_state: u32,
    checksum: u32,
}
const REC_MAGIC: u32 = 0x4E42_5243; // "NBRC"

#[no_mangle]
#[used]
static mut NOBRO_RECOVERY_REPORT: RecoveryReport = RecoveryReport {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    wd_action: 0,
    wd_state: 0,
    err_action: 0,
    err_state: 0,
    final_state: 0,
    checksum: 0,
};

fn state_u32(s: SystemState) -> u32 {
    match s {
        SystemState::ColdBoot => 0,
        SystemState::ValidateManifest => 1,
        SystemState::InitDrivers => 2,
        SystemState::Running => 3,
        SystemState::Degraded => 4,
        SystemState::Recovering => 5,
        SystemState::Halted => 6,
    }
}

fn action_u32(a: Action) -> u32 {
    match a {
        Action::RetryNow => 0,
        Action::RetryDelay(_) => 1,
        Action::NotifyUserTask => 2,
        Action::RebootModule => 3,
        Action::Ignore => 4,
    }
}

#[entry]
fn main() -> ! {
    let mut rec = RecoveryCoordinator::<2, 12>::new(FaultThresholds {
        notify_after: 1,
        reboot_after: 3,
    });
    let _ = rec.transition(SystemState::ValidateManifest, 10);
    let _ = rec.transition(SystemState::InitDrivers, 20);
    let _ = rec.transition(SystemState::Running, 30);

    // Actuator watchdog expiry -> degraded + notify.
    let wd = rec.record_watchdog_expired(ModuleId::Actuator, 40);
    // Three Radio errors -> escalate to reboot + recovering at reboot_after = 3.
    let _ = rec.record_error(ModuleId::Radio, KernelError::RadioTxFail, 50);
    let _ = rec.record_error(ModuleId::Radio, KernelError::RadioTxFail, 60);
    let er = rec.record_error(ModuleId::Radio, KernelError::RadioTxFail, 70);

    let (wd_action, wd_state) = match wd {
        Ok(o) => (action_u32(o.action), state_u32(o.state)),
        Err(_) => (0xFF, 0xFF),
    };
    let (err_action, err_state) = match er {
        Ok(o) => (action_u32(o.action), state_u32(o.state)),
        Err(_) => (0xFF, 0xFF),
    };
    let final_state = state_u32(rec.state());

    // Pass = watchdog -> (NotifyUserTask=2, Degraded=4); 3rd error -> (RebootModule=3,
    // Recovering=5); coordinator ends in Recovering(5).
    let pass =
        wd_action == 2 && wd_state == 4 && err_action == 3 && err_state == 5 && final_state == 5;
    let all_pass = u32::from(pass);
    let completed = 1u32;
    let cs = REC_MAGIC
        ^ 1
        ^ completed
        ^ all_pass
        ^ wd_action
        ^ wd_state
        ^ err_action
        ^ err_state
        ^ final_state;
    unsafe {
        NOBRO_RECOVERY_REPORT = RecoveryReport {
            magic: REC_MAGIC,
            version: 1,
            completed,
            all_pass,
            wd_action,
            wd_state,
            err_action,
            err_state,
            final_state,
            checksum: cs,
        };
    }

    loop {
        asm::delay(16_000_000);
    }
}
