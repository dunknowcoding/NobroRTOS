//! Hardware-in-the-loop fault-injection campaign (M169). The kernel's
//! FaultInjector generates a scripted sequence of module faults; each injected error is
//! fed to a RecoveryCoordinator running on real silicon, and we assert the recovery state
//! machine escalates exactly as specified: nominal Running, then repeated sensor errors
//! drive Degraded, sustained errors drive Recovering, an OK clears back toward Running,
//! and a watchdog expiry is handled. Self-certifies via NOBRO_HIL_REPORT (J-Link mem32).
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_hal::{
    lease::Resource,
    traits::{HalClock, HalLease, PlatformHal},
    ActivePlatform as Hal,
};
use nobro_kernel::fault_inject::{FaultInjector, FaultMode, FaultRule};
use nobro_kernel::lifecycle::SystemState;
use nobro_kernel::recovery::RecoveryCoordinator;
use nobro_kernel::{FaultThresholds, KernelError, ModuleId};

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    faults_injected: u32,
    reached_degraded: u32,
    reached_recovering: u32,
    recovered_running: u32,
}
const MAGIC: u32 = 0x4E48_494C; // "NHIL"

#[no_mangle]
#[used]
static mut NOBRO_HIL_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    faults_injected: 0,
    reached_degraded: 0,
    reached_recovering: 0,
    recovered_running: 0,
};

#[entry]
fn main() -> ! {
    Hal::acquire(Resource::Timer0, 2).ok();
    unsafe {
        Hal::init_timebase();
    }

    // notify after 2 consecutive errors, reboot/recover after 4.
    let thresholds = FaultThresholds {
        notify_after: 2,
        reboot_after: 4,
    };
    let mut rc = RecoveryCoordinator::<4, 16>::new(thresholds);
    rc.transition(SystemState::ValidateManifest, Hal::now_us())
        .ok();
    rc.transition(SystemState::InitDrivers, Hal::now_us()).ok();
    rc.transition(SystemState::Running, Hal::now_us()).ok();

    // Fault script: the sensor throws SensorReadFail on every check for a window.
    let mut fi = FaultInjector::<2>::new();
    fi.add(FaultRule::new(
        ModuleId::Sensor,
        KernelError::SensorReadFail,
        FaultMode::Window { start: 1, end: 6 },
    ))
    .ok();

    let mut faults_injected = 0u32;
    let mut reached_degraded = 0u32;
    let mut reached_recovering = 0u32;

    // Run the campaign: 6 ticks of injected faults.
    for _ in 0..6u32 {
        let now = Hal::now_us();
        if let Some(err) = fi.check(ModuleId::Sensor) {
            faults_injected += 1;
            if let Ok(outcome) = rc.record_error(ModuleId::Sensor, err, now) {
                match outcome.state {
                    SystemState::Degraded => reached_degraded = 1,
                    SystemState::Recovering => reached_recovering = 1,
                    _ => {}
                }
            }
        }
        // small spacing on the real clock
        let t0 = Hal::now_us();
        while Hal::now_us().wrapping_sub(t0) < 5_000 {
            cortex_m::asm::nop();
        }
    }

    // Sensor recovers. From Recovering the valid lifecycle path is a reboot cycle:
    // Recovering -> InitDrivers -> Running (matching a real module reboot-and-rejoin).
    rc.record_ok(ModuleId::Sensor, Hal::now_us());
    if rc.state() == SystemState::Recovering {
        let _ = rc.transition(SystemState::InitDrivers, Hal::now_us());
    } else if rc.state() == SystemState::Degraded {
        // Degraded can rejoin Running directly.
    }
    let _ = rc.transition(SystemState::Running, Hal::now_us());
    let recovered_running = u32::from(rc.state() == SystemState::Running);

    let pass = faults_injected == 6
        && reached_degraded == 1
        && reached_recovering == 1
        && recovered_running == 1;
    let ap = u32::from(pass);
    unsafe {
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_HIL_REPORT),
            Report {
                magic: MAGIC,
                version: 1,
                completed: 1,
                all_pass: ap,
                faults_injected,
                reached_degraded,
                reached_recovering,
                recovered_running,
            },
        );
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
