//! `libnobro.a` - the prebuilt NobroRTOS runtime for Tier C (C developers, no Rust).
//!
//! This staticlib is the Rust side of the C-ABI story compiled once: boot assembly,
//! admission, kernel drive loop, the `extern "C"` host services, and the vector
//! table/entry from cortex-m-rt. A C developer links their `nobro_app_init` /
//! `nobro_app_poll` module against it with arm-none-eabi-gcc - see
//! docs/USER_GUIDE.md and tools/build_libnobro.py.
//! NobroRTOS C ABI demo.
//!
//! Tier C runtime staticlib: provides the `extern "C"` host services (the NobroRTOS C ABI), admits a
//! module through `BootAssembly`, and drives a module's `extern "C"`
//! `nobro_app_init` / `nobro_app_poll` callbacks. The module logic is authored
//! against the C ABI (bindings/c/include/nobro_app.h) and provided either by the
//! Rust reference crate (feature `rust-module`, default) or compiled from C
//! (feature `c-source`, build.rs + arm-none-eabi-gcc) - the linked object is
//! byte-identical either way. A passing NOBRO_IMU_HEALTH_REPORT proves a module
//! authored outside Rust reads the IMU through the kernel's C ABI.
#![no_std]

use core::{
    ffi::{c_char, c_void},
    ptr,
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
};
use cortex_m::asm;
use defmt_rtt as _; // defmt.x linker section
use panic_halt as _;

use nobro_hal::{
    bus::TwimBus,
    lease::Resource,
    traits::{HalClock, HalLease, HalTimebaseProvider},
    ActivePlatform as Hal, Twim0, I2C_SCL_PIN, I2C_SDA_PIN,
};
use nobro_imu::{
    ImuHealthReport, IMU_HEALTH_REPORT_MAGIC, IMU_HEALTH_REPORT_VERSION, MIN_HEALTH_SAMPLES,
};
use nobro_kernel::{
    kernel_module_spec, AdmissionReport, BootAssembly, CApp, CAppError, CTaskOptions, CTaskRole,
    CTaskStep, Capability, CapabilitySet, CapabilityTraceOp, Criticality, DeadlineContract,
    FaultThresholds, ForeignHostCall, ForeignHostContext, ForeignHostError, ForeignHostQuota,
    ForeignModuleRunner, KernelError, ManifestReport, MemoryBudget, ModuleId, ModuleLaunchGate,
    ModuleSpec, StartupDependency, SystemProfile,
};

// ---- The NobroRTOS C ABI: host services callable from a C (or extern-"C") module ----

static MODULE_GATE: ModuleLaunchGate = ModuleLaunchGate::new();
static HOST_CONTEXT: ForeignHostContext<32> = ForeignHostContext::new(
    &MODULE_GATE,
    ForeignHostQuota::new(100_000, 4 * 1024 * 1024),
);

const C_TASK_CAPACITY: usize = 8;
const C_WIRE_CAPACITY: usize = 8;
const C_MODULE_CAPACITY: usize = C_TASK_CAPACITY + 1;

static mut C_APP: CApp<C_TASK_CAPACITY, C_WIRE_CAPACITY> = CApp::new();
static C_APP_RUNNING: AtomicBool = AtomicBool::new(false);
static LAST_STEP_ERROR: AtomicI32 = AtomicI32::new(0);

/// C layout for the compact explicit-override record in `nobro_app.h`.
#[repr(C)]
struct NobroTaskOptions {
    role: u32,
    budget_us: u32,
    deadline_us: u32,
    jitter_us: u32,
    blocking_us: u32,
}

unsafe fn c_app() -> &'static mut CApp<C_TASK_CAPACITY, C_WIRE_CAPACITY> {
    // The Tier-C entry loop is single-threaded. Registration finishes before
    // dispatch, and no interrupt calls this facade.
    unsafe { &mut *ptr::addr_of_mut!(C_APP) }
}

unsafe fn c_name(name: *const c_char) -> Result<&'static str, CAppError> {
    if name.is_null() {
        return Err(CAppError::InvalidName);
    }
    let mut len = 0usize;
    while len <= 48 {
        if unsafe { *name.add(len) } == 0 {
            break;
        }
        len += 1;
    }
    if len == 0 || len > 48 {
        return Err(CAppError::InvalidName);
    }
    let bytes = unsafe { core::slice::from_raw_parts(name.cast::<u8>(), len) };
    core::str::from_utf8(bytes).map_err(|_| CAppError::InvalidName)
}

