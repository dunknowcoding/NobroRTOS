//! ROS bridge on hardware: read the IMU, publish a bounded ROS-style topic message
//! through RosImuBridge (RosBridgeSal), pump the queue to the transport, and record
//! published / transmitted / dropped / peak-depth in NOBRO_ROS_EVAL_REPORT. With one
//! publish + one pump per cycle the bounded ring never overflows (dropped == 0) and
//! everything published is transmitted - the bounded bridge contract, on a real board.
#![no_std]
#![no_main]

use cortex_m::asm;
use defmt_rtt as _;
use panic_halt as _;

use nobro_adapter_mpu9250_imu::{accel_mag_mg, Mpu9250Imu};
use nobro_adapter_ros_imu_bridge::{RosImuBridge, DEPTH, MAX_MSG, TOPIC_IMU};
use nobro_hal::{
    lease::Resource,
    traits::{HalClock, HalLease, HalTimebaseProvider},
    ActivePlatform as Hal,
};
use nobro_kernel::{pool::SamplePool, ImuPayload};
use nobro_sal::{RosBridgeSal, SensorSal};

#[repr(C)]
#[derive(Clone, Copy)]
struct RosEvalReport {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    topic_count: u32,
    published: u32,
    transmitted: u32,
    dropped: u32,
    max_depth: u32,
    buffer_bytes: u32,
    checksum: u32,
}

impl RosEvalReport {
    const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            all_pass: 0,
            topic_count: 0,
            published: 0,
            transmitted: 0,
            dropped: 0,
            max_depth: 0,
            buffer_bytes: 0,
            checksum: 0,
        }
    }
}

const ROS_MAGIC: u32 = 0x4E52_4F53; // "NROS"
const OWNER_TWIM: u8 = 3;
const MIN_PUBLISHED: u32 = 8;

#[no_mangle]
#[used]
static mut NOBRO_ROS_EVAL_REPORT: RosEvalReport = RosEvalReport::zeroed();

fn idle() -> ! {
    loop {
        asm::delay(16_000_000);
    }
}

#[cortex_m_rt::entry]
fn main() -> ! {
    Hal::acquire(Resource::Timer0, 2).ok();
    unsafe {
        Hal::init_timebase();
    }
    Hal::acquire(Resource::Twim0, OWNER_TWIM).ok();
    let mut imu = match Mpu9250Imu::probe_and_init(OWNER_TWIM) {
        Ok(d) => d,
        Err(_) => idle(),
    };

    let mut bridge = RosImuBridge::new();
    let contract = bridge.contract();
    let topic_count = u32::from(contract.topic_count);
    let buffer_bytes = contract.total_buffer_bytes;
    unsafe {
        NOBRO_ROS_EVAL_REPORT.magic = ROS_MAGIC;
        NOBRO_ROS_EVAL_REPORT.version = 1;
        NOBRO_ROS_EVAL_REPORT.topic_count = topic_count;
        NOBRO_ROS_EVAL_REPORT.buffer_bytes = buffer_bytes;
    }

    let mut seq: u32 = 0;

    loop {
        if let Ok(Some(sample)) = imu.poll() {
            if let Some(p) = ImuPayload::read_from_handle(sample.handle) {
                let accel_mg = accel_mag_mg(p.accel_g) as u16;
                let gyro_mdps = imu.last_gyro_mag_mdps() as u16;
                let temp_centi = imu.last_temp_centi_c() as u16;

                // Bounded ROS-style IMU message: seq + accel + gyro + temp (10 bytes).
                let mut msg = [0u8; 10];
                msg[0..4].copy_from_slice(&seq.to_le_bytes());
                msg[4..6].copy_from_slice(&accel_mg.to_le_bytes());
                msg[6..8].copy_from_slice(&gyro_mdps.to_le_bytes());
                msg[8..10].copy_from_slice(&temp_centi.to_le_bytes());
                seq = seq.wrapping_add(1);

                let now = Hal::now_us();
                let _ = bridge.publish(TOPIC_IMU, &msg, now + 5_000);
                bridge.pump(); // hand one queued message to the transport
            }
            SamplePool::release(sample.handle);
        }

        let published = bridge.published();
        let transmitted = bridge.transmitted();
        let dropped = bridge.dropped();
        let max_depth = bridge.max_depth();
        let completed = u32::from(published >= MIN_PUBLISHED);
        let pass = published >= MIN_PUBLISHED
            && dropped == 0
            && transmitted == published
            && max_depth >= 1
            && max_depth <= DEPTH as u32
            && buffer_bytes == (DEPTH * MAX_MSG) as u32;
        let all_pass = u32::from(pass);
        let checksum = ROS_MAGIC
            ^ 1
            ^ completed
            ^ all_pass
            ^ topic_count
            ^ published
            ^ transmitted
            ^ dropped
            ^ max_depth
            ^ buffer_bytes;
        unsafe {
            NOBRO_ROS_EVAL_REPORT.completed = completed;
            NOBRO_ROS_EVAL_REPORT.all_pass = all_pass;
            NOBRO_ROS_EVAL_REPORT.published = published;
            NOBRO_ROS_EVAL_REPORT.transmitted = transmitted;
            NOBRO_ROS_EVAL_REPORT.dropped = dropped;
            NOBRO_ROS_EVAL_REPORT.max_depth = max_depth;
            NOBRO_ROS_EVAL_REPORT.checksum = checksum;
        }

        asm::delay(150_000);
    }
}
