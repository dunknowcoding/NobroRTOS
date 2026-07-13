use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::asm;
use defmt_rtt as _;
use panic_probe as _;

use nobro_adapter_mpu9250_imu::Mpu9250Imu;
use nobro_hal::{
    board_desc::BoardDesc,
    lease::Resource,
    ppi,
    traits::{HalClock, HalLease, HalTimebaseProvider},
    ActivePlatform as Hal, Board,
};
use nobro_imu::{
    ImuHealthReport, IMU_HEALTH_REPORT_MAGIC, IMU_HEALTH_REPORT_VERSION, MIN_HEALTH_SAMPLES,
};
use nobro_kernel::pool::SamplePool;
use nobro_sal::SensorSal;

static IMU_READS: AtomicU32 = AtomicU32::new(0);
static IMU_ERRORS: AtomicU32 = AtomicU32::new(0);
static LAST_MAG_MG: AtomicU32 = AtomicU32::new(0);
static LAST_TEMP_CENTI: AtomicU32 = AtomicU32::new(0);
static LAST_GYRO_MDPS: AtomicU32 = AtomicU32::new(0);
static HEALTH_READY: AtomicU32 = AtomicU32::new(0);

const OWNER_TWIM: u8 = 3;

#[no_mangle]
#[used]
static mut NOBRO_IMU_HEALTH_REPORT: ImuHealthReport = ImuHealthReport::zeroed();

#[cortex_m_rt::entry]
fn main() -> ! {
    defmt::info!(
        "NobroRTOS imu_i2c_demo board={} flash=0x{:X}",
        Board::BOARD_ID,
        Board::APP_FLASH_START
    );

    Hal::acquire(Resource::Timer0, 2).unwrap_or_else(|_| defmt::panic!("Timer0"));
    unsafe {
        Hal::init_timebase();
        ppi::led_init_output();
    }
    unsafe {
        NOBRO_IMU_HEALTH_REPORT.magic = IMU_HEALTH_REPORT_MAGIC;
        NOBRO_IMU_HEALTH_REPORT.version = IMU_HEALTH_REPORT_VERSION;
    }

    let device_count = Mpu9250Imu::scan_device_count(OWNER_TWIM).unwrap_or(0);
    defmt::info!("I2C scan: {} device(s)", device_count);
    unsafe {
        NOBRO_IMU_HEALTH_REPORT.devices_seen = u32::from(device_count);
    }

    let mut imu = match Mpu9250Imu::probe_and_init(OWNER_TWIM) {
        Ok(imu) => imu,
        Err(_) => {
            defmt::warn!("MPU probe failed; check SDA/SCL wiring");
            idle_fail_loop();
        }
    };

    defmt::info!(
        "IMU addr=0x{:02X} WHO_AM_I=0x{:02X} bmp280={}",
        imu.addr(),
        imu.who_am_i(),
        imu.companion_present()
    );

    unsafe {
        NOBRO_IMU_HEALTH_REPORT.who_am_i = u32::from(imu.who_am_i());
        NOBRO_IMU_HEALTH_REPORT.device_address = u32::from(imu.addr());
        NOBRO_IMU_HEALTH_REPORT.devices_seen = u32::from(device_count);
        NOBRO_IMU_HEALTH_REPORT.companion_present = u32::from(imu.companion_present());
    }

    let mut last_report_ms = 0u64;

    loop {
        let now = Hal::now_us();

        match imu.poll() {
            Ok(Some(sample)) => {
                IMU_READS.fetch_add(1, Ordering::AcqRel);
                if let Some(payload) = nobro_kernel::CompactImuPayload::read_from_handle(sample.handle) {
                    let canonical = payload.into_sample(sample.captured_us);
                    if (800..1300).contains(&canonical.accel_mag_mg) {
                        LAST_MAG_MG.store(canonical.accel_mag_mg, Ordering::Release);
                    } else {
                        IMU_ERRORS.fetch_add(1, Ordering::AcqRel);
                    }
                }
                SamplePool::release(sample.handle);
            }
            Ok(None) => {}
            Err(_) => {
                IMU_ERRORS.fetch_add(1, Ordering::AcqRel);
            }
        }
        LAST_TEMP_CENTI.store(imu.last_temp_centi_c(), Ordering::Release);
        LAST_GYRO_MDPS.store(imu.last_gyro_mag_mdps(), Ordering::Release);

        if now / 1_000_000 >= last_report_ms + 2 {
            last_report_ms = now / 1_000_000;
            write_progress_report();
            try_finalize_eval();
            defmt::info!(
                "imu progress reads={} err={} |a|={}mg",
                IMU_READS.load(Ordering::Acquire),
                IMU_ERRORS.load(Ordering::Acquire),
                LAST_MAG_MG.load(Ordering::Acquire)
            );
        }

        if HEALTH_READY.load(Ordering::Acquire) != 0 {
            unsafe {
                ppi::led_toggle();
            }
            asm::delay(8_000_000);
        } else {
            asm::delay(200_000);
        }
    }
}

fn write_progress_report() {
    if HEALTH_READY.load(Ordering::Acquire) != 0 {
        return;
    }
    unsafe {
        NOBRO_IMU_HEALTH_REPORT.samples = IMU_READS.load(Ordering::Acquire);
        NOBRO_IMU_HEALTH_REPORT.read_errors = IMU_ERRORS.load(Ordering::Acquire);
        NOBRO_IMU_HEALTH_REPORT.accel_mag_mg = LAST_MAG_MG.load(Ordering::Acquire);
        NOBRO_IMU_HEALTH_REPORT.gyro_mag_mdps = LAST_GYRO_MDPS.load(Ordering::Acquire);
        NOBRO_IMU_HEALTH_REPORT.temperature_centi_c = LAST_TEMP_CENTI.load(Ordering::Acquire);
    }
}

fn try_finalize_eval() {
    if HEALTH_READY.load(Ordering::Acquire) != 0 {
        return;
    }

    let reads = IMU_READS.load(Ordering::Acquire);
    let errors = IMU_ERRORS.load(Ordering::Acquire);
    let mag = LAST_MAG_MG.load(Ordering::Acquire);

    if reads < MIN_HEALTH_SAMPLES || errors * 100 > reads {
        return;
    }

    let mut report = ImuHealthReport {
        who_am_i: unsafe { NOBRO_IMU_HEALTH_REPORT.who_am_i },
        device_address: unsafe { NOBRO_IMU_HEALTH_REPORT.device_address },
        devices_seen: unsafe { NOBRO_IMU_HEALTH_REPORT.devices_seen },
        companion_present: unsafe { NOBRO_IMU_HEALTH_REPORT.companion_present },
        samples: reads,
        read_errors: errors,
        accel_mag_mg: mag,
        gyro_mag_mdps: LAST_GYRO_MDPS.load(Ordering::Acquire),
        temperature_centi_c: LAST_TEMP_CENTI.load(Ordering::Acquire),
        ..ImuHealthReport::zeroed()
    };
    report.seal();

    unsafe {
        NOBRO_IMU_HEALTH_REPORT = report;
    }
    HEALTH_READY.store(1, Ordering::Release);

    defmt::info!(
        "NOBRO_IMU_HEALTH_READY samples={} accel={}mg companion={}",
        reads,
        mag,
        report.companion_present
    );
}

fn idle_fail_loop() -> ! {
    loop {
        unsafe {
            ppi::led_toggle();
        }
        asm::delay(16_000_000);
    }
}
