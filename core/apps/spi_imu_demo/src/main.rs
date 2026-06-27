//! SPI IMU bring-up on board1: read the GY-9250 (MPU-9250) over the SPIM HAL driver and
//! self-certify via NOBRO_SPI_IMU_REPORT (read over J-Link `mem32`). board1's sensor is
//! wired for SPI (SCK=P0.17, MISO=P0.20, MOSI=P0.22, CS=P0.24); this proves the SPIM
//! driver + the SPI signal path on real hardware, alongside the I2C path on board5.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_hal::{
    board,
    lease::Resource,
    traits::{HalLease, PlatformHal},
    ActivePlatform as Hal, Spim0,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct SpiImuReport {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    who_am_i: u32,
    accel_mag_mg: u32,
    reads: u32,
    errors: u32,
    raw_ax: u32,
    raw_ay: u32,
    raw_az: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E42_5350; // "NBSP" (NoBro SPi)

#[no_mangle]
#[used]
static mut NOBRO_SPI_IMU_REPORT: SpiImuReport = SpiImuReport {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    who_am_i: 0,
    accel_mag_mg: 0,
    reads: 0,
    errors: 0,
    raw_ax: 0,
    raw_ay: 0,
    raw_az: 0,
    checksum: 0,
};

/// Integer square root (Newton's method) - no libm, no float, for the accel magnitude.
fn isqrt(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

const OWNER_SPI: u8 = 4;

#[entry]
fn main() -> ! {
    Hal::acquire(Resource::Timer0, 2).ok();
    unsafe {
        Hal::init_timebase();
    }
    Hal::acquire(Resource::Spim0, OWNER_SPI).ok();
    let spi = unsafe {
        Spim0::init(
            board::SPI_SCK_PIN,
            board::SPI_MOSI_PIN,
            board::SPI_MISO_PIN,
            board::SPI_CS_PIN,
        )
    };

    // MPU-9250 SPI bring-up: reset, wake, force SPI-only (disable the aux I2C slave),
    // enable accel + gyro. Default accel FS is +/-2 g (16384 LSB/g).
    let _ = spi.write_reg(0x6B, 0x80); // PWR_MGMT_1: device reset
    for _ in 0..400_000u32 {
        cortex_m::asm::nop();
    }
    let _ = spi.write_reg(0x6B, 0x01); // PWR_MGMT_1: wake, auto clock
    let _ = spi.write_reg(0x6A, 0x10); // USER_CTRL: I2C_IF_DIS (SPI only)
    let _ = spi.write_reg(0x6C, 0x00); // PWR_MGMT_2: accel + gyro on
    let _ = spi.write_reg(0x1A, 0x03); // CONFIG: gyro DLPF 41 Hz
    let _ = spi.write_reg(0x19, 0x04); // SMPLRT_DIV: 200 Hz
    let _ = spi.write_reg(0x1B, 0x00); // GYRO_CONFIG: +/-250 dps
    let _ = spi.write_reg(0x1C, 0x00); // ACCEL_CONFIG: +/-2 g (16384 LSB/g)
    let _ = spi.write_reg(0x1D, 0x03); // ACCEL_CONFIG2: accel DLPF 41 Hz
    for _ in 0..400_000u32 {
        cortex_m::asm::nop();
    }

    let who_am_i = u32::from(spi.read_reg(0x75).unwrap_or(0));

    // Determine the accel full-scale actually in effect (a clone may ignore our +/-2 g
    // write). ACCEL_CONFIG.AFS_SEL (bits 4:3): 0=+/-2g (16384), 1=4g (8192), 2=8g
    // (4096), 3=16g (2048 LSB/g).
    let afs = (spi.read_reg(0x1C).unwrap_or(0) >> 3) & 0x03;
    let accel_divisor: u64 = u64::from(16384u32 >> afs);

    let mut reads: u32 = 0;
    let mut errors: u32 = 0;
    let mut accel_mag_mg: u32 = 0;
    let mut raw_ax: u32 = 0;
    let mut raw_ay: u32 = 0;
    let mut raw_az: u32 = 0;
    let mut spin: u32 = 0;
    loop {
        spin = spin.wrapping_add(1);
        if spin % 200_000 == 0 {
            // Per-register reads (not a burst): the clone's auto-increment burst
            // returned zeros for X/Z, so read each ACCEL_*OUT register individually.
            let rd = |r: u8| spi.read_reg(r);
            match (
                rd(0x3B),
                rd(0x3C),
                rd(0x3D),
                rd(0x3E),
                rd(0x3F),
                rd(0x40),
            ) {
                (Ok(xh), Ok(xl), Ok(yh), Ok(yl), Ok(zh), Ok(zl)) => {
                    reads += 1;
                    let ix = i16::from_be_bytes([xh, xl]);
                    let iy = i16::from_be_bytes([yh, yl]);
                    let iz = i16::from_be_bytes([zh, zl]);
                    raw_ax = (ix as i32) as u32;
                    raw_ay = (iy as i32) as u32;
                    raw_az = (iz as i32) as u32;
                    let ax = i64::from(ix);
                    let ay = i64::from(iy);
                    let az = i64::from(iz);
                    let sq = (ax * ax + ay * ay + az * az) as u64;
                    // |a| in LSB -> milli-g at the detected full-scale.
                    accel_mag_mg = (isqrt(sq) * 1000 / accel_divisor) as u32;
                }
                _ => errors += 1,
            }

            // who_am_i: 0x71 MPU-9250 / 0x70 MPU-6500 / 0x73 MPU-9255 - accept any of
            // these (clone GY-9250 boards vary); the accel magnitude is the real proof.
            let who_ok = matches!(who_am_i, 0x70 | 0x71 | 0x73 | 0x68);
            let pass = who_ok && reads >= 10 && (800..1200).contains(&accel_mag_mg);
            let ap = u32::from(pass);
            let cs = MAGIC ^ 1 ^ 1 ^ ap ^ who_am_i ^ accel_mag_mg ^ reads ^ errors;
            unsafe {
                NOBRO_SPI_IMU_REPORT = SpiImuReport {
                    magic: MAGIC,
                    version: 1,
                    completed: 1,
                    all_pass: ap,
                    who_am_i,
                    accel_mag_mg,
                    reads,
                    errors,
                    raw_ax,
                    raw_ay,
                    raw_az,
                    checksum: cs,
                };
            }
        }
    }
}
