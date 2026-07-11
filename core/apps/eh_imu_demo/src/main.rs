//! embedded-hal IMU demo: reads an MPU-9250-class IMU using ONLY the
//! `embedded_hal::i2c::I2c` trait (through the nobro-eh-i2c adapter), then writes the
//! standard NOBRO_IMU_HW_EVAL_REPORT. The `imu_*` helpers below take `impl I2c` -
//! exactly the signature an off-the-shelf embedded-hal driver uses - so a passing
//! report proves unmodified embedded-hal drivers run on NobroRTOS.
#![no_std]
#![no_main]

use cortex_m::asm;
use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use embedded_hal::i2c::I2c;
use nobro_eh_i2c::NobroI2c;
use nobro_hal::{
    bus::TwimBus,
    lease::Resource,
    traits::{HalLease, HalTimebaseProvider},
    ActivePlatform as Hal, I2C_SCL_PIN, I2C_SDA_PIN,
};
use nobro_kernel::eval::{
    ImuHwEvalReport, IMU_HW_EVAL_MAGIC, IMU_HW_EVAL_VERSION, MIN_IMU_HW_READS,
};

#[no_mangle]
#[used]
static mut NOBRO_IMU_HW_EVAL_REPORT: ImuHwEvalReport = ImuHwEvalReport::zeroed();

const REG_WHO_AM_I: u8 = 0x75;
const REG_PWR_MGMT_1: u8 = 0x6B;
const REG_ACCEL_XOUT_H: u8 = 0x3B;

// --- pure embedded-hal driver surface (generic over any I2c bus) ---
fn imu_who<I: I2c>(i2c: &mut I, addr: u8) -> Result<u8, I::Error> {
    let mut who = [0u8; 1];
    i2c.write_read(addr, &[REG_WHO_AM_I], &mut who)?;
    Ok(who[0])
}
fn imu_wake<I: I2c>(i2c: &mut I, addr: u8) -> Result<(), I::Error> {
    i2c.write(addr, &[REG_PWR_MGMT_1, 0x01]) // wake + PLL clock
}
fn imu_burst<I: I2c>(i2c: &mut I, addr: u8) -> Result<[u8; 14], I::Error> {
    let mut raw = [0u8; 14]; // accel(6) + temp(2) + gyro(6)
    i2c.write_read(addr, &[REG_ACCEL_XOUT_H], &mut raw)?;
    Ok(raw)
}

fn idle() -> ! {
    loop {
        asm::delay(16_000_000);
    }
}

#[entry]
fn main() -> ! {
    Hal::acquire(Resource::Timer0, 2).unwrap_or_else(|_| defmt::panic!("timer lease"));
    unsafe {
        Hal::init_timebase();
    }
    Hal::acquire(Resource::Twim0, 3).unwrap_or_else(|_| defmt::panic!("I2C lease"));
    TwimBus::init_pins(I2C_SDA_PIN, I2C_SCL_PIN);

    unsafe {
        NOBRO_IMU_HW_EVAL_REPORT.magic = IMU_HW_EVAL_MAGIC;
        NOBRO_IMU_HW_EVAL_REPORT.version = IMU_HW_EVAL_VERSION;
    }

    let mut i2c = NobroI2c::new();

    // Discover the IMU through the embedded-hal trait.
    let mut addr = 0u8;
    let mut who = 0u8;
    for a in [0x68u8, 0x69] {
        if let Ok(w) = imu_who(&mut i2c, a) {
            if matches!(w, 0x68 | 0x70 | 0x71 | 0x73) {
                addr = a;
                who = w;
                break;
            }
        }
    }
    if addr == 0 {
        idle();
    }
    let _ = imu_wake(&mut i2c, addr);
    asm::delay(2_000_000); // let the sensor settle after wake

    let mut reads = 0u32;
    let mut errors = 0u32;
    let mut accel_mg = 0u32;
    let mut gyro_mdps = 0u32;
    let mut temp_centi = 0u32;

    loop {
        match imu_burst(&mut i2c, addr) {
            Ok(raw) => {
                reads += 1;
                let ax = i16::from_be_bytes([raw[0], raw[1]]) as f32 / 16_384.0;
                let ay = i16::from_be_bytes([raw[2], raw[3]]) as f32 / 16_384.0;
                let az = i16::from_be_bytes([raw[4], raw[5]]) as f32 / 16_384.0;
                let traw = i16::from_be_bytes([raw[6], raw[7]]) as f32;
                let gx = i16::from_be_bytes([raw[8], raw[9]]) as f32 / 131.0;
                let gy = i16::from_be_bytes([raw[10], raw[11]]) as f32 / 131.0;
                let gz = i16::from_be_bytes([raw[12], raw[13]]) as f32 / 131.0;
                accel_mg = (libm::sqrtf(ax * ax + ay * ay + az * az) * 1000.0) as u32;
                gyro_mdps = (libm::sqrtf(gx * gx + gy * gy + gz * gz) * 1000.0) as u32;
                let tc = traw / 333.87 + 21.0;
                temp_centi = if tc > 0.0 { (tc * 100.0) as u32 } else { 0 };
            }
            Err(_) => errors += 1,
        }

        unsafe {
            NOBRO_IMU_HW_EVAL_REPORT.board_id_tag = 1;
            NOBRO_IMU_HW_EVAL_REPORT.who_am_i = u32::from(who);
            NOBRO_IMU_HW_EVAL_REPORT.dev_addr = u32::from(addr);
            NOBRO_IMU_HW_EVAL_REPORT.i2c_devices = 1;
            NOBRO_IMU_HW_EVAL_REPORT.imu_reads = reads;
            NOBRO_IMU_HW_EVAL_REPORT.imu_errors = errors;
            NOBRO_IMU_HW_EVAL_REPORT.accel_mag_mg = accel_mg;
            NOBRO_IMU_HW_EVAL_REPORT.gyro_mag_mdps = gyro_mdps;
            NOBRO_IMU_HW_EVAL_REPORT.temp_centi_c = temp_centi;
        }

        if reads >= MIN_IMU_HW_READS && errors * 100 <= reads {
            let mut report = unsafe { NOBRO_IMU_HW_EVAL_REPORT };
            report.seal(); // computes all_pass + checksum
            unsafe {
                NOBRO_IMU_HW_EVAL_REPORT = report;
            }
        }
        asm::delay(400_000);
    }
}
