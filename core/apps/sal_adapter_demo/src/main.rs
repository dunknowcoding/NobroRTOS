//! Phase 2 SAL adapter demo with RoboServo and sensor-stub.
//!
//! Autonomous eval writes `NOBRO_SAL_EVAL_REPORT` for J-Link mem32 readback.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::asm;
use defmt_rtt as _;
use panic_probe as _;

use airon_adapter_robo_servo::{module_spec as robo_servo_spec, RoboServoAdapter};
use airon_adapter_sensor_stub::{module_spec as sensor_stub_spec, stub_imu_plausible, SensorStub};
use airon_hal::{
    lease::Resource,
    ppi,
    traits::{HalClock, HalDeadline, HalLease, HalServoPwm, PlatformHal},
    ActivePlatform as Hal, BoardPackageReport, BoardProfileReport, ACTIVE_BOARD_PACKAGE,
};
use airon_kernel::{
    eval::{
        SalEvalReport, MIN_IMU_SAMPLES, MIN_SERVO_STEPS, SAL_EVAL_MAGIC, SERVO_READBACK_TOL_US,
    },
    executor::{Poll, StatsTask, Task},
    kernel_module_spec,
    pool::SamplePool,
    scheduler::Scheduler,
    AdmissionReport, BootAssembly, BootAssemblyReports, DeadlineContract, DegradeApplicationReport,
    EventLogReport, FaultThresholds, ManifestReport, MemoryBudget, ModuleId, ModuleRuntimeReport,
    ModuleSpec, RuntimeReport, StartupDependency, SystemProfile,
};
use airon_sal::{ActuatorSal, AdapterCompatibilityReport, AdapterPreflight, SensorSal};

static SERVO_STEPS: AtomicU32 = AtomicU32::new(0);
static SERVO_READBACK_OK: AtomicU32 = AtomicU32::new(0);
static IMU_SAMPLES: AtomicU32 = AtomicU32::new(0);
static IMU_PLAUSIBLE: AtomicU32 = AtomicU32::new(0);
static EVAL_DONE: AtomicU32 = AtomicU32::new(0);

static mut SERVO_CMD_US: u32 = 1500;
static mut SERVO_DIR: i8 = 1;

#[no_mangle]
#[used]
static mut NOBRO_SAL_EVAL_REPORT: SalEvalReport = SalEvalReport::zeroed();

#[no_mangle]
#[used]
static mut NOBRO_ADMISSION_REPORT: AdmissionReport = AdmissionReport::zeroed();

#[no_mangle]
#[used]
static mut NOBRO_RUNTIME_REPORT: RuntimeReport = RuntimeReport::zeroed();

#[no_mangle]
#[used]
static mut NOBRO_EVENT_LOG_REPORT: EventLogReport = EventLogReport::zeroed();

#[no_mangle]
#[used]
static mut NOBRO_MODULE_RUNTIME_REPORT: ModuleRuntimeReport = ModuleRuntimeReport::zeroed();

#[no_mangle]
#[used]
static mut NOBRO_DEGRADE_APPLICATION_REPORT: DegradeApplicationReport =
    DegradeApplicationReport::zeroed();

#[no_mangle]
#[used]
static mut NOBRO_ADAPTER_COMPAT_REPORT: AdapterCompatibilityReport =
    AdapterCompatibilityReport::zeroed();

#[no_mangle]
#[used]
static mut NOBRO_BOARD_PROFILE_REPORT: BoardProfileReport = BoardProfileReport::zeroed();

#[no_mangle]
#[used]
static mut NOBRO_BOARD_PACKAGE_REPORT: BoardPackageReport = BoardPackageReport::zeroed();

#[no_mangle]
#[used]
static mut NOBRO_MANIFEST_REPORT: ManifestReport = ManifestReport::zeroed();

type SalDemoBoot = BootAssembly<4, 4, 4, 4, 4, 4, 4, 4, 16>;

const SAL_DEMO_DEPENDENCIES: [StartupDependency; 2] = [
    StartupDependency::new(ModuleId::Actuator, ModuleId::Kernel),
    StartupDependency::new(ModuleId::Sensor, ModuleId::Kernel),
];

fn on_deadline_slot() {}

