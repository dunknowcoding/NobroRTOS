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
//! byte-identical either way. A passing NOBRO_IMU_HW_EVAL_REPORT proves a module
//! authored outside Rust reads the IMU through the kernel's C ABI.
#![no_std]

use cortex_m::asm;
use defmt_rtt as _; // defmt.x linker section
use panic_halt as _;

use nobro_hal::{
    bus::TwimBus,
    lease::Resource,
    traits::{HalClock, HalLease, PlatformHal},
    ActivePlatform as Hal, Twim0, I2C_SCL_PIN, I2C_SDA_PIN,
};
use nobro_kernel::{
    eval::{ImuHwEvalReport, IMU_HW_EVAL_MAGIC, IMU_HW_EVAL_VERSION, MIN_IMU_HW_READS},
    kernel_module_spec, AdmissionReport, BootAssembly, Capability, CapabilitySet,
    CapabilityTraceOp, Criticality, DeadlineContract, FaultThresholds, ForeignHostCall,
    ForeignHostContext, ForeignHostError, ForeignHostQuota, ForeignModuleRunner, KernelError,
    ManifestReport, MemoryBudget, ModuleId, ModuleLaunchGate, ModuleSpec, StartupDependency,
    SystemProfile,
};

// ---- The NobroRTOS C ABI: host services callable from a C (or extern-"C") module ----

static MODULE_GATE: ModuleLaunchGate = ModuleLaunchGate::new();
static HOST_CONTEXT: ForeignHostContext<32> = ForeignHostContext::new(
    &MODULE_GATE,
    ForeignHostQuota::new(100_000, 4 * 1024 * 1024),
);

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
pub extern "C" fn nobro_i2c_write(addr: u8, tx: *const u8, len: u32) -> i32 {
    if tx.is_null() {
        return -1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(tx, len as usize) };
    match HOST_CONTEXT.invoke(
        ForeignHostCall::new(Capability::Bus0, CapabilityTraceOp::Write, Hal::now_us())
            .args(u32::from(addr), len)
            .bytes(len),
        || match Twim0::write_bytes(addr, bytes) {
            Ok(()) => 0,
            Err(_) => -1,
        },
    ) {
        Ok(result) => result,
        Err(error) => host_error(error),
    }
}

#[no_mangle]
pub extern "C" fn nobro_i2c_write_read(
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
        || match Twim0::write_read(addr, t, r) {
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
    dev_addr: u8,
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
        .args(u32::from(who), u32::from(dev_addr))
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
                NOBRO_IMU_HW_EVAL_REPORT.board_id_tag = 1;
                NOBRO_IMU_HW_EVAL_REPORT.who_am_i = u32::from(who);
                NOBRO_IMU_HW_EVAL_REPORT.dev_addr = u32::from(dev_addr);
                NOBRO_IMU_HW_EVAL_REPORT.i2c_devices = 1;
                NOBRO_IMU_HW_EVAL_REPORT.imu_reads = READS;
                NOBRO_IMU_HW_EVAL_REPORT.imu_errors = 0;
                NOBRO_IMU_HW_EVAL_REPORT.accel_mag_mg = accel_mg;
                NOBRO_IMU_HW_EVAL_REPORT.gyro_mag_mdps = gyro_mdps;
                NOBRO_IMU_HW_EVAL_REPORT.temp_centi_c = temp_centi;
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
static mut NOBRO_IMU_HW_EVAL_REPORT: ImuHwEvalReport = ImuHwEvalReport::zeroed();
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
    TwimBus::init_pins(I2C_SDA_PIN, I2C_SCL_PIN);

    unsafe {
        NOBRO_IMU_HW_EVAL_REPORT.magic = IMU_HW_EVAL_MAGIC;
        NOBRO_IMU_HW_EVAL_REPORT.version = IMU_HW_EVAL_VERSION;
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
        if unsafe { READS } >= MIN_IMU_HW_READS {
            let mut report = unsafe { NOBRO_IMU_HW_EVAL_REPORT };
            report.seal();
            unsafe {
                NOBRO_IMU_HW_EVAL_REPORT = report;
            }
        }
        asm::delay(400_000);
    }
}
