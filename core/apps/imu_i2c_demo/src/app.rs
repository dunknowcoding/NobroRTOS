use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::asm;
use defmt_rtt as _;
use panic_probe as _;

use nobro_adapter_mpu9250_imu::{accel_mag_mg, imu_plausible, Mpu9250Imu};
use nobro_hal::{
    board_desc::BoardDesc,
    lease::Resource,
    ppi,
    traits::{HalClock, HalLease, PlatformHal},
    ActivePlatform as Hal, Board,
};
use nobro_kernel::{
    eval::{ImuHwEvalReport, IMU_HW_EVAL_MAGIC, MIN_IMU_HW_READS},
    pool::SamplePool,
};
use nobro_sal::SensorSal;

static IMU_READS: AtomicU32 = AtomicU32::new(0);
static IMU_ERRORS: AtomicU32 = AtomicU32::new(0);
static LAST_MAG_MG: AtomicU32 = AtomicU32::new(0);
static EVAL_DONE: AtomicU32 = AtomicU32::new(0);

const OWNER_TWIM: u8 = 3;

#[no_mangle]
#[used]
static mut NOBRO_IMU_HW_EVAL_REPORT: ImuHwEvalReport = ImuHwEvalReport::zeroed();

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
    Hal::acquire(Resource::Twim0, OWNER_TWIM).unwrap_or_else(|_| defmt::panic!("TWIM0"));

    unsafe {
        NOBRO_IMU_HW_EVAL_REPORT.magic = IMU_HW_EVAL_MAGIC;
        NOBRO_IMU_HW_EVAL_REPORT.version = nobro_kernel::eval::IMU_HW_EVAL_VERSION;
    }

    let device_count = Mpu9250Imu::scan_device_count();
    defmt::info!("I2C scan: {} device(s)", device_count);
    unsafe {
        NOBRO_IMU_HW_EVAL_REPORT.i2c_devices = u32::from(device_count);
    }

    let mut imu = match Mpu9250Imu::probe_and_init(OWNER_TWIM) {
        Ok(imu) => imu,
        Err(_) => {
            if let Ok(raw) = nobro_hal::Twim0::read_reg(0x68, 0x75) {
                unsafe {
                    NOBRO_IMU_HW_EVAL_REPORT.who_am_i = u32::from(raw);
                    NOBRO_IMU_HW_EVAL_REPORT.dev_addr = 0x68;
                }
            }
            defmt::warn!("MPU probe failed; check SDA/SCL wiring");
            idle_fail_loop();
        }
    };

    defmt::info!(
        "IMU addr=0x{:02X} WHO_AM_I=0x{:02X} bmp280={}",
        imu.addr(),
        imu.who_am_i(),
        imu.bmp280_present()
    );

    let board_tag = if Board::APP_FLASH_START == 0x26000 {
        5u32
    } else {
        1u32
    };

    unsafe {
        NOBRO_IMU_HW_EVAL_REPORT.board_id_tag = board_tag;
        NOBRO_IMU_HW_EVAL_REPORT.who_am_i = u32::from(imu.who_am_i());
        NOBRO_IMU_HW_EVAL_REPORT.dev_addr = u32::from(imu.addr());
        NOBRO_IMU_HW_EVAL_REPORT.i2c_devices = u32::from(device_count);
        NOBRO_IMU_HW_EVAL_REPORT.bmp280_present = u32::from(imu.bmp280_present());
    }

    let mut last_report_ms = 0u64;

    loop {
        let now = Hal::now_us();

        match imu.poll() {
            Ok(Some(sample)) => {
                IMU_READS.fetch_add(1, Ordering::AcqRel);
                if let Some(payload) = nobro_kernel::ImuPayload::read_from_handle(sample.handle) {
                    if imu_plausible(payload.accel_g) {
                        LAST_MAG_MG.store(accel_mag_mg(payload.accel_g), Ordering::Release);
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

        if EVAL_DONE.load(Ordering::Acquire) != 0 {
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
    if EVAL_DONE.load(Ordering::Acquire) != 0 {
        return;
    }
    unsafe {
        NOBRO_IMU_HW_EVAL_REPORT.imu_reads = IMU_READS.load(Ordering::Acquire);
        NOBRO_IMU_HW_EVAL_REPORT.imu_errors = IMU_ERRORS.load(Ordering::Acquire);
        NOBRO_IMU_HW_EVAL_REPORT.accel_mag_mg = LAST_MAG_MG.load(Ordering::Acquire);
    }
}

fn try_finalize_eval() {
    if EVAL_DONE.load(Ordering::Acquire) != 0 {
        return;
    }

    let reads = IMU_READS.load(Ordering::Acquire);
    let errors = IMU_ERRORS.load(Ordering::Acquire);
    let mag = LAST_MAG_MG.load(Ordering::Acquire);

    if reads < MIN_IMU_HW_READS || errors * 100 > reads {
        return;
    }

    let mut report = ImuHwEvalReport {
        board_id_tag: unsafe { NOBRO_IMU_HW_EVAL_REPORT.board_id_tag },
        who_am_i: unsafe { NOBRO_IMU_HW_EVAL_REPORT.who_am_i },
        dev_addr: unsafe { NOBRO_IMU_HW_EVAL_REPORT.dev_addr },
        i2c_devices: unsafe { NOBRO_IMU_HW_EVAL_REPORT.i2c_devices },
        bmp280_present: unsafe { NOBRO_IMU_HW_EVAL_REPORT.bmp280_present },
        imu_reads: reads,
        imu_errors: errors,
        accel_mag_mg: mag,
        ..ImuHwEvalReport::zeroed()
    };
    report.seal();

    unsafe {
        NOBRO_IMU_HW_EVAL_REPORT = report;
    }
    EVAL_DONE.store(1, Ordering::Release);

    defmt::info!(
        "NOBRO_IMU_HW_EVAL_FINAL ALL=1 reads={} mag={}mg bmp={}",
        reads,
        mag,
        report.bmp280_present
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
