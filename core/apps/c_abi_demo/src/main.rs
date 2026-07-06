//! NobroRTOS C ABI demo.
//!
//! The app provides the `extern "C"` host services (the NobroRTOS C ABI), admits a
//! module through `BootAssembly`, and drives a module's `extern "C"`
//! `nobro_app_init` / `nobro_app_poll` callbacks. The module logic is authored
//! against the C ABI (bindings/c/include/nobro_app.h) and provided either by the
//! Rust reference crate (feature `rust-module`, default) or compiled from C
//! (feature `c-source`, build.rs + arm-none-eabi-gcc) - the linked object is
//! byte-identical either way. A passing NOBRO_IMU_HW_EVAL_REPORT proves a module
//! authored outside Rust reads the IMU through the kernel's C ABI.
#![no_std]
#![no_main]

use cortex_m::asm;
use defmt_rtt as _; // defmt.x linker section
use panic_halt as _;

#[cfg(feature = "rust-module")]
use nobro_c_abi_module as _; // force-link the module's extern "C" symbols

use nobro_hal::{
    bus::TwimBus,
    lease::Resource,
    traits::{HalClock, HalLease, PlatformHal},
    ActivePlatform as Hal, Twim0, I2C_SCL_PIN, I2C_SDA_PIN,
};
use nobro_kernel::{
    eval::{ImuHwEvalReport, IMU_HW_EVAL_MAGIC, IMU_HW_EVAL_VERSION, MIN_IMU_HW_READS},
    kernel_module_spec, AdmissionReport, BootAssembly, Capability, CapabilitySet, Criticality,
    DeadlineContract, FaultThresholds, ManifestReport, MemoryBudget, ModuleId, ModuleSpec,
    StartupDependency, SystemProfile,
};

// ---- The NobroRTOS C ABI: host services callable from a C (or extern-"C") module ----

#[no_mangle]
pub extern "C" fn nobro_now_us() -> u64 {
    Hal::now_us()
}

#[no_mangle]
pub extern "C" fn nobro_i2c_write(addr: u8, tx: *const u8, len: u32) -> i32 {
    if tx.is_null() {
        return -1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(tx, len as usize) };
    match Twim0::write_bytes(addr, bytes) {
        Ok(()) => 0,
        Err(_) => -1,
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
    match Twim0::write_read(addr, t, r) {
        Ok(()) => 0,
        Err(_) => -1,
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

fn admit() {
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
                    .with(Capability::Timebase),
            )
            .owns(CapabilitySet::empty().with(Capability::Bus0))
            .memory(MemoryBudget::new(30 * 1024, 2 * 1024, 2)),
    ];
    let deps = [StartupDependency::new(ModuleId::Sensor, ModuleId::Kernel)];
    // System budget the admitted modules must fit within (flash, RAM, pool slots,
    // max modules). Generous for the kernel + one sensor module.
    let profile = SystemProfile::new(192 * 1024, 64 * 1024, 8, 4);
    let reports =
        match CDemoBoot::build_with_failure(&specs, &deps, profile, FaultThresholds::DEFAULT, 0) {
            Ok(boot) => boot.reports(),
            Err(failure) => failure.reports(),
        };
    unsafe {
        NOBRO_MANIFEST_REPORT = reports.manifest;
        NOBRO_ADMISSION_REPORT = reports.admission;
    }
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

    admit();

    if unsafe { nobro_app_init() } < 0 {
        idle();
    }
    loop {
        let _ = unsafe { nobro_app_poll() };
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
