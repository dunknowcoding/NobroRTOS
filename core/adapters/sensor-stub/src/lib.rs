//! SensorSal stub with synthetic IMU samples when no NiusIMU hardware is connected.
//!
//! Replace with `nobro-adapter-nius-imu` when a real MPU/ICM breakout is wired.

#![no_std]

#[cfg(test)]
extern crate std;

use nobro_hal::{traits::HalClock, ActivePlatform as Hal};
use nobro_kernel::{
    pool::{ImuPayload, SamplePool},
    Capability, CapabilitySet, Criticality, MemoryBudget, ModuleId, ModuleSpec, Sample, SampleKind,
};
use nobro_sal::{AdapterManifest, SensorSal};

const STUB_I2C_ADDR: u8 = 0x68;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorStubError {
    PoolFull,
    InvalidProfile,
    InjectedFault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorStubMode {
    Nominal,
    Silent,
    ErrorEvery(u32),
    BadDataEvery(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SensorStubProfile {
    pub sample_period_ticks: u32,
    pub mode: SensorStubMode,
}

impl SensorStubProfile {
    pub const DEFAULT: Self = Self {
        sample_period_ticks: 50,
        mode: SensorStubMode::Nominal,
    };

    pub const fn new(sample_period_ticks: u32, mode: SensorStubMode) -> Self {
        Self {
            sample_period_ticks,
            mode,
        }
    }
}

/// Software IMU stand-in (BusSal read pattern, no TWIM traffic required).
pub struct SensorStub {
    tick: u32,
    owner: u8,
    profile: SensorStubProfile,
}

impl SensorStub {
    pub fn new(owner: u8) -> Self {
        Self::with_profile(owner, SensorStubProfile::DEFAULT)
    }

    pub fn with_profile(owner: u8, profile: SensorStubProfile) -> Self {
        Self {
            tick: 0,
            owner,
            profile,
        }
    }

    pub fn owner(&self) -> u8 {
        self.owner
    }

    pub fn stub_i2c_addr(&self) -> u8 {
        STUB_I2C_ADDR
    }

    pub fn profile(&self) -> SensorStubProfile {
        self.profile
    }

    pub fn set_mode(&mut self, mode: SensorStubMode) {
        self.profile.mode = mode;
    }

    pub fn tick_count(&self) -> u32 {
        self.tick
    }

    pub fn poll_at(&mut self, now_us: u64) -> Result<Option<Sample>, SensorStubError> {
        self.tick = self.tick.wrapping_add(1);

        if self.profile.sample_period_ticks == 0 {
            return Err(SensorStubError::InvalidProfile);
        }
        if matches!(self.profile.mode, SensorStubMode::Silent) {
            return Ok(None);
        }
        if let SensorStubMode::ErrorEvery(period) = self.profile.mode {
            if period != 0 && self.tick % period == 0 {
                return Err(SensorStubError::InjectedFault);
            }
        }
        if self.tick % self.profile.sample_period_ticks != 0 {
            return Ok(None);
        }

        let sample = SamplePool::alloc(SampleKind::Imu, ImuPayload::LEN, now_us, now_us)
            .ok_or(SensorStubError::PoolFull)?;

        let payload = self.payload_for_tick();
        let _ = ImuPayload::write_to_handle(sample.handle, &payload);
        Ok(Some(sample))
    }

    fn payload_for_tick(&self) -> ImuPayload {
        if let SensorStubMode::BadDataEvery(period) = self.profile.mode {
            if period != 0 && self.tick % period == 0 {
                return ImuPayload {
                    accel_g: [4.0, 0.0, 0.0],
                    gyro_dps: [0.0, 0.0, 0.0],
                };
            }
        }

        let wobble = ((self.tick / self.profile.sample_period_ticks) % 360) as f32 * 0.01;
        ImuPayload {
            accel_g: [wobble, 0.0, 1.0],
            gyro_dps: [0.0, 0.0, wobble * 10.0],
        }
    }
}

impl AdapterManifest for SensorStub {
    fn module_spec() -> ModuleSpec {
        ModuleSpec::new(ModuleId::Sensor, Criticality::BestEffort)
            .requires(
                CapabilitySet::empty()
                    .with(Capability::SamplePool)
                    .with(Capability::Timebase),
            )
            .memory(MemoryBudget::new(4 * 1024, 512, 1))
    }
}

impl SensorSal for SensorStub {
    type Error = SensorStubError;

    fn poll(&mut self) -> Result<Option<Sample>, Self::Error> {
        self.poll_at(Hal::now_us())
    }
}

pub fn module_spec() -> ModuleSpec {
    SensorStub::module_spec()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn release(sample: Option<Sample>) {
        if let Some(sample) = sample {
            SamplePool::release(sample.handle);
        }
    }

    #[test]
    fn default_stub_emits_plausible_sample_on_period() {
        let mut stub = SensorStub::new(7);
        for tick in 1..50 {
            assert!(stub.poll_at(tick).unwrap().is_none());
        }

        let sample = stub.poll_at(50).unwrap().expect("sample");
        assert!(stub_imu_plausible(&sample));
        assert_eq!(sample.captured_us, 50);
        assert_eq!(stub.tick_count(), 50);
        SamplePool::release(sample.handle);
    }

    #[test]
    fn silent_stub_never_emits_samples() {
        let mut stub =
            SensorStub::with_profile(7, SensorStubProfile::new(1, SensorStubMode::Silent));

        for tick in 1..10 {
            assert!(stub.poll_at(tick).unwrap().is_none());
        }
        assert_eq!(stub.tick_count(), 9);
    }

    #[test]
    fn injected_fault_mode_returns_periodic_error() {
        let mut stub =
            SensorStub::with_profile(7, SensorStubProfile::new(1, SensorStubMode::ErrorEvery(3)));

        release(stub.poll_at(1).unwrap());
        release(stub.poll_at(2).unwrap());
        assert!(matches!(
            stub.poll_at(3),
            Err(SensorStubError::InjectedFault)
        ));
        release(stub.poll_at(4).unwrap());
    }

    #[test]
    fn bad_data_mode_emits_implausible_payload_on_period() {
        let mut stub = SensorStub::with_profile(
            7,
            SensorStubProfile::new(1, SensorStubMode::BadDataEvery(2)),
        );

        let sample = stub.poll_at(1).unwrap().expect("first sample");
        assert!(stub_imu_plausible(&sample));
        SamplePool::release(sample.handle);

        let sample = stub.poll_at(2).unwrap().expect("bad sample");
        assert!(!stub_imu_plausible(&sample));
        SamplePool::release(sample.handle);
    }

    #[test]
    fn invalid_profile_is_rejected() {
        let mut stub =
            SensorStub::with_profile(7, SensorStubProfile::new(0, SensorStubMode::Nominal));

        assert!(matches!(
            stub.poll_at(1),
            Err(SensorStubError::InvalidProfile)
        ));
    }
}
