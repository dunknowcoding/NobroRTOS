//! Portable MPU9250-family adapter.
//!
//! Sensor logic is generic over an injected `embedded-hal` I2C bus, monotonic
//! clock, delay, interrupt source, and lease controller. The retained
//! [`Mpu9250Imu`] alias composes those contracts with the nRF52840 provider, so
//! existing applications keep their source API without making the driver itself
//! an nRF driver.

#![no_std]

use embedded_hal::{delay::DelayNs, i2c::I2c};
use nobro_imu::{
    magnitude3, ImuBackend, ImuCapabilities, ImuCapabilityBackend, ImuCompositeBackend,
    ImuCompositeStatus, ImuDiagnostics, ImuEvent, ImuFamily, ImuIdentity, ImuPressureBackend,
    ImuPressureSample, ImuSample,
};
use nobro_kernel::{
    pool::{CompactImuPayload, SamplePool},
    Capability, CapabilitySet, Criticality, MemoryBudget, ModuleId, ModuleSpec, Sample, SampleKind,
};
use nobro_sal::{AdapterManifest, SensorSal};

const REG_WHO_AM_I: u8 = 0x75;
const REG_PWR_MGMT_1: u8 = 0x6B;
const REG_ACCEL_XOUT_H: u8 = 0x3B;
#[cfg(feature = "bmp280-companion")]
const REG_BMP280_ID: u8 = 0xD0;
#[cfg(feature = "bmp280-companion")]
const REG_BMP280_CALIBRATION: u8 = 0x88;
#[cfg(feature = "bmp280-companion")]
const REG_BMP280_CTRL_MEAS: u8 = 0xF4;
const REG_BMP280_DATA: u8 = 0xF7;
const BMP280_ADDR: u8 = 0x76;
#[cfg(feature = "bmp280-companion")]
const BMP280_CTRL_TEMP_X1_PRESSURE_X1_NORMAL: u8 = 0x27;

