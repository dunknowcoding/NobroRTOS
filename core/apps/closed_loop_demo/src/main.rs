//! Closed-loop Robot demo on hardware: IMU -> compute -> servo -> verify.
//!
//! Each cycle reads the IMU accel magnitude, maps it to a 50 Hz servo pulse, drives
//! the servo through the RoboServo ActuatorSal, and reads the PWM pulse back to
//! confirm the actuator reproduced the sensor-derived command within tolerance.
//! Records steps + readback matches in NOBRO_CLOSEDLOOP_REPORT.
#![no_std]
#![no_main]

use cortex_m::asm;
use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_adapter_mpu9250_imu::{accel_mag_mg, Mpu9250Imu};
use nobro_adapter_robo_servo::RoboServoAdapter;
use nobro_hal::{
    lease::Resource,
    traits::{HalClock, HalLease, HalServoPwm, PlatformHal},
    ActivePlatform as Hal,
};
use nobro_kernel::{pool::SamplePool, ImuPayload};
use nobro_sal::{ActuatorSal, SensorSal};

#[repr(C)]
#[derive(Clone, Copy)]
struct ClosedLoopReport {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    steps: u32,
    readback_ok: u32,
    last_cmd_us: u32,
    last_readback_us: u32,
    accel_mg: u32,
    checksum: u32,
}
const CL_MAGIC: u32 = 0x4E42_434C; // "NBCL"
const OWNER_TWIM: u8 = 3;
const TOL_US: u32 = 50;
const MIN_STEPS: u32 = 20;

#[no_mangle]
#[used]
static mut NOBRO_CLOSEDLOOP_REPORT: ClosedLoopReport = ClosedLoopReport {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    steps: 0,
    readback_ok: 0,
    last_cmd_us: 0,
    last_readback_us: 0,
    accel_mg: 0,
    checksum: 0,
};

#[entry]
fn main() -> ! {
    Hal::acquire(Resource::Timer0, 1).ok();
    unsafe {
        Hal::init_timebase();
    }
    Hal::acquire(Resource::Twim0, OWNER_TWIM).ok();
    let mut imu = Mpu9250Imu::probe_and_init(OWNER_TWIM).ok();

    let profile = Hal::servo_profile();
    let mut servo = RoboServoAdapter::new(profile.pin);
    unsafe {
        let _ = servo.attach_50hz(profile.center_pulse_us);
    }

    unsafe {
        NOBRO_CLOSEDLOOP_REPORT.magic = CL_MAGIC;
        NOBRO_CLOSEDLOOP_REPORT.version = 1;
    }

    let mut steps = 0u32;
    let mut readback_ok = 0u32;
    let mut accel_mg = 0u32;

    loop {
        if let Some(d) = imu.as_mut() {
            if let Ok(Some(sample)) = d.poll() {
                if let Some(p) = ImuPayload::read_from_handle(sample.handle) {
                    accel_mg = accel_mag_mg(p.accel_g);
                }
                SamplePool::release(sample.handle);
            }
        }

        // Map accel magnitude (mg) to a servo pulse (us), centered at 1 g -> 1500 us,
        // clamped to the standard 1000..2000 us range. This closes the loop.
        let cmd_us = (1500i32 + (accel_mg as i32 - 1000) * 2).clamp(1000, 2000) as u32;
        let deadline_us = Hal::now_us() + 5_000;

        if servo.set_duty_us(0, cmd_us, deadline_us).is_ok() {
            let readback = <Hal as HalServoPwm>::read_pulse_us();
            if cmd_us.abs_diff(readback) <= TOL_US {
                readback_ok += 1;
            }
            steps += 1;

            let pass = steps >= MIN_STEPS
                && readback_ok + 1 >= steps // allow one transient miss
                && (1000..=2000).contains(&cmd_us)
                && (800..1200).contains(&accel_mg);
            let completed = u32::from(steps >= MIN_STEPS);
            let all_pass = u32::from(pass);
            let cs = CL_MAGIC
                ^ 1
                ^ completed
                ^ all_pass
                ^ steps
                ^ readback_ok
                ^ cmd_us
                ^ readback
                ^ accel_mg;
            unsafe {
                NOBRO_CLOSEDLOOP_REPORT.completed = completed;
                NOBRO_CLOSEDLOOP_REPORT.all_pass = all_pass;
                NOBRO_CLOSEDLOOP_REPORT.steps = steps;
                NOBRO_CLOSEDLOOP_REPORT.readback_ok = readback_ok;
                NOBRO_CLOSEDLOOP_REPORT.last_cmd_us = cmd_us;
                NOBRO_CLOSEDLOOP_REPORT.last_readback_us = readback;
                NOBRO_CLOSEDLOOP_REPORT.accel_mg = accel_mg;
                NOBRO_CLOSEDLOOP_REPORT.checksum = cs;
            }
        }

        asm::delay(150_000);
    }
}