fn c_options(options: *const c_void) -> Result<CTaskOptions, CAppError> {
    if options.is_null() {
        return Ok(CTaskOptions::DEFAULT);
    }
    let options = unsafe { &*options.cast::<NobroTaskOptions>() };
    let role = match options.role {
        0 => CTaskRole::Periodic,
        1 => CTaskRole::Control,
        2 => CTaskRole::Service,
        _ => return Err(CAppError::InvalidOptions),
    };
    Ok(CTaskOptions {
        role,
        budget_us: options.budget_us,
        deadline_us: options.deadline_us,
        jitter_us: options.jitter_us,
        blocking_us: options.blocking_us,
    })
}

fn status(result: Result<(), CAppError>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(error) => error.status(),
    }
}

/// Register one periodic task with the beginner-safe defaults.
///
/// # Safety
/// `name` must point to a static NUL-terminated string that remains valid for
/// the firmware lifetime. `step`, when present, must be a valid C callback.
#[no_mangle]
pub unsafe extern "C" fn nobro_task(
    name: *const c_char,
    period_us: u32,
    step: Option<CTaskStep>,
) -> i32 {
    let Some(step) = step else {
        return CAppError::InvalidOptions.status();
    };
    let name = match unsafe { c_name(name) } {
        Ok(name) => name,
        Err(error) => return error.status(),
    };
    status(unsafe { c_app() }.task(name, period_us, step, CTaskOptions::DEFAULT))
}

/// Register one task with a compact explicit role/timing override record.
///
/// # Safety
/// `name` must point to a static NUL-terminated string and `options`, when
/// non-null, must point to a readable `nobro_task_options_t`. Both must remain
/// valid for the call; the name storage must remain valid for firmware life.
#[no_mangle]
pub unsafe extern "C" fn nobro_task_with(
    name: *const c_char,
    period_us: u32,
    step: Option<CTaskStep>,
    options: *const c_void,
) -> i32 {
    let Some(step) = step else {
        return CAppError::InvalidOptions.status();
    };
    let name = match unsafe { c_name(name) } {
        Ok(name) => name,
        Err(error) => return error.status(),
    };
    let options = match c_options(options) {
        Ok(options) => options,
        Err(error) => return error.status(),
    };
    status(unsafe { c_app() }.task(name, period_us, step, options))
}

/// Declare a bounded graph relationship between two registered tasks.
///
/// # Safety
/// `from` and `to` must point to static NUL-terminated strings that remain
/// valid for the firmware lifetime.
#[no_mangle]
pub unsafe extern "C" fn nobro_wire(from: *const c_char, to: *const c_char, capacity: u32) -> i32 {
    let from = match unsafe { c_name(from) } {
        Ok(name) => name,
        Err(error) => return error.status(),
    };
    let to = match unsafe { c_name(to) } {
        Ok(name) => name,
        Err(error) => return error.status(),
    };
    let Ok(capacity) = u8::try_from(capacity) else {
        return CAppError::WireCapacity.status();
    };
    status(unsafe { c_app() }.wire(from, to, capacity))
}

/// Admit the complete declaration through the shared Rust graph validator.
///
/// # Safety
/// Call once from the Tier-C initialization callback after registration. The
/// single-threaded Tier-C runtime must remain the sole owner of the C app state.
#[no_mangle]
pub unsafe extern "C" fn nobro_run() -> i32 {
    let result = unsafe { c_app() }.run::<C_MODULE_CAPACITY>(
        SystemProfile::new(192 * 1024, 64 * 1024, 16, C_MODULE_CAPACITY),
        Hal::now_us(),
    );
    if result.is_ok() {
        C_APP_RUNNING.store(true, Ordering::Release);
    }
    status(result)
}

/// Dispatch all currently due registered callbacks once.
///
/// # Safety
/// Call only from the Tier-C poll callback after `nobro_run` succeeds. No
/// interrupt may concurrently access the C app state.
#[no_mangle]
pub unsafe extern "C" fn nobro_poll() -> i32 {
    match unsafe { c_app() }.poll_at(Hal::now_us()) {
        Ok(_) => 0,
        Err(error @ CAppError::StepFailed { code, .. }) => {
            LAST_STEP_ERROR.store(code, Ordering::Relaxed);
            error.status()
        }
        Err(error) => error.status(),
    }
}

#[no_mangle]
/// Return the saturated count of releases skipped by late polling.
///
/// # Safety
/// No interrupt may concurrently access the single-threaded C app state.
pub unsafe extern "C" fn nobro_skipped_releases() -> u32 {
    unsafe { c_app() }.skipped_releases()
}

#[no_mangle]
pub extern "C" fn nobro_last_step_error() -> i32 {
    LAST_STEP_ERROR.load(Ordering::Relaxed)
}

