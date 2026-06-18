//! SensorSal stub with synthetic IMU samples when no NiusIMU hardware is connected.
//!
//! Replace with `airon-adapter-nius-imu` when a real MPU/ICM breakout is wired.

#![no_std]

use airon_hal::{traits::HalClock, ActivePlatform as Hal};
use airon_kernel::{
    pool::{ImuPayload, SamplePool},
    Sample, SampleKind,
};
use airon_sal::SensorSal;

const STUB_I2C_ADDR: u8 = 0x68;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorStubError {
    PoolFull,
}

/// Software IMU stand-in (BusSal read pattern, no TWIM traffic required).
pub struct SensorStub {
    tick: u32,
    owner: u8,
}

impl SensorStub {
    pub fn new(owner: u8) -> Self {
        Self { tick: 0, owner }
    }

    pub fn owner(&self) -> u8 {
        self.owner
    }

    pub fn stub_i2c_addr(&self) -> u8 {
        STUB_I2C_ADDR
    }
}

impl SensorSal for SensorStub {
    type Error = SensorStubError;

    fn poll(&mut self) -> Result<Option<Sample>, Self::Error> {
        self.tick = self.tick.wrapping_add(1);
        if self.tick % 50 != 0 {
            return Ok(None);
        }

        let now = Hal::now_us();
        let sample = SamplePool::alloc(SampleKind::Imu, ImuPayload::LEN, now, now)
            .ok_or(SensorStubError::PoolFull)?;

        let wobble = ((self.tick / 50) % 360) as f32 * 0.01;
        let payload = ImuPayload {
            accel_g: [wobble, 0.0, 1.0],
            gyro_dps: [0.0, 0.0, wobble * 10.0],
        };
        let _ = ImuPayload::write_to_handle(sample.handle, &payload);
        Ok(Some(sample))
    }
}

/// Validate that the stub sample magnitude is close to 1 g.
pub fn stub_imu_plausible(sample: &Sample) -> bool {
    if sample.kind != SampleKind::Imu {
        return false;
    }
    let Some(payload) = ImuPayload::read_from_handle(sample.handle) else {
        return false;
    };
    let mag_sq = payload.accel_g[0] * payload.accel_g[0]
        + payload.accel_g[1] * payload.accel_g[1]
        + payload.accel_g[2] * payload.accel_g[2];
    (0.81..1.44).contains(&mag_sq)
}