#[cortex_m_rt::entry]
fn main() -> ! {
    write_board_profile_report();
    admit_sal_demo();

    defmt::info!(
        "NobroRTOS sal_adapter_demo platform={} (sensor-stub, no NiusIMU)",
        Hal::PLATFORM_ID
    );

    Hal::acquire(Resource::Timer0, 1).unwrap_or_else(|_| defmt::panic!("Timer0"));

    let profile = Hal::servo_profile();
    unsafe {
        Hal::init_scheduling_demo(profile);
        Scheduler::set_deadline_handler(on_deadline_slot);
        ppi::led_init_output();
    }

    let mut servo = RoboServoAdapter::new(profile.pin);
    unsafe {
        servo
            .attach_50hz(profile.center_pulse_us)
            .unwrap_or_else(|_| defmt::panic!("servo attach"));
    }

    let mut sensor = SensorStub::new(2);
    defmt::info!(
        "adapters: robo-servo pin={} stub_i2c=0x{:02X}",
        profile.pin,
        sensor.stub_i2c_addr()
    );

    let mut stats = StatsTask::new(Hal::now_us());
    Scheduler::reset_stats();

    unsafe {
        NOBRO_SAL_EVAL_REPORT.magic = SAL_EVAL_MAGIC;
        NOBRO_SAL_EVAL_REPORT.version = airon_kernel::eval::SAL_EVAL_VERSION;
    }

    loop {
        let now = Hal::now_us();
        Hal::poll_compare(|t| {
            Scheduler::on_deadline_tick(t);
            try_servo_step(&mut servo, now);
        });

        if let Ok(Some(sample)) = sensor.poll() {
            IMU_SAMPLES.fetch_add(1, Ordering::AcqRel);
            if stub_imu_plausible(&sample) {
                IMU_PLAUSIBLE.fetch_add(1, Ordering::AcqRel);
            }
            SamplePool::release(sample.handle);
        }

        if stats.poll(now) == Poll::Ready {
            write_progress_report();
            try_finalize_eval();
        }

        if EVAL_DONE.load(Ordering::Acquire) != 0 {
            unsafe {
                ppi::led_toggle();
            }
            asm::delay(8_000_000);
        } else {
            asm::nop();
        }
    }
}

fn write_board_profile_report() {
    unsafe {
        NOBRO_BOARD_PROFILE_REPORT =
            BoardProfileReport::from_board::<<Hal as PlatformHal>::Board>();
        NOBRO_BOARD_PACKAGE_REPORT = BoardPackageReport::from_package(&ACTIVE_BOARD_PACKAGE);
    }
}

fn admit_sal_demo() {
    let specs = [kernel_spec(), robo_servo_spec(), sensor_stub_spec()];
    let profile = active_profile();
    let boot = match SalDemoBoot::build_with_failure(
        &specs,
        &SAL_DEMO_DEPENDENCIES,
        profile,
        FaultThresholds::DEFAULT,
        0,
    ) {
        Ok(boot) => boot,
        Err(failure) => {
            write_boot_reports(failure.reports());
            defmt::panic!("sal demo boot assembly failed");
        }
    };

    write_boot_reports(boot.reports());
    validate_adapter_set(profile);

    let runtime = boot.runtime;
    unsafe {
        NOBRO_RUNTIME_REPORT = runtime.runtime_report();
        NOBRO_EVENT_LOG_REPORT = runtime.event_log_report();
        NOBRO_MODULE_RUNTIME_REPORT = runtime.module_runtime_report();
        NOBRO_DEGRADE_APPLICATION_REPORT = runtime.degrade_application_report();
    }
}

fn write_boot_reports(reports: BootAssemblyReports) {
    unsafe {
        NOBRO_MANIFEST_REPORT = reports.manifest;
        NOBRO_ADMISSION_REPORT = reports.admission;
    }
}

fn validate_adapter_set(profile: SystemProfile) {
    let mut preflight = AdapterPreflight::<2>::new();
    let _ = preflight.add_manifest::<RoboServoAdapter>();
    let _ = preflight.add_manifest::<SensorStub>();
    let report = preflight.compatibility_report(profile);
    unsafe {
        NOBRO_ADAPTER_COMPAT_REPORT = report;
    }
    if report.compatible == 0 {
        defmt::panic!("adapter compatibility");
    }
}