fn host_error(error: ForeignHostError) -> i32 {
    match error {
        ForeignHostError::NotAdmitted | ForeignHostError::CapabilityDenied => -2,
        ForeignHostError::QuotaExceeded => -3,
    }
}

#[no_mangle]
pub extern "C" fn nobro_now_us() -> u64 {
    let mut now = 0;
    if HOST_CONTEXT
        .invoke(
            ForeignHostCall::new(Capability::Timebase, CapabilityTraceOp::Read, 0),
            || {
                now = Hal::now_us();
                0
            },
        )
        .is_ok()
    {
        now
    } else {
        0
    }
}

#[no_mangle]
/// Write one bounded C byte slice through the admitted Tier-C I2C service.
///
/// # Safety
/// For nonzero `len`, `tx` must point to at least `len` readable bytes for the
/// duration of this call.
pub unsafe extern "C" fn nobro_i2c_write(addr: u8, tx: *const u8, len: u32) -> i32 {
    if tx.is_null() {
        return -1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(tx, len as usize) };
    match HOST_CONTEXT.invoke(
        ForeignHostCall::new(Capability::Bus0, CapabilityTraceOp::Write, Hal::now_us())
            .args(u32::from(addr), len)
            .bytes(len),
        || match unsafe { Twim0::write_bytes(addr, bytes) } {
            Ok(()) => 0,
            Err(_) => -1,
        },
    ) {
        Ok(result) => result,
        Err(error) => host_error(error),
    }
}

#[no_mangle]
/// Perform one bounded write/read transaction through the admitted Tier-C I2C service.
///
/// # Safety
/// For nonzero lengths, `tx` must point to `tx_len` readable bytes and `rx`
/// must point to `rx_len` writable bytes for the duration of this call. The
/// regions must satisfy Rust's aliasing rules.
pub unsafe extern "C" fn nobro_i2c_write_read(
    addr: u8,
    tx: *const u8,
    tx_len: u32,
    rx: *mut u8,
    rx_len: u32,
) -> i32 {
    if tx.is_null() || rx.is_null() {
        return -1;
    }
    let t = unsafe { core::slice::from_raw_parts(tx, tx_len as usize) };
    let r = unsafe { core::slice::from_raw_parts_mut(rx, rx_len as usize) };
    match HOST_CONTEXT.invoke(
        ForeignHostCall::new(Capability::Bus0, CapabilityTraceOp::Write, Hal::now_us())
            .args(u32::from(addr), tx_len.saturating_add(rx_len))
            .bytes(tx_len.saturating_add(rx_len)),
        || match unsafe { Twim0::write_read(addr, t, r) } {
            Ok(()) => 0,
            Err(_) => -1,
        },
    ) {
        Ok(result) => result,
        Err(error) => host_error(error),
    }
}

static mut READS: u32 = 0;

/// Module publishes a parsed IMU sample; the kernel side computes magnitudes and
/// fills the standard report (keeps the module free of floating-point/math deps).
#[no_mangle]
pub extern "C" fn nobro_publish_imu(
    who: u8,
    device_address: u8,
    ax: i16,
    ay: i16,
    az: i16,
    gx: i16,
    gy: i16,
    gz: i16,
    temp_raw: i16,
) {
    let _ = HOST_CONTEXT.invoke(
        ForeignHostCall::new(
            Capability::HostReport,
            CapabilityTraceOp::Write,
            Hal::now_us(),
        )
        .args(u32::from(who), u32::from(device_address))
        .bytes(18),
        || {
            let (axf, ayf, azf) = (
                ax as f32 / 16_384.0,
                ay as f32 / 16_384.0,
                az as f32 / 16_384.0,
            );
            let accel_mg = (libm::sqrtf(axf * axf + ayf * ayf + azf * azf) * 1000.0) as u32;
            let (gxf, gyf, gzf) = (gx as f32 / 131.0, gy as f32 / 131.0, gz as f32 / 131.0);
            let gyro_mdps = (libm::sqrtf(gxf * gxf + gyf * gyf + gzf * gzf) * 1000.0) as u32;
            let tc = temp_raw as f32 / 333.87 + 21.0;
            let temp_centi = if tc > 0.0 { (tc * 100.0) as u32 } else { 0 };
            unsafe {
                READS += 1;
                NOBRO_IMU_HEALTH_REPORT.who_am_i = u32::from(who);
                NOBRO_IMU_HEALTH_REPORT.device_address = u32::from(device_address);
                NOBRO_IMU_HEALTH_REPORT.devices_seen = 1;
                NOBRO_IMU_HEALTH_REPORT.samples = READS;
                NOBRO_IMU_HEALTH_REPORT.read_errors = 0;
                NOBRO_IMU_HEALTH_REPORT.accel_mag_mg = accel_mg;
                NOBRO_IMU_HEALTH_REPORT.gyro_mag_mdps = gyro_mdps;
                NOBRO_IMU_HEALTH_REPORT.temperature_centi_c = temp_centi;
            }
            0
        },
    );
}

