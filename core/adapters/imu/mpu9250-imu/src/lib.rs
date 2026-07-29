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
    Lease,
    PoolFull,
    NotReady,
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

        self.bmp280_present = self
            .read_reg(BMP280_ADDR, REG_BMP280_ID)
            .map(|id| id == 0x58)
            .unwrap_or(false);
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
        traits::HalClock, ActivePlatform, Resource, ResourceLease, I2C_SCL_PIN, I2C_SDA_PIN,
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
        pub fn probe_and_init(owner: u8) -> Result<Self, Mpu9250Error> {
            let bus =
                NobroI2c::new(owner, I2C_SDA_PIN, I2C_SCL_PIN).map_err(|_| Mpu9250Error::Bus)?;
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
            let bus =
                NobroI2c::new(owner, I2C_SDA_PIN, I2C_SCL_PIN).map_err(|_| Mpu9250Error::Bus)?;
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
                        (BMP280_ADDR, Some(REG_BMP280_ID)) => bytes[0] = 0x58,
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
        assert!(imu.bmp280_present());
        let sample = ImuBackend::sample(&mut imu).unwrap();
        assert_eq!(sample.timestamp_us, 123);
        assert_eq!(sample.accel_mg[2], 1000);
        assert_eq!(imu.transport_diagnostics().irq_events, 1);
        ImuBackend::recover(&mut imu).unwrap();
        assert_eq!(ImuBackend::diagnostics(&imu).recoveries, 1);
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