fn active_profile() -> SystemProfile {
    SystemProfile::from_board_package(&ACTIVE_BOARD_PACKAGE)
        .unwrap_or_else(|_| defmt::panic!("board package profile"))
}

fn kernel_spec() -> ModuleSpec {
    kernel_module_spec(
        MemoryBudget::new(24 * 1024, 8 * 1024, 4),
        DeadlineContract::new(20_000, 10),
    )
}

fn try_servo_step(servo: &mut RoboServoAdapter, deadline_us: u64) {
    let steps = SERVO_STEPS.load(Ordering::Acquire);
    if steps >= MIN_SERVO_STEPS {
        return;
    }

    let cmd = unsafe { SERVO_CMD_US };
    if servo.set_duty_us(0, cmd, deadline_us).is_err() {
        return;
    }

    let readback = <Hal as HalServoPwm>::read_pulse_us();
    let delta = cmd.abs_diff(readback);
    if delta <= SERVO_READBACK_TOL_US {
        SERVO_READBACK_OK.fetch_add(1, Ordering::AcqRel);
    }
    SERVO_STEPS.fetch_add(1, Ordering::AcqRel);

    let next = next_pulse(cmd);
    unsafe {
        SERVO_CMD_US = next;
    }
}

fn next_pulse(cmd: u32) -> u32 {
    const MIN_US: u32 = 1200;
    const MAX_US: u32 = 1800;
    const STEP: u32 = 30;

    unsafe {
        let mut dir = SERVO_DIR;
        let mut next = cmd as i32 + (STEP as i32) * i32::from(dir as i32);
        if next >= MAX_US as i32 {
            next = MAX_US as i32;
            dir = -1;
        } else if next <= MIN_US as i32 {
            next = MIN_US as i32;
            dir = 1;
        }
        SERVO_DIR = dir;
        next as u32
    }
}

fn try_finalize_eval() {
    if EVAL_DONE.load(Ordering::Acquire) != 0 {
        return;
    }

    let servo_steps = SERVO_STEPS.load(Ordering::Acquire);
    let readback_ok = SERVO_READBACK_OK.load(Ordering::Acquire);
    let imu_samples = IMU_SAMPLES.load(Ordering::Acquire);
    let imu_plausible = IMU_PLAUSIBLE.load(Ordering::Acquire);

    defmt::info!(
        "sal eval progress servo={}/{} readback={} imu={}/{}",
        servo_steps,
        MIN_SERVO_STEPS,
        readback_ok,
        imu_plausible,
        imu_samples
    );

    if servo_steps < MIN_SERVO_STEPS
        || readback_ok < MIN_SERVO_STEPS
        || imu_samples < MIN_IMU_SAMPLES
        || imu_plausible < MIN_IMU_SAMPLES
    {
        return;
    }

    let mut report = SalEvalReport {
        servo_steps,
        servo_readback_ok: readback_ok,
        imu_samples,
        imu_plausible,
        ..SalEvalReport::zeroed()
    };
    report.seal();

    unsafe {
        NOBRO_SAL_EVAL_REPORT = report;
    }
    EVAL_DONE.store(1, Ordering::Release);

    defmt::info!(
        "NOBRO_SAL_EVAL_FINAL magic=0x{:08X} ALL=1 servo={} imu={}",
        SAL_EVAL_MAGIC,
        servo_steps,
        imu_samples
    );
}

fn write_progress_report() {
    if EVAL_DONE.load(Ordering::Acquire) != 0 {
        return;
    }
    let report = SalEvalReport {
        magic: SAL_EVAL_MAGIC,
        version: airon_kernel::eval::SAL_EVAL_VERSION,
        servo_steps: SERVO_STEPS.load(Ordering::Acquire),
        servo_readback_ok: SERVO_READBACK_OK.load(Ordering::Acquire),
        imu_samples: IMU_SAMPLES.load(Ordering::Acquire),
        imu_plausible: IMU_PLAUSIBLE.load(Ordering::Acquire),
        ..SalEvalReport::zeroed()
    };
    unsafe {
        NOBRO_SAL_EVAL_REPORT = report;
    }
}
