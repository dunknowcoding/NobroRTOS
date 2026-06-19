//! MPU9250 / MPU6500 / GY-9250 / GY-91 IMU over the real TWIM SensorSal adapter.

#![no_std]

use airon_hal::{
    bus::TwimBus, traits::HalClock, twim_hw::Twim0, ActivePlatform as Hal, I2C_SCL_PIN, I2C_SDA_PIN,
};
use airon_kernel::{
    pool::{ImuPayload, SamplePool},
    Capability, CapabilitySet, Criticality, MemoryBudget, ModuleId, ModuleSpec, Sample, SampleKind,
};
use airon_sal::{AdapterManifest, SensorSal};

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
    addr: u8,
    who_am_i: u8,
    owner: u8,
    ready: bool,
    bmp280_present: bool,
}

impl Mpu9250Imu {
    pub fn probe_and_init(owner: u8) -> Result<Self, Mpu9250Error> {
        TwimBus::init_pins(I2C_SDA_PIN, I2C_SCL_PIN);

        let mut found = None;
        for addr in [0x68u8, 0x69] {
            if let Ok(id) = Twim0::read_reg(addr, REG_WHO_AM_I) {
                if matches!(id, WHO_MPU6050 | WHO_MPU6500 | WHO_MPU9250 | WHO_MPU9255) {
                    found = Some((addr, id));
                    break;
                }
            }
        }
        let (addr, who_am_i) = found.ok_or(Mpu9250Error::NotFound)?;

        Twim0::write_reg(addr, REG_PWR_MGMT_1, 0x01).map_err(|_| Mpu9250Error::Bus)?;
        spin_wait(500_000);
        Twim0::write_reg(addr, 0x1A, 0x03).map_err(|_| Mpu9250Error::Bus)?;
        Twim0::write_reg(addr, 0x1B, 0x00).map_err(|_| Mpu9250Error::Bus)?;
        Twim0::write_reg(addr, 0x1C, 0x00).map_err(|_| Mpu9250Error::Bus)?;

        let bmp280_present = Twim0::read_reg(BMP280_ADDR, REG_BMP280_ID)
            .map(|id| id == 0x58)
            .unwrap_or(false);

        Ok(Self {
            addr,
            who_am_i,
            owner,
            ready: true,
            bmp280_present,
        })
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

    pub fn scan_device_count() -> u8 {
        TwimBus::init_pins(I2C_SDA_PIN, I2C_SCL_PIN);
        Twim0::scan(|_| {})
    }

    fn read_burst(&self) -> Result<([f32; 3], [f32; 3]), Mpu9250Error> {
        if !self.ready {
            return Err(Mpu9250Error::NotReady);
        }
        let mut raw = [0u8; 6];
        Twim0::write_read(self.addr, &[REG_ACCEL_XOUT_H], &mut raw)
            .map_err(|_| Mpu9250Error::Bus)?;

        let ax = i16::from_be_bytes([raw[0], raw[1]]);
        let ay = i16::from_be_bytes([raw[2], raw[3]]);
        let az = i16::from_be_bytes([raw[4], raw[5]]);
        let gx = 0i16;
        let gy = 0i16;
        let gz = 0i16;

        // +/-2 g and +/-250 dps factory defaults.
        let accel_g = [
            ax as f32 / 16_384.0,
            ay as f32 / 16_384.0,
            az as f32 / 16_384.0,
        ];
        let gyro_dps = [gx as f32 / 131.0, gy as f32 / 131.0, gz as f32 / 131.0];
        Ok((accel_g, gyro_dps))
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
        let now = Hal::now_us();
        let (accel_g, gyro_dps) = self.read_burst()?;
        let sample = SamplePool::alloc(SampleKind::Imu, ImuPayload::LEN, now, now)
            .ok_or(Mpu9250Error::PoolFull)?;
        let _ = ImuPayload::write_to_handle(sample.handle, &ImuPayload { accel_g, gyro_dps });
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