const WHO_MPU6050: u8 = 0x68;
const WHO_MPU6500: u8 = 0x70;
const WHO_MPU9250: u8 = 0x71;
const WHO_MPU9255: u8 = 0x73;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mpu9250Error {
    NotFound,
    WhoAmMismatch,
    Bus,
    Lease,
    PoolFull,
    NotReady,
    CompanionUnavailable,
    InvalidCalibration,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Bmp280Calibration {
    t1: u16,
    t2: i16,
    t3: i16,
    p1: u16,
    p2: i16,
    p3: i16,
    p4: i16,
    p5: i16,
    p6: i16,
    p7: i16,
    p8: i16,
    p9: i16,
}

impl Bmp280Calibration {
    #[cfg(feature = "bmp280-companion")]
    fn from_registers(bytes: [u8; 24]) -> Result<Self, Mpu9250Error> {
        let u16_at = |offset: usize| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        let i16_at = |offset: usize| i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        let calibration = Self {
            t1: u16_at(0),
            t2: i16_at(2),
            t3: i16_at(4),
            p1: u16_at(6),
            p2: i16_at(8),
            p3: i16_at(10),
            p4: i16_at(12),
            p5: i16_at(14),
            p6: i16_at(16),
            p7: i16_at(18),
            p8: i16_at(20),
            p9: i16_at(22),
        };
        if calibration.t1 == 0 || calibration.t1 == u16::MAX || calibration.p1 == 0 {
            Err(Mpu9250Error::InvalidCalibration)
        } else {
            Ok(calibration)
        }
    }

    /// Bosch BST-BMP280-DS001 rev. 1.26, section 8.2 fixed-point
    /// compensation. The returned pressure is integer pascals.
    fn compensate(self, adc_temperature: i32, adc_pressure: i32) -> Option<(i32, u32)> {
        let var1 =
            (((adc_temperature >> 3) - (i32::from(self.t1) << 1)) * i32::from(self.t2)) >> 11;
        let delta = (adc_temperature >> 4) - i32::from(self.t1);
        let var2 = (((delta * delta) >> 12) * i32::from(self.t3)) >> 14;
        let t_fine = var1 + var2;
        let temperature_centi_c = (t_fine * 5 + 128) >> 8;

        let mut p_var1 = (t_fine >> 1) - 64_000;
        let mut p_var2 = ((((p_var1 >> 2) * (p_var1 >> 2)) >> 11) * i32::from(self.p6))
            + ((p_var1 * i32::from(self.p5)) << 1);
        p_var2 = (p_var2 >> 2) + (i32::from(self.p4) << 16);
        p_var1 = (((i32::from(self.p3) * (((p_var1 >> 2) * (p_var1 >> 2)) >> 13)) >> 3)
            + ((i32::from(self.p2) * p_var1) >> 1))
            >> 18;
        p_var1 = ((32_768 + p_var1) * i32::from(self.p1)) >> 15;
        if p_var1 == 0 {
            return None;
        }
        let initial = (1_048_576i64 - i64::from(adc_pressure) - i64::from(p_var2 >> 12)) * 3_125;
        let mut pressure = if initial < 0x8000_0000 {
            (initial << 1) / i64::from(p_var1)
        } else {
            (initial / i64::from(p_var1)) * 2
        };
        let correction1 = (i64::from(self.p9) * (((pressure >> 3) * (pressure >> 3)) >> 13)) >> 12;
        let correction2 = ((pressure >> 2) * i64::from(self.p8)) >> 13;
        pressure += (correction1 + correction2 + i64::from(self.p7)) >> 4;
        u32::try_from(pressure)
            .ok()
            .map(|pressure_pa| (temperature_centi_c, pressure_pa))
    }
}

pub trait MonotonicClock {
    fn now_us(&self) -> u64;
}

pub trait InterruptSource {
    const PRESENT: bool;
    fn take_pending(&mut self) -> bool;
}

pub trait BusLease<B> {
    fn validate(&self) -> bool;
    fn recover(&mut self, bus: &mut B) -> Result<(), Mpu9250Error>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoInterrupt;

impl InterruptSource for NoInterrupt {
    const PRESENT: bool = false;

    fn take_pending(&mut self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoLease;

impl<B> BusLease<B> for NoLease {
    fn validate(&self) -> bool {
        true
    }

    fn recover(&mut self, _bus: &mut B) -> Result<(), Mpu9250Error> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mpu9250TransportDiagnostics {
    pub irq_events: u32,
    pub lease_rejections: u32,
}

pub struct PortableMpu9250Imu<B, C, D, I, L> {
    bus: B,
    clock: C,
    delay: D,
    irq: I,
    lease: L,
    addr: u8,
    who_am_i: u8,
    owner: u8,
    ready: bool,
    bmp280_present: bool,
    bmp280_healthy: bool,
    bmp280_calibration: Option<Bmp280Calibration>,
    composite_generation: u32,
    last_temp_centi: u32,
    last_gyro_mdps: u32,
    diagnostics: ImuDiagnostics,
    transport_diagnostics: Mpu9250TransportDiagnostics,
}

impl<B, C, D, I, L> PortableMpu9250Imu<B, C, D, I, L>
where
    B: I2c,
    C: MonotonicClock,
    D: DelayNs,
    I: InterruptSource,
    L: BusLease<B>,
{
    pub fn mount(
        bus: B,
        clock: C,
        delay: D,
        irq: I,
        lease: L,
        owner: u8,
    ) -> Result<Self, Mpu9250Error> {
        let mut sensor = Self {
            bus,
            clock,
            delay,
            irq,
            lease,
            addr: 0,
            who_am_i: 0,
            owner,
            ready: false,
            bmp280_present: false,
            bmp280_healthy: false,
            bmp280_calibration: None,
            composite_generation: 1,
            last_temp_centi: 0,
            last_gyro_mdps: 0,
            diagnostics: ImuDiagnostics::default(),
            transport_diagnostics: Mpu9250TransportDiagnostics::default(),
        };
        sensor.initialize()?;
        Ok(sensor)
    }

    fn ensure_lease(&mut self) -> Result<(), Mpu9250Error> {
        if self.lease.validate() {
            Ok(())
        } else {
            self.transport_diagnostics.lease_rejections = self
                .transport_diagnostics
                .lease_rejections
                .saturating_add(1);
            Err(Mpu9250Error::Lease)
        }
    }

    fn bus_mut(&mut self) -> Result<&mut B, Mpu9250Error> {
        self.ensure_lease()?;
        Ok(&mut self.bus)
    }

    fn read_reg(&mut self, address: u8, register: u8) -> Result<u8, Mpu9250Error> {
        let mut value = [0u8; 1];
        self.bus_mut()?
            .write_read(address, &[register], &mut value)
            .map_err(|_| Mpu9250Error::Bus)?;
        Ok(value[0])
    }

    fn write_reg(&mut self, address: u8, register: u8, value: u8) -> Result<(), Mpu9250Error> {
        self.bus_mut()?
            .write(address, &[register, value])
            .map_err(|_| Mpu9250Error::Bus)
    }

    fn read_registers(
        &mut self,
        address: u8,
        register: u8,
        output: &mut [u8],
    ) -> Result<(), Mpu9250Error> {
        self.bus_mut()?
            .write_read(address, &[register], output)
            .map_err(|_| Mpu9250Error::Bus)
    }

    #[cfg(feature = "bmp280-companion")]
    fn initialize_bmp280(&mut self) -> Result<(), Mpu9250Error> {
        if self.read_reg(BMP280_ADDR, REG_BMP280_ID)? != 0x58 {
            return Err(Mpu9250Error::CompanionUnavailable);
        }
        let mut raw_calibration = [0u8; 24];
        self.read_registers(BMP280_ADDR, REG_BMP280_CALIBRATION, &mut raw_calibration)?;
        let calibration = Bmp280Calibration::from_registers(raw_calibration)?;
        self.write_reg(
            BMP280_ADDR,
            REG_BMP280_CTRL_MEAS,
            BMP280_CTRL_TEMP_X1_PRESSURE_X1_NORMAL,
        )?;
        self.bmp280_calibration = Some(calibration);
        self.bmp280_healthy = true;
        Ok(())
    }

    fn initialize(&mut self) -> Result<(), Mpu9250Error> {
        self.ready = false;
        self.ensure_lease()?;
        let mut found = None;
        for addr in [0x68u8, 0x69] {
            if let Ok(id) = self.read_reg(addr, REG_WHO_AM_I) {
                if matches!(id, WHO_MPU6050 | WHO_MPU6500 | WHO_MPU9250 | WHO_MPU9255) {
                    found = Some((addr, id));
                    break;
                }
            }
        }
        let (addr, who_am_i) = found.ok_or(Mpu9250Error::NotFound)?;
        self.addr = addr;
        self.who_am_i = who_am_i;

        self.write_reg(addr, REG_PWR_MGMT_1, 0x01)?;
        self.delay.delay_us(8_000);
        self.write_reg(addr, 0x1A, 0x03)?;
        self.write_reg(addr, 0x1B, 0x00)?;
        self.write_reg(addr, 0x1C, 0x00)?;

        self.bmp280_calibration = None;
        #[cfg(feature = "bmp280-companion")]
        {
            self.bmp280_present = self.initialize_bmp280().is_ok();
        }
        #[cfg(not(feature = "bmp280-companion"))]
        {
            self.bmp280_present = false;
        }
        self.bmp280_healthy = self.bmp280_present;
        self.ready = true;
        Ok(())
    }

    /// Die temperature from the most recent burst, in centi-degrees C.
    pub const fn last_temp_centi_c(&self) -> u32 {
        self.last_temp_centi
    }

    /// Gyro magnitude from the most recent burst, in milli-deg/s.
    pub const fn last_gyro_mag_mdps(&self) -> u32 {
        self.last_gyro_mdps
    }

    pub const fn addr(&self) -> u8 {
        self.addr
    }

    pub const fn who_am_i(&self) -> u8 {
        self.who_am_i
    }

    pub const fn bmp280_present(&self) -> bool {
        self.bmp280_present
    }

    pub const fn owner(&self) -> u8 {
        self.owner
    }

    pub const fn interrupt_present(&self) -> bool {
        I::PRESENT
    }

    pub const fn transport_diagnostics(&self) -> Mpu9250TransportDiagnostics {
        self.transport_diagnostics
    }

    fn read_burst(&mut self) -> Result<([f32; 3], [f32; 3]), Mpu9250Error> {
        if !self.ready {
            return Err(Mpu9250Error::NotReady);
        }
        if self.irq.take_pending() {
            self.transport_diagnostics.irq_events =
                self.transport_diagnostics.irq_events.saturating_add(1);
        }
        let address = self.addr;
        let mut raw = [0u8; 14];
        self.bus_mut()?
            .write_read(address, &[REG_ACCEL_XOUT_H], &mut raw)
            .map_err(|_| Mpu9250Error::Bus)?;

        let ax = i16::from_be_bytes([raw[0], raw[1]]);
        let ay = i16::from_be_bytes([raw[2], raw[3]]);
        let az = i16::from_be_bytes([raw[4], raw[5]]);
        let temp_raw = i16::from_be_bytes([raw[6], raw[7]]);
        let gx = i16::from_be_bytes([raw[8], raw[9]]);
        let gy = i16::from_be_bytes([raw[10], raw[11]]);
        let gz = i16::from_be_bytes([raw[12], raw[13]]);

        let accel_g = [
            ax as f32 / 16_384.0,
            ay as f32 / 16_384.0,
            az as f32 / 16_384.0,
        ];
        let gyro_dps = [gx as f32 / 131.0, gy as f32 / 131.0, gz as f32 / 131.0];
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

    fn read_pressure(&mut self) -> Result<ImuPressureSample, Mpu9250Error> {
        let calibration = self
            .bmp280_calibration
            .ok_or(Mpu9250Error::CompanionUnavailable)?;
        let mut raw = [0u8; 6];
        if let Err(error) = self.read_registers(BMP280_ADDR, REG_BMP280_DATA, &mut raw) {
            self.bmp280_healthy = false;
            return Err(error);
        }
        let adc_pressure =
            (i32::from(raw[0]) << 12) | (i32::from(raw[1]) << 4) | (i32::from(raw[2]) >> 4);
        let adc_temperature =
            (i32::from(raw[3]) << 12) | (i32::from(raw[4]) << 4) | (i32::from(raw[5]) >> 4);
        let (temperature_centi_c, pressure_pa) = calibration
            .compensate(adc_temperature, adc_pressure)
            .ok_or(Mpu9250Error::InvalidCalibration)?;
        self.bmp280_healthy = true;
        let altitude_m =
            44_330.0 * (1.0 - libm::powf(pressure_pa as f32 / 101_325.0, 0.190_294_95));
        Ok(ImuPressureSample {
            pressure_pa,
            temperature_centi_c,
            altitude_mm: (altitude_m * 1_000.0) as i32,
            timestamp_us: self.clock.now_us(),
        })
    }
}

impl<B, C, D, I, L> ImuBackend for PortableMpu9250Imu<B, C, D, I, L>
where
    B: I2c,
    C: MonotonicClock,
    D: DelayNs,
    I: InterruptSource,
    L: BusLease<B>,
{
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
            timestamp_us: self.clock.now_us(),
            ..ImuSample::default()
        })
    }

    fn recover(&mut self) -> Result<(), Self::Error> {
        self.diagnostics.recovery_attempts = self.diagnostics.recovery_attempts.saturating_add(1);
        self.ready = false;
        self.lease.recover(&mut self.bus)?;
        match self.initialize() {
            Ok(()) => {
                self.composite_generation = self.composite_generation.saturating_add(1);
                self.diagnostics.recoveries = self.diagnostics.recoveries.saturating_add(1);
                self.diagnostics.consecutive_errors = 0;
                self.diagnostics.last_event = ImuEvent::Recovered;
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

impl<B, C, D, I, L> ImuCapabilityBackend for PortableMpu9250Imu<B, C, D, I, L>
where
    B: I2c,
    C: MonotonicClock,
    D: DelayNs,
    I: InterruptSource,
    L: BusLease<B>,
{
    fn capabilities(&self) -> ImuCapabilities {
        let mut capabilities = ImuCapabilities::COMPOSITE;
        if self.bmp280_present {
            capabilities = capabilities.union(ImuCapabilities::PRESSURE);
        }
        capabilities
    }
}

impl<B, C, D, I, L> ImuPressureBackend for PortableMpu9250Imu<B, C, D, I, L>
where
    B: I2c,
    C: MonotonicClock,
    D: DelayNs,
    I: InterruptSource,
    L: BusLease<B>,
{
    fn pressure_sample(&mut self) -> Result<ImuPressureSample, Self::Error> {
        self.read_pressure()
    }
}

impl<B, C, D, I, L> ImuCompositeBackend for PortableMpu9250Imu<B, C, D, I, L>
where
    B: I2c,
    C: MonotonicClock,
    D: DelayNs,
    I: InterruptSource,
    L: BusLease<B>,
{
    fn composite_status(&mut self) -> Result<ImuCompositeStatus, Self::Error> {
        self.ensure_lease()?;
        let present_mask = 1 | (u16::from(self.bmp280_present) << 1);
        let healthy_mask = u16::from(self.ready) | (u16::from(self.bmp280_healthy) << 1);
        Ok(ImuCompositeStatus {
            present_mask,
            healthy_mask,
            generation: self.composite_generation,
        })
    }
}

impl<B, C, D, I, L> AdapterManifest for PortableMpu9250Imu<B, C, D, I, L> {
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

impl<B, C, D, I, L> SensorSal for PortableMpu9250Imu<B, C, D, I, L>
where
    B: I2c,
    C: MonotonicClock,
    D: DelayNs,
    I: InterruptSource,
    L: BusLease<B>,
{
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

pub fn accel_mag_mg(accel_g: [f32; 3]) -> u32 {
    let mag_sq = accel_g[0] * accel_g[0] + accel_g[1] * accel_g[1] + accel_g[2] * accel_g[2];
    (libm::sqrtf(mag_sq) * 1000.0) as u32
}

pub fn imu_plausible(accel_g: [f32; 3]) -> bool {
    let mag_sq = accel_g[0] * accel_g[0] + accel_g[1] * accel_g[1] + accel_g[2] * accel_g[2];
    (0.64..1.69).contains(&mag_sq)
}

#[cfg(feature = "nrf52840")]
mod nrf {
    use super::*;
    use nobro_eh_i2c::NobroI2c;
    use nobro_hal::{
        traits::HalClock, ActivePlatform, Resource, ResourceLease, TwimFrequency, I2C_SCL_PIN,
        I2C_SDA_PIN,
    };

    #[derive(Clone, Copy, Debug, Default)]
    pub struct NrfClock;

    impl MonotonicClock for NrfClock {
        fn now_us(&self) -> u64 {
            ActivePlatform::now_us()
        }
    }

    #[derive(Clone, Copy, Debug, Default)]
    pub struct NrfDelay;

    impl DelayNs for NrfDelay {
        fn delay_ns(&mut self, ns: u32) {
            let cycles = ns / 16 + 1;
            for _ in 0..cycles {
                cortex_m::asm::nop();
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct NrfLease {
        owner: u8,
    }

    impl NrfLease {
        pub const fn new(owner: u8) -> Self {
            Self { owner }
        }
    }

    impl BusLease<NobroI2c> for NrfLease {
        fn validate(&self) -> bool {
            ResourceLease::owner(Resource::Twim0) == Some(self.owner)
        }

        fn recover(&mut self, bus: &mut NobroI2c) -> Result<(), Mpu9250Error> {
            bus.recover(self.owner, I2C_SDA_PIN, I2C_SCL_PIN)
                .map_err(|_| Mpu9250Error::Bus)
        }
    }

    pub type Mpu9250Imu = PortableMpu9250Imu<NobroI2c, NrfClock, NrfDelay, NoInterrupt, NrfLease>;

    impl Mpu9250Imu {
        /// Mount the native nRF composition and own its generation-tagged TWIM0
        /// lease for the adapter lifetime. Callers must not pre-acquire TWIM0;
        /// doing so correctly fails this mount with `AlreadyHeld` before MMIO.
        pub fn probe_and_init(owner: u8) -> Result<Self, Mpu9250Error> {
            let bus = NobroI2c::new_with_frequency(
                owner,
                I2C_SDA_PIN,
                I2C_SCL_PIN,
                TwimFrequency::Khz400,
            )
            .map_err(|_| Mpu9250Error::Bus)?;
            Self::mount(
                bus,
                NrfClock,
                NrfDelay,
                NoInterrupt,
                NrfLease::new(owner),
                owner,
            )
        }

        pub fn scan_device_count(owner: u8) -> Result<u8, Mpu9250Error> {
            let bus = NobroI2c::new_with_frequency(
                owner,
                I2C_SDA_PIN,
                I2C_SCL_PIN,
                TwimFrequency::Khz400,
            )
            .map_err(|_| Mpu9250Error::Bus)?;
            bus.scan_device_count().map_err(|_| Mpu9250Error::Bus)
        }
    }
}

#[cfg(feature = "nrf52840")]
pub use nrf::{Mpu9250Imu, NrfClock, NrfDelay, NrfLease};

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_hal::i2c::{Error, ErrorKind, ErrorType, Operation};

    #[derive(Clone, Copy, Debug)]
    struct MockError;

    impl Error for MockError {
        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    struct MockBus;

    #[cfg(feature = "bmp280-companion")]
    const BMP280_EXAMPLE_CALIBRATION: [u8; 24] = [
        0x70, 0x6B, 0x43, 0x67, 0x18, 0xFC, 0x7D, 0x8E, 0x43, 0xD6, 0xD0, 0x0B, 0x27, 0x0B, 0x8C,
        0x00, 0xF9, 0xFF, 0x8C, 0x3C, 0xF8, 0xC6, 0x70, 0x17,
    ];

    impl ErrorType for MockBus {
        type Error = MockError;
    }

    impl I2c for MockBus {
        fn transaction(
            &mut self,
            address: u8,
            operations: &mut [Operation<'_>],
        ) -> Result<(), Self::Error> {
            let register = operations.iter().find_map(|operation| match operation {
                Operation::Write(bytes) => bytes.first().copied(),
                Operation::Read(_) => None,
            });
            for operation in operations {
                if let Operation::Read(bytes) = operation {
                    bytes.fill(0);
                    match (address, register) {
                        (0x68, Some(REG_WHO_AM_I)) => bytes[0] = WHO_MPU9250,
                        #[cfg(feature = "bmp280-companion")]
                        (BMP280_ADDR, Some(REG_BMP280_ID)) => bytes[0] = 0x58,
                        #[cfg(feature = "bmp280-companion")]
                        (BMP280_ADDR, Some(REG_BMP280_CALIBRATION)) if bytes.len() == 24 => {
                            bytes.copy_from_slice(&BMP280_EXAMPLE_CALIBRATION);
                        }
                        #[cfg(feature = "bmp280-companion")]
                        (BMP280_ADDR, Some(REG_BMP280_DATA)) if bytes.len() == 6 => {
                            bytes.copy_from_slice(&[0x65, 0x5A, 0xC0, 0x7E, 0xED, 0x00]);
                        }
                        (0x68, Some(REG_ACCEL_XOUT_H)) if bytes.len() == 14 => {
                            bytes[4] = 0x40;
                        }
                        _ => return Err(MockError),
                    }
                }
            }
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    struct Clock;
    impl MonotonicClock for Clock {
        fn now_us(&self) -> u64 {
            123
        }
    }

    #[derive(Clone, Copy)]
    struct Delay;
    impl DelayNs for Delay {
        fn delay_ns(&mut self, _ns: u32) {}
    }

    #[derive(Clone, Copy)]
    struct Irq(bool);
    impl InterruptSource for Irq {
        const PRESENT: bool = true;
        fn take_pending(&mut self) -> bool {
            core::mem::take(&mut self.0)
        }
    }

    #[test]
    fn portable_composition_mounts_samples_recovers_and_tracks_irq() {
        let mut imu =
            PortableMpu9250Imu::mount(MockBus, Clock, Delay, Irq(true), NoLease, 7).unwrap();
        assert_eq!(imu.who_am_i(), WHO_MPU9250);
        #[cfg(feature = "bmp280-companion")]
        assert!(imu.bmp280_present());
        #[cfg(not(feature = "bmp280-companion"))]
        assert!(!imu.bmp280_present());
        let sample = ImuBackend::sample(&mut imu).unwrap();
        assert_eq!(sample.timestamp_us, 123);
        assert_eq!(sample.accel_mg[2], 1000);
        assert_eq!(imu.transport_diagnostics().irq_events, 1);
        #[cfg(feature = "bmp280-companion")]
        {
            assert_eq!(
                ImuCapabilityBackend::capabilities(&imu),
                ImuCapabilities::COMPOSITE.union(ImuCapabilities::PRESSURE)
            );
            let pressure = ImuPressureBackend::pressure_sample(&mut imu).unwrap();
            assert!((100_600..=100_700).contains(&pressure.pressure_pa));
            assert_eq!(pressure.temperature_centi_c, 2508);
        }
        #[cfg(not(feature = "bmp280-companion"))]
        assert_eq!(
            ImuCapabilityBackend::capabilities(&imu),
            ImuCapabilities::COMPOSITE
        );
        let composite = ImuCompositeBackend::composite_status(&mut imu).unwrap();
        let expected_mask = if cfg!(feature = "bmp280-companion") {
            3
        } else {
            1
        };
        assert_eq!(composite.present_mask, expected_mask);
        assert_eq!(composite.healthy_mask, expected_mask);
        ImuBackend::recover(&mut imu).unwrap();
        assert_eq!(ImuBackend::diagnostics(&imu).recoveries, 1);
        assert_eq!(
            ImuCompositeBackend::composite_status(&mut imu)
                .unwrap()
                .generation,
            2
        );
    }

    #[cfg(feature = "bmp280-companion")]
    #[test]
    fn bmp280_datasheet_fixed_point_example_is_preserved() {
        let calibration = Bmp280Calibration::from_registers(BMP280_EXAMPLE_CALIBRATION).unwrap();
        let (temperature_centi_c, pressure_pa) = calibration.compensate(519_888, 415_148).unwrap();
        assert_eq!(temperature_centi_c, 2508);
        assert!((100_600..=100_700).contains(&pressure_pa));
    }

    #[test]
    fn invalid_lease_fails_before_bus_io() {
        struct Denied;
        impl BusLease<MockBus> for Denied {
            fn validate(&self) -> bool {
                false
            }
            fn recover(&mut self, _bus: &mut MockBus) -> Result<(), Mpu9250Error> {
                Ok(())
            }
        }
        assert!(matches!(
            PortableMpu9250Imu::mount(MockBus, Clock, Delay, Irq(false), Denied, 1),
            Err(Mpu9250Error::Lease)
        ));
    }
}
