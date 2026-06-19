//! Phase 2 SAL adapter demo with RoboServo and sensor-stub.
//!
//! Autonomous eval writes `AIRON_SAL_EVAL_REPORT` for J-Link mem32 readback.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::asm;
use defmt_rtt as _;
use panic_probe as _;

use airon_adapter_robo_servo::{module_spec as robo_servo_spec, RoboServoAdapter};
use airon_adapter_sensor_stub::{module_spec as sensor_stub_spec, stub_imu_plausible, SensorStub};
use airon_hal::{
    board_desc::BoardDesc,
    lease::Resource,
    ppi,
    traits::{HalClock, HalDeadline, HalLease, HalServoPwm, PlatformHal},
    ActivePlatform as Hal, BoardProfileReport,
};
use airon_kernel::{
    eval::{
        SalEvalReport, MIN_IMU_SAMPLES, MIN_SERVO_STEPS, SAL_EVAL_MAGIC, SERVO_READBACK_TOL_US,
    },
    executor::{Poll, StatsTask, Task},
    kernel_module_spec,
    pool::SamplePool,
    scheduler::Scheduler,
    AdmissionController, AdmissionReport, DeadlineContract, EventLogReport, FaultThresholds,
    ManifestReport, MemoryBudget, ModuleId, ModuleSpec, Runtime, RuntimeReport, SystemManifest,
    SystemProfile,
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
static mut AIRON_SAL_EVAL_REPORT: SalEvalReport = SalEvalReport::zeroed();

#[no_mangle]
#[used]
static mut AIRON_ADMISSION_REPORT: AdmissionReport = AdmissionReport::zeroed();

#[no_mangle]
#[used]
static mut AIRON_RUNTIME_REPORT: RuntimeReport = RuntimeReport::zeroed();

#[no_mangle]
#[used]
static mut AIRON_EVENT_LOG_REPORT: EventLogReport = EventLogReport::zeroed();

#[no_mangle]
#[used]
static mut AIRON_ADAPTER_COMPAT_REPORT: AdapterCompatibilityReport =
    AdapterCompatibilityReport::zeroed();

#[no_mangle]
#[used]
static mut AIRON_BOARD_PROFILE_REPORT: BoardProfileReport = BoardProfileReport::zeroed();

#[no_mangle]
#[used]
static mut AIRON_MANIFEST_REPORT: ManifestReport = ManifestReport::zeroed();

type SalDemoRuntime = Runtime<4, 4, 4, 4, 4, 4, 16>;

fn on_deadline_slot() {}

#[cortex_m_rt::entry]
fn main() -> ! {
    write_board_profile_report();
    admit_sal_demo();

    defmt::info!(
        "AIRON sal_adapter_demo platform={} (sensor-stub, no NiusIMU)",
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
        AIRON_SAL_EVAL_REPORT.magic = SAL_EVAL_MAGIC;
        AIRON_SAL_EVAL_REPORT.version = airon_kernel::eval::SAL_EVAL_VERSION;
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
        AIRON_BOARD_PROFILE_REPORT =
            BoardProfileReport::from_board::<<Hal as PlatformHal>::Board>();
    }
}

fn admit_sal_demo() {
    let specs = [kernel_spec(), robo_servo_spec(), sensor_stub_spec()];
    let manifest =
        SystemManifest::<4>::from_specs(&specs).unwrap_or_else(|_| defmt::panic!("manifest"));
    unsafe {
        AIRON_MANIFEST_REPORT =
            ManifestReport::from_result(&manifest, manifest.validate_profile(active_profile()));
    }
    validate_adapter_set(active_profile());

    let mut startup = manifest
        .startup_graph::<4>()
        .unwrap_or_else(|_| defmt::panic!("startup graph"));
    startup
        .add_dependency(ModuleId::Actuator, ModuleId::Kernel)
        .unwrap_or_else(|_| defmt::panic!("actuator startup dependency"));
    startup
        .add_dependency(ModuleId::Sensor, ModuleId::Kernel)
        .unwrap_or_else(|_| defmt::panic!("sensor startup dependency"));

    let admission =
        AdmissionController::admit_graph::<4, 4, 4, 4>(&manifest, &startup, active_profile());
    unsafe {
        AIRON_ADMISSION_REPORT =
            AdmissionReport::from_result(admission.as_ref().map_err(|error| *error));
    }

    match admission {
        Ok(plan) => {
            let mut runtime = SalDemoRuntime::from_plan(plan, FaultThresholds::DEFAULT);
            runtime
                .boot_to_running(0)
                .unwrap_or_else(|_| defmt::panic!("runtime boot"));
            unsafe {
                AIRON_RUNTIME_REPORT = runtime.runtime_report();
                AIRON_EVENT_LOG_REPORT = runtime.event_log_report();
            }
        }
        Err(_) => {
            defmt::panic!("sal demo admission failed");
        }
    }
}

fn validate_adapter_set(profile: SystemProfile) {
    let mut preflight = AdapterPreflight::<2>::new();
    let _ = preflight.add_manifest::<RoboServoAdapter>();
    let _ = preflight.add_manifest::<SensorStub>();
    let report = preflight.compatibility_report(profile);
    unsafe {
        AIRON_ADAPTER_COMPAT_REPORT = report;
    }
    if report.compatible == 0 {
        defmt::panic!("adapter compatibility");
    }
}

fn active_profile() -> SystemProfile {
    let capacity = <Hal as PlatformHal>::Board::CAPACITY;
    SystemProfile::new(
        capacity.flash_budget_bytes,
        capacity.ram_budget_bytes,
        capacity.sample_pool_slots,
        capacity.max_modules,
    )
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
        AIRON_SAL_EVAL_REPORT = report;
    }
    EVAL_DONE.store(1, Ordering::Release);

    defmt::info!(
        "AIRON_SAL_EVAL_FINAL magic=0x{:08X} ALL=1 servo={} imu={}",
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
        AIRON_SAL_EVAL_REPORT = report;
    }
}
