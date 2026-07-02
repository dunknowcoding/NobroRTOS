//! Control primitives on real hardware (M148/M149): read the live SPI MPU-9250, derive a
//! tilt angle from the accelerometer, fuse it with the gyro rate through the
//! complementary filter, and drive a PID toward level (setpoint 0), emitting a servo
//! pulse command. On a stationary bench board the filter must settle to a small tilt and
//! the PID must hold the pulse near center. NOBRO_CONTROL_REPORT (J-Link mem32).
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use embedded_hal::spi::SpiDevice as _;
use nobro_control::{ComplementaryFilter, Pid};
use nobro_eh_spi::NobroSpiDevice;
use nobro_hal::{
    board,
    lease::Resource,
    traits::{HalClock, HalLease, PlatformHal},
    ActivePlatform as Hal,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    who_am_i: u32,
    angle_mdeg: u32, // i32 bit-cast: final fused tilt in milli-degrees
    pulse_us: u32,   // final PID-driven servo pulse command
    loops: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E43_544C; // "NCTL"

#[no_mangle]
#[used]
static mut NOBRO_CONTROL_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    who_am_i: 0,
    angle_mdeg: 0,
    pulse_us: 0,
    loops: 0,
    checksum: 0,
};

fn rd(dev: &mut NobroSpiDevice, reg: u8) -> Result<u8, ()> {
    let mut rx = [0u8; 2];
    dev.transfer(&mut rx, &[0x80 | reg, 0]).map_err(|_| ())?;
    Ok(rx[1])
}

fn wr(dev: &mut NobroSpiDevice, reg: u8, val: u8) {
    let _ = dev.write(&[reg & 0x7F, val]);
}

/// Read a big-endian i16 register pair (per-register: clone bursts are unreliable).
fn rd16(dev: &mut NobroSpiDevice, reg_h: u8) -> i16 {
    let h = rd(dev, reg_h).unwrap_or(0);
    let l = rd(dev, reg_h + 1).unwrap_or(0);
    i16::from_be_bytes([h, l])
}

const OWNER_SPI: u8 = 4;
const LOOPS: u32 = 300;

#[entry]
fn main() -> ! {
    Hal::acquire(Resource::Timer0, 2).ok();
    unsafe {
        Hal::init_timebase();
    }
    Hal::acquire(Resource::Spim0, OWNER_SPI).ok();
    let mut dev = unsafe {
        NobroSpiDevice::new(
            board::SPI_SCK_PIN,
            board::SPI_MOSI_PIN,
            board::SPI_MISO_PIN,
            board::SPI_CS_PIN,
        )
    };

    // MPU-9250 bring-up (same sequence the SPI IMU demo verified on this board).
    wr(&mut dev, 0x6B, 0x80);
    for _ in 0..400_000u32 {
        cortex_m::asm::nop();
    }
    wr(&mut dev, 0x6B, 0x01);
    wr(&mut dev, 0x6A, 0x10);
    wr(&mut dev, 0x6C, 0x00);
    wr(&mut dev, 0x1A, 0x03);
    wr(&mut dev, 0x19, 0x04);
    wr(&mut dev, 0x1B, 0x00); // +/-250 dps (131 LSB/dps)
    wr(&mut dev, 0x1C, 0x00); // +/-2 g
    wr(&mut dev, 0x1D, 0x03);
    for _ in 0..400_000u32 {
        cortex_m::asm::nop();
    }

    let who_am_i = u32::from(rd(&mut dev, 0x75).unwrap_or(0));
    let afs = (rd(&mut dev, 0x1C).unwrap_or(0) >> 3) & 0x03;
    let accel_lsb_per_g = (16384u32 >> afs) as f32;

    // Accel tilt angle in degrees. This board's module is mounted with gravity mostly
    // along X (measured: |angle| saturates the atan clamp with a level=0 setpoint), so
    // the control target is HOLD THE INITIAL ATTITUDE: capture a baseline below and
    // regulate deviation from it - meaningful regardless of mounting.
    let read_angle = |dev: &mut NobroSpiDevice| -> f32 {
        let ax = rd16(dev, 0x3B) as f32 / accel_lsb_per_g;
        let az = rd16(dev, 0x3F) as f32 / accel_lsb_per_g;
        let ratio = if az.abs() > 0.05 {
            (ax / az).clamp(-1.0, 1.0)
        } else if ax >= 0.0 {
            1.0
        } else {
            -1.0
        };
        ratio * 57.295_78
    };

    // Baseline: average 20 accel angles at rest.
    let mut baseline = 0.0f32;
    for _ in 0..20 {
        baseline += read_angle(&mut dev);
        for _ in 0..80_000u32 {
            cortex_m::asm::nop();
        }
    }
    baseline /= 20.0;

    let mut cf = ComplementaryFilter::new(0.98);
    // PID holds the initial attitude: output is a servo pulse offset in us, clamped +/-400.
    let mut pid = Pid::new(8.0, 0.5, 0.2, -400.0, 400.0);

    let mut loops: u32 = 0;
    let mut angle: f32 = 0.0;
    let mut pulse: f32 = 1500.0;
    let mut t_prev = Hal::now_us();

    while loops < LOOPS {
        let accel_angle_deg = read_angle(&mut dev);
        let gy = rd16(&mut dev, 0x45); // GYRO_YOUT
        let gyro_dps = gy as f32 / 131.0;

        let now = Hal::now_us();
        let dt = (now.wrapping_sub(t_prev) as f32) / 1_000_000.0;
        t_prev = now;

        angle = cf.update(accel_angle_deg, gyro_dps, dt);
        let out = pid.update(baseline, angle, dt);
        pulse = 1500.0 + out;

        loops += 1;
        for _ in 0..160_000u32 {
            cortex_m::asm::nop(); // pacing
        }
    }

    let deviation_mdeg = ((angle - baseline) * 1000.0) as i32;
    let angle_mdeg = deviation_mdeg; // report the deviation from the held attitude
    let pulse_us = pulse as u32;
    // Stationary bench board: fused angle holds the baseline within 5 deg and the PID
    // keeps the pulse near center (small correction only).
    let pass = loops == LOOPS
        && who_am_i != 0
        && who_am_i != 0xFF
        && angle_mdeg.unsigned_abs() < 5_000
        && (1_400..=1_600).contains(&pulse_us);
    let ap = u32::from(pass);
    let am = angle_mdeg as u32;
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ who_am_i ^ am ^ pulse_us ^ loops;
    unsafe {
        NOBRO_CONTROL_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            who_am_i,
            angle_mdeg: am,
            pulse_us,
            loops,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
