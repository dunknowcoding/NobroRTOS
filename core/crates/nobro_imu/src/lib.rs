//! Canonical, allocation-free IMU domain contracts for NobroRTOS.
//!
//! Sensor adapters and external-library adapters implement this API. The
//! domain crate deliberately contains no chip registers, bus ownership, or
//! board policy.
#![cfg_attr(not(test), no_std)]

pub const IMU_API_VERSION: u16 = 0x0100;
pub const CALIBRATION_MAGIC: u16 = 0x4D49;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuSample {
    pub accel_mg: [i32; 3],
    pub accel_mag_mg: u32,
    pub gyro_mdps: [i32; 3],
    pub mag_milli_ut: [i32; 3],
    pub temperature_centi_c: i32,
    pub timestamp_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImuCalibration {
    pub accel_bias_mg: [i32; 3],
    pub accel_scale_ppm: [i32; 3],
    pub gyro_bias_mdps: [i32; 3],
    pub mag_bias_milli_ut: [i32; 3],
    pub mag_scale_ppm: [i32; 3],
    pub magic: u16,
}

impl Default for ImuCalibration {
    fn default() -> Self {
        Self {
            accel_bias_mg: [0; 3],
            accel_scale_ppm: [1_000_000; 3],
            gyro_bias_mdps: [0; 3],
            mag_bias_milli_ut: [0; 3],
            mag_scale_ppm: [1_000_000; 3],
            magic: CALIBRATION_MAGIC,
        }
    }
}

impl ImuCalibration {
    pub const fn valid(&self) -> bool {
        self.magic == CALIBRATION_MAGIC
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuFamily(pub u16);

impl ImuFamily {
    pub const UNKNOWN: Self = Self(0);
    pub const MPU6050: Self = Self(0x6050);
    pub const MPU6500: Self = Self(0x6500);
    pub const MPU9250: Self = Self(0x9250);
    pub const MPU9255: Self = Self(0x9255);
    pub const ICM45686: Self = Self(0x4568);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuIdentity {
    pub family: ImuFamily,
    pub who_am_i: u8,
    pub address: u8,
    pub has_magnetometer: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum ImuEvent {
    #[default]
    None = 0,
    Sample = 1,
    ReadError = 2,
    Recovered = 3,
    RecoveryExhausted = 4,
    CalibrationRejected = 5,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuDiagnostics {
    pub samples: u32,
    pub read_errors: u32,
    pub recoveries: u32,
    pub consecutive_errors: u16,
    pub recovery_attempts: u8,
    pub last_event: ImuEvent,
}

pub trait ImuBackend {
    type Error;

    fn identity(&mut self) -> Result<ImuIdentity, Self::Error>;
    fn sample(&mut self) -> Result<ImuSample, Self::Error>;
    fn recover(&mut self) -> Result<(), Self::Error>;
    fn diagnostics(&self) -> ImuDiagnostics;

    fn calibration(&self) -> Option<ImuCalibration> {
        None
    }

    fn set_calibration(&mut self, calibration: ImuCalibration) -> bool {
        let _ = calibration;
        false
    }
}

pub fn magnitude3(values: [i32; 3]) -> u32 {
    let sum = values.into_iter().fold(0u64, |acc, value| {
        let magnitude = i64::from(value).unsigned_abs();
        acc.saturating_add(magnitude.saturating_mul(magnitude))
    });
    integer_sqrt(sum).min(u64::from(u32::MAX)) as u32
}

fn integer_sqrt(value: u64) -> u64 {
    if value < 2 {
        return value;
    }
    let mut x = value;
    let mut next = (x + value / x) / 2;
    while next < x {
        x = next;
        next = (x + value / x) / 2;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_units_and_calibration_are_stable() {
        assert_eq!(IMU_API_VERSION, 0x0100);
        assert!(ImuCalibration::default().valid());
        assert_eq!(magnitude3([300, 400, 0]), 500);
        assert_ne!(ImuFamily::MPU6050, ImuFamily::MPU9250);
    }
}