// ---- Module callbacks (provided by the rust-module crate or the compiled C) ----
extern "C" {
    fn nobro_app_init() -> i32;
    fn nobro_app_poll() -> i32;
}

#[no_mangle]
#[used]
static mut NOBRO_IMU_HEALTH_REPORT: ImuHealthReport = ImuHealthReport::zeroed();
#[no_mangle]
#[used]
static mut NOBRO_MANIFEST_REPORT: ManifestReport = ManifestReport::zeroed();
#[no_mangle]
#[used]
static mut NOBRO_ADMISSION_REPORT: AdmissionReport = AdmissionReport::zeroed();

type CDemoBoot = BootAssembly<4, 4, 4, 4, 4, 4, 4, 4, 16>;

fn idle() -> ! {
    loop {
        asm::delay(16_000_000);
    }
}

fn admit() -> Option<CDemoBoot> {
    let specs = [
        kernel_module_spec(
            MemoryBudget::new(24 * 1024, 8 * 1024, 4),
            DeadlineContract::new(20_000, 10),
        ),
        ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
            .requires(
                CapabilitySet::empty()
                    .with(Capability::Bus0)
                    .with(Capability::SamplePool)
                    .with(Capability::Timebase)
                    .with(Capability::HostReport),
            )
            .owns(CapabilitySet::empty().with(Capability::Bus0))
            .memory(MemoryBudget::new(30 * 1024, 2 * 1024, 2)),
    ];
    let deps = [StartupDependency::new(ModuleId::Sensor, ModuleId::Kernel)];
    // System budget the admitted modules must fit within (flash, RAM, pool slots,
    // max modules). Generous for the kernel + one sensor module.
    let profile = SystemProfile::new(192 * 1024, 64 * 1024, 8, 4);
    let result = CDemoBoot::build_with_failure(&specs, &deps, profile, FaultThresholds::DEFAULT, 0);
    let reports = match &result {
        Ok(boot) => boot.reports(),
        Err(failure) => failure.reports(),
    };
    unsafe {
        NOBRO_MANIFEST_REPORT = reports.manifest;
        NOBRO_ADMISSION_REPORT = reports.admission;
    }
    result.ok()
}

#[cortex_m_rt::entry]
fn main() -> ! {
    Hal::acquire(Resource::Timer0, 2).ok();
    unsafe {
        Hal::init_timebase();
    }
    Hal::acquire(Resource::Twim0, 3).ok();
    unsafe { TwimBus::init_pins_unchecked(I2C_SDA_PIN, I2C_SCL_PIN) };

    unsafe {
        NOBRO_IMU_HEALTH_REPORT.magic = IMU_HEALTH_REPORT_MAGIC;
        NOBRO_IMU_HEALTH_REPORT.version = IMU_HEALTH_REPORT_VERSION;
    }

    let Some(mut boot) = admit() else { idle() };
    let granted = boot.runtime.plan().grants.granted(ModuleId::Sensor);
    let mut foreign = ForeignModuleRunner::new(&MODULE_GATE);
    if foreign.admit(granted).is_err() {
        idle();
    }
    if let Some(granted) = granted {
        HOST_CONTEXT.admit(ModuleId::Sensor, granted);
    } else {
        idle();
    }
    if foreign.initialize(|| unsafe { nobro_app_init() }).is_err() {
        let _ = boot.runtime.record_error(
            ModuleId::Sensor,
            KernelError::ForeignModuleInitFail,
            Hal::now_us(),
        );
        idle();
    }
    loop {
        if foreign.poll(|| unsafe { nobro_app_poll() }).is_err() {
            let _ = boot.runtime.record_error(
                ModuleId::Sensor,
                KernelError::ForeignModulePollFail,
                Hal::now_us(),
            );
            idle();
        }
        if unsafe { READS } >= MIN_HEALTH_SAMPLES {
            let mut report = unsafe { NOBRO_IMU_HEALTH_REPORT };
            report.seal();
            unsafe {
                NOBRO_IMU_HEALTH_REPORT = report;
            }
        }
        // Declarative task rates are checked from the microsecond clock, so poll
        // them at a sub-default-jitter cadence. The legacy single-callback ABI
        // keeps its historical relaxed cadence. This remains a busy-poll Tier-C
        // composition; a later power-provider slice may replace it with compare/WFE.
        asm::delay(if C_APP_RUNNING.load(Ordering::Acquire) {
            1_000
        } else {
            400_000
        });
    }
}
