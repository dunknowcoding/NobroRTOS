//! MPU9250-family adapter over real TWIM.

#![no_std]

use nobro_hal::{bus::TwimBus, traits::HalClock, ActivePlatform as Hal, I2C_SCL_PIN, I2C_SDA_PIN};
use nobro_imu::{
    magnitude3, ImuBackend, ImuDiagnostics, ImuEvent, ImuFamily, ImuIdentity, ImuSample,
};
use nobro_kernel::{
    pool::{CompactImuPayload, SamplePool},
    Capability, CapabilitySet, Criticality, MemoryBudget, ModuleId, ModuleSpec, Sample, SampleKind,
};
use nobro_sal::{AdapterManifest, SensorSal};

const REG_WHO_AM_I: u8 = 0x75;
const REG_PWR_MGMT_1: u8 = 0x6B;
const REG_ACCEL_XOUT_H: u8 = 0x3B;
const REG_BMP280_ID: u8 = 0xD0;
const BMP280_ADDR: u8 = 0x76;

const WHO_MPU6050: u8 = 0x68;
const WHO_MPU6500: u8 = 0x70;
const WHO_MPU9250: u8 = 0x71;
const WHO_MPU9255: u8 = 0x73;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mpu9250Error {
    NotFound,
    WhoAmMismatch,
    Bus,
    PoolFull,
    NotReady,
}

pub struct Mpu9250Imu {
    bus: TwimBus,
    addr: u8,
    who_am_i: u8,
    owner: u8,
    ready: bool,
    bmp280_present: bool,
    last_temp_centi: u32,
    last_gyro_mdps: u32,
    diagnostics: ImuDiagnostics,
}

impl Mpu9250Imu {
    pub fn probe_and_init(owner: u8) -> Result<Self, Mpu9250Error> {
        let bus = TwimBus::new_twim0(owner).map_err(|_| Mpu9250Error::Bus)?;
        bus.init_pins(I2C_SDA_PIN, I2C_SCL_PIN)
            .map_err(|_| Mpu9250Error::Bus)?;

        let mut found = None;
        for addr in [0x68u8, 0x69] {
            if let Ok(id) = bus.read_reg(addr, REG_WHO_AM_I) {
                if matches!(id, WHO_MPU6050 | WHO_MPU6500 | WHO_MPU9250 | WHO_MPU9255) {
                    found = Some((addr, id));
                    break;
                }
            }
        }
        let (addr, who_am_i) = found.ok_or(Mpu9250Error::NotFound)?;

        bus.write_reg(addr, REG_PWR_MGMT_1, 0x01)
            .map_err(|_| Mpu9250Error::Bus)?;
        spin_wait(500_000);
        bus.write_reg(addr, 0x1A, 0x03)
            .map_err(|_| Mpu9250Error::Bus)?;
        bus.write_reg(addr, 0x1B, 0x00)
            .map_err(|_| Mpu9250Error::Bus)?;
        bus.write_reg(addr, 0x1C, 0x00)
            .map_err(|_| Mpu9250Error::Bus)?;

        let bmp280_present = bus
            .read_reg(BMP280_ADDR, REG_BMP280_ID)
            .map(|id| id == 0x58)
            .unwrap_or(false);

        Ok(Self {
            bus,
            addr,
            who_am_i,
            owner,
            ready: true,
            bmp280_present,
            last_temp_centi: 0,
            last_gyro_mdps: 0,
            diagnostics: ImuDiagnostics::default(),
        })
    }

    /// Die temperature from the most recent burst, in centi-degrees C.
    pub fn last_temp_centi_c(&self) -> u32 {
        self.last_temp_centi
    }

    /// Gyro magnitude from the most recent burst, in milli-deg/s.
    pub fn last_gyro_mag_mdps(&self) -> u32 {
        self.last_gyro_mdps
    }

    pub fn addr(&self) -> u8 {
        self.addr
    }

    pub fn who_am_i(&self) -> u8 {
        self.who_am_i
    }

    pub fn bmp280_present(&self) -> bool {
        self.bmp280_present
    }

    pub fn owner(&self) -> u8 {
        self.owner
    }

    pub fn scan_device_count(owner: u8) -> Result<u8, Mpu9250Error> {
        let bus = TwimBus::new_twim0(owner).map_err(|_| Mpu9250Error::Bus)?;
        bus.init_pins(I2C_SDA_PIN, I2C_SCL_PIN)
            .map_err(|_| Mpu9250Error::Bus)?;
        bus.scan(|_| {}).map_err(|_| Mpu9250Error::Bus)
    }

    fn read_burst(&mut self) -> Result<([f32; 3], [f32; 3]), Mpu9250Error> {
        if !self.ready {
            return Err(Mpu9250Error::NotReady);
        }
        // One burst from ACCEL_XOUT_H covers accel (6), temperature (2), gyro (6):
        // the MPU register map is contiguous, so a 14-byte read gets all three.
        let mut raw = [0u8; 14];
        self.bus
            .write_read(self.addr, &[REG_ACCEL_XOUT_H], &mut raw)
            .map_err(|_| Mpu9250Error::Bus)?;

        let ax = i16::from_be_bytes([raw[0], raw[1]]);
        let ay = i16::from_be_bytes([raw[2], raw[3]]);
        let az = i16::from_be_bytes([raw[4], raw[5]]);
        let temp_raw = i16::from_be_bytes([raw[6], raw[7]]);
        let gx = i16::from_be_bytes([raw[8], raw[9]]);
        let gy = i16::from_be_bytes([raw[10], raw[11]]);
        let gz = i16::from_be_bytes([raw[12], raw[13]]);

        // +/-2 g and +/-250 dps factory defaults.
        let accel_g = [
            ax as f32 / 16_384.0,
            ay as f32 / 16_384.0,
            az as f32 / 16_384.0,
        ];
        let gyro_dps = [gx as f32 / 131.0, gy as f32 / 131.0, gz as f32 / 131.0];

        // MPU-9250 die temperature: degC = raw / 333.87 + 21.0.
        let temp_c = temp_raw as f32 / 333.87 + 21.0;
        self.last_temp_centi = if temp_c > 0.0 {
            (temp_c * 100.0) as u32
        } else {
            0
        };
        let gmag = libm::sqrtf(
            gyro_dps[0] * gyro_dps[0] + gyro_dps[1] * gyro_dps[1] + gyro_dps[2] * gyro_dps[2],
        );
        self.last_gyro_mdps = (gmag * 1000.0) as u32;

        Ok((accel_g, gyro_dps))
    }
}

