//! Canonical, allocation-free IMU domain contracts for NobroRTOS.
//!
//! Sensor adapters and external-library adapters implement this API. The
//! domain crate deliberately contains no chip registers, bus ownership, or
//! board policy.
//! `ImuHealthReport::diagnostic_checksum` detects accidental snapshot damage;
//! it does not authenticate report bytes.
#![cfg_attr(not(test), no_std)]

pub const IMU_API_VERSION: u16 = 0x0100;
pub const CALIBRATION_MAGIC: u16 = 0x4D49;
pub const IMU_HEALTH_REPORT_MAGIC: u32 = 0x4E42_4933;
pub const IMU_HEALTH_REPORT_VERSION: u32 = 1;
pub const MIN_HEALTH_SAMPLES: u32 = 10;

/// Fixed-layout summary for device applications that expose IMU health to a host.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuHealthReport {
    pub magic: u32,
    pub version: u32,
    pub completed: u32,
    pub healthy: u32,
    pub who_am_i: u32,
    pub device_address: u32,
    pub devices_seen: u32,
    pub companion_present: u32,
    pub samples: u32,
    pub read_errors: u32,
    pub accel_mag_mg: u32,
    pub gyro_mag_mdps: u32,
    pub temperature_centi_c: u32,
    pub diagnostic_checksum: u32,
}

impl ImuHealthReport {
    pub const fn zeroed() -> Self {
        Self {
            magic: 0,
            version: 0,
            completed: 0,
            healthy: 0,
            who_am_i: 0,
            device_address: 0,
            devices_seen: 0,
            companion_present: 0,
            samples: 0,
            read_errors: 0,
            accel_mag_mg: 0,
            gyro_mag_mdps: 0,
            temperature_centi_c: 0,
            diagnostic_checksum: 0,
        }
    }

    pub fn finalize_diagnostic(&mut self) {
        self.magic = IMU_HEALTH_REPORT_MAGIC;
        self.version = IMU_HEALTH_REPORT_VERSION;
        self.healthy = u32::from(
            self.who_am_i != 0
                && matches!(self.device_address, 0x68 | 0x69)
                && self.devices_seen != 0
                && self.samples >= MIN_HEALTH_SAMPLES
                && self.read_errors.saturating_mul(100) <= self.samples
                && (800..=1200).contains(&self.accel_mag_mg)
                && (500..=6000).contains(&self.temperature_centi_c),
        );
        self.completed = 1;
        self.diagnostic_checksum = 0;
        self.diagnostic_checksum = self.compute_diagnostic_checksum();
    }

    pub fn diagnostic_checksum_matches(&self) -> bool {
        self.diagnostic_checksum == self.compute_diagnostic_checksum()
    }

    fn compute_diagnostic_checksum(&self) -> u32 {
        self.magic
            ^ self.version
            ^ self.completed
            ^ self.healthy
            ^ self.who_am_i
            ^ self.device_address
            ^ self.devices_seen
            ^ self.companion_present
            ^ self.samples
            ^ self.read_errors
            ^ self.accel_mag_mg
            ^ self.gyro_mag_mdps
            ^ self.temperature_centi_c
    }
}

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

/// Optional capabilities are orthogonal to the common [`ImuBackend`] sample
/// path. A provider advertises only the traits it actually implements; a
/// sensor without an auxiliary bus or fusion engine pays no storage cost for
/// those features.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuCapabilities(u16);

impl ImuCapabilities {
    pub const FIFO: Self = Self(1 << 0);
    pub const INTERRUPT: Self = Self(1 << 1);
    pub const FUSION: Self = Self(1 << 2);
    pub const AUX_BUS: Self = Self(1 << 3);
    pub const PRESSURE: Self = Self(1 << 4);
    pub const COMPOSITE: Self = Self(1 << 5);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn contains(self, capability: Self) -> bool {
        self.0 & capability.0 == capability.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuFifoStatus {
    pub available: u16,
    pub capacity: u16,
    pub watermark: u16,
    pub overflowed: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuInterruptStatus {
    pub data_ready: bool,
    pub fifo_watermark: bool,
    pub fifo_overflow: bool,
    pub motion: bool,
    pub timestamp_us: u64,
}

/// Unit quaternion in signed Q1.30 representation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuQuaternionQ30 {
    pub w: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub accuracy_millirad: u32,
    pub timestamp_us: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuPressureSample {
    pub pressure_pa: u32,
    pub temperature_centi_c: i32,
    pub altitude_mm: i32,
    pub timestamp_us: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImuCompositeStatus {
    pub present_mask: u16,
    pub healthy_mask: u16,
    pub generation: u32,
}

pub trait ImuCapabilityBackend {
    fn capabilities(&self) -> ImuCapabilities;
}

pub trait ImuFifoBackend: ImuBackend {
    fn fifo_status(&mut self) -> Result<ImuFifoStatus, Self::Error>;
    fn read_fifo(
        &mut self,
        output: &mut [ImuSample],
        deadline_us: u64,
    ) -> Result<usize, Self::Error>;
}

pub trait ImuInterruptBackend: ImuBackend {
    fn take_interrupt(&mut self) -> Result<ImuInterruptStatus, Self::Error>;
}

pub trait ImuFusionBackend: ImuBackend {
    fn fusion_sample(&mut self) -> Result<ImuQuaternionQ30, Self::Error>;
}

pub trait ImuAuxBusBackend: ImuBackend {
    fn aux_read(
        &mut self,
        address: u8,
        register: u8,
        output: &mut [u8],
        deadline_us: u64,
    ) -> Result<usize, Self::Error>;
    fn aux_write(
        &mut self,
        address: u8,
        register: u8,
        payload: &[u8],
        deadline_us: u64,
    ) -> Result<usize, Self::Error>;
}

pub trait ImuPressureBackend: ImuBackend {
    fn pressure_sample(&mut self) -> Result<ImuPressureSample, Self::Error>;
}

pub trait ImuCompositeBackend: ImuBackend {
    fn composite_status(&mut self) -> Result<ImuCompositeStatus, Self::Error>;
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
        let capabilities = ImuCapabilities::FIFO
            .union(ImuCapabilities::INTERRUPT)
            .union(ImuCapabilities::AUX_BUS);
        assert!(capabilities.contains(ImuCapabilities::FIFO));
        assert!(capabilities.contains(ImuCapabilities::AUX_BUS));
        assert!(!capabilities.contains(ImuCapabilities::FUSION));
    }

    #[test]
    fn health_report_is_fixed_layout_and_self_checking() {
        let mut report = ImuHealthReport {
            who_am_i: 0x71,
            device_address: 0x68,
            devices_seen: 1,
            samples: 10,
            accel_mag_mg: 1000,
            temperature_centi_c: 2500,
            ..ImuHealthReport::zeroed()
        };
        report.finalize_diagnostic();
        assert_eq!(report.healthy, 1);
        assert!(report.diagnostic_checksum_matches());
    }
}