impl ImuBackend for Mpu9250Imu {
    type Error = Mpu9250Error;

    fn identity(&mut self) -> Result<ImuIdentity, Self::Error> {
        let family = match self.who_am_i {
            WHO_MPU6050 => ImuFamily::MPU6050,
            WHO_MPU6500 => ImuFamily::MPU6500,
            WHO_MPU9250 => ImuFamily::MPU9250,
            WHO_MPU9255 => ImuFamily::MPU9255,
            _ => ImuFamily::UNKNOWN,
        };
        Ok(ImuIdentity {
            family,
            who_am_i: self.who_am_i,
            address: self.addr,
            // The chip may contain an AK8963, but this native backend does not yet
            // acquire it. Capability reports describe delivered data, not silicon.
            has_magnetometer: false,
        })
    }

    fn sample(&mut self) -> Result<ImuSample, Self::Error> {
        let (accel_g, gyro_dps) = match self.read_burst() {
            Ok(sample) => sample,
            Err(error) => {
                self.diagnostics.read_errors = self.diagnostics.read_errors.saturating_add(1);
                self.diagnostics.consecutive_errors =
                    self.diagnostics.consecutive_errors.saturating_add(1);
                self.diagnostics.last_event = ImuEvent::ReadError;
                return Err(error);
            }
        };
        let accel_mg = accel_g.map(|value| (value * 1000.0) as i32);
        let gyro_mdps = gyro_dps.map(|value| (value * 1000.0) as i32);
        self.diagnostics.samples = self.diagnostics.samples.saturating_add(1);
        self.diagnostics.consecutive_errors = 0;
        self.diagnostics.last_event = ImuEvent::Sample;
        Ok(ImuSample {
            accel_mg,
            accel_mag_mg: magnitude3(accel_mg),
            gyro_mdps,
            temperature_centi_c: self.last_temp_centi as i32,
            timestamp_us: Hal::now_us(),
            ..ImuSample::default()
        })
    }

    fn recover(&mut self) -> Result<(), Self::Error> {
        self.diagnostics.recovery_attempts = self.diagnostics.recovery_attempts.saturating_add(1);
        match Self::probe_and_init(self.owner) {
            Ok(mut replacement) => {
                replacement.diagnostics = self.diagnostics;
                replacement.diagnostics.recoveries =
                    replacement.diagnostics.recoveries.saturating_add(1);
                replacement.diagnostics.consecutive_errors = 0;
                replacement.diagnostics.last_event = ImuEvent::Recovered;
                *self = replacement;
                Ok(())
            }
            Err(error) => {
                self.diagnostics.last_event = ImuEvent::RecoveryExhausted;
                Err(error)
            }
        }
    }

    fn diagnostics(&self) -> ImuDiagnostics {
        self.diagnostics
    }
}

impl AdapterManifest for Mpu9250Imu {
    fn module_spec() -> ModuleSpec {
        ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
            .requires(
                CapabilitySet::empty()
                    .with(Capability::Bus0)
                    .with(Capability::SamplePool)
                    .with(Capability::Timebase),
            )
            .owns(CapabilitySet::empty().with(Capability::Bus0))
            .memory(MemoryBudget::new(30 * 1024, 2 * 1024, 2))
    }
}

impl SensorSal for Mpu9250Imu {
    type Error = Mpu9250Error;

    fn poll(&mut self) -> Result<Option<Sample>, Self::Error> {
        let domain = ImuBackend::sample(self)?;
        let now = domain.timestamp_us;
        let payload = CompactImuPayload::from_sample(domain);
        let sample = SamplePool::alloc(SampleKind::Imu, CompactImuPayload::LEN, now, now)
            .ok_or(Mpu9250Error::PoolFull)?;
        let _ = CompactImuPayload::write_to_handle(sample.handle, &payload);
        Ok(Some(sample))
    }
}

pub fn module_spec() -> ModuleSpec {
    Mpu9250Imu::module_spec()
}

pub fn accel_mag_mg(accel_g: [f32; 3]) -> u32 {
    let mag_sq = accel_g[0] * accel_g[0] + accel_g[1] * accel_g[1] + accel_g[2] * accel_g[2];
    (libm::sqrtf(mag_sq) * 1000.0) as u32
}

pub fn imu_plausible(accel_g: [f32; 3]) -> bool {
    let mag_sq = accel_g[0] * accel_g[0] + accel_g[1] * accel_g[1] + accel_g[2] * accel_g[2];
    (0.64..1.69).contains(&mag_sq)
}

fn spin_wait(iterations: u32) {
    for _ in 0..iterations {
        cortex_m::asm::nop();
    }
}
