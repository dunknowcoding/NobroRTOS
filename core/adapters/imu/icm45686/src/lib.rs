//! ICM-45686 sensor adapter, generic over `embedded-hal` I2C.

#![cfg_attr(not(test), no_std)]

use embedded_hal::i2c::I2c;

use nobro_imu::{
    magnitude3, ImuBackend, ImuDiagnostics, ImuEvent, ImuFamily, ImuIdentity, ImuSample,
};

pub const DEFAULT_ADDR: u8 = 0x68;
pub const REG_WHO_AM_I: u8 = 0x72;
pub const DEVICE_ID: u8 = 0xE9;

pub fn to_milli(raw: i16, full_scale: i32) -> i32 {
    ((i64::from(raw) * i64::from(full_scale) * 1000) / 32768) as i32
}

#[derive(Clone, Copy, Debug)]
pub struct Icm45686<I> {
    i2c: I,
    addr: u8,
    pub accel_fs_g: i32,
    pub gyro_fs_dps: i32,
    diagnostics: ImuDiagnostics,
}

impl<I: I2c> Icm45686<I> {
    pub fn new(i2c: I, addr: u8, accel_fs_g: i32, gyro_fs_dps: i32) -> Self {
        Self {
            i2c,
            addr,
            accel_fs_g,
            gyro_fs_dps,
            diagnostics: ImuDiagnostics::default(),
        }
    }

    fn read(&mut self, reg: u8, buf: &mut [u8]) -> Result<(), I::Error> {
        self.i2c.write_read(self.addr, &[reg], buf)
    }

    pub fn who_am_i(&mut self) -> Result<u8, I::Error> {
        let mut value = [0u8; 1];
        self.read(REG_WHO_AM_I, &mut value)?;
        Ok(value[0])
    }

    pub fn decode_accel(&self, raw: &[u8; 6]) -> [i32; 3] {
        decode(raw, self.accel_fs_g)
    }

    pub fn decode_gyro(&self, raw: &[u8; 6]) -> [i32; 3] {
        decode(raw, self.gyro_fs_dps)
    }
}

impl<I: I2c> ImuBackend for Icm45686<I> {
    type Error = I::Error;

    fn identity(&mut self) -> Result<ImuIdentity, Self::Error> {
        Ok(ImuIdentity {
            family: ImuFamily::ICM45686,
            who_am_i: self.who_am_i()?,
            address: self.addr,
            has_magnetometer: false,
        })
    }

    fn sample(&mut self) -> Result<ImuSample, Self::Error> {
        let mut raw = [0u8; 12];
        if let Err(error) = self.read(0x00, &mut raw) {
            self.diagnostics.read_errors = self.diagnostics.read_errors.saturating_add(1);
            self.diagnostics.consecutive_errors =
                self.diagnostics.consecutive_errors.saturating_add(1);
            self.diagnostics.last_event = ImuEvent::ReadError;
            return Err(error);
        }
        let accel = self.decode_accel(raw[0..6].try_into().expect("six accel bytes"));
        let gyro = self.decode_gyro(raw[6..12].try_into().expect("six gyro bytes"));
        self.diagnostics.samples = self.diagnostics.samples.saturating_add(1);
        self.diagnostics.consecutive_errors = 0;
        self.diagnostics.last_event = ImuEvent::Sample;
        Ok(ImuSample {
            accel_mg: accel,
            accel_mag_mg: magnitude3(accel),
            gyro_mdps: gyro,
            ..ImuSample::default()
        })
    }

    fn recover(&mut self) -> Result<(), Self::Error> {
        self.diagnostics.recovery_attempts = self.diagnostics.recovery_attempts.saturating_add(1);
        match self.who_am_i() {
            Ok(_) => {
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

fn decode(raw: &[u8; 6], full_scale: i32) -> [i32; 3] {
    [
        to_milli(i16::from_be_bytes([raw[0], raw[1]]), full_scale),
        to_milli(i16::from_be_bytes([raw[2], raw[3]]), full_scale),
        to_milli(i16::from_be_bytes([raw[4], raw[5]]), full_scale),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Error;
    impl embedded_hal::i2c::Error for Error {
        fn kind(&self) -> embedded_hal::i2c::ErrorKind {
            embedded_hal::i2c::ErrorKind::Other
        }
    }
    struct Mock;
    impl embedded_hal::i2c::ErrorType for Mock {
        type Error = Error;
    }
    impl I2c for Mock {
        fn transaction(
            &mut self,
            _address: u8,
            operations: &mut [embedded_hal::i2c::Operation<'_>],
        ) -> Result<(), Self::Error> {
            for operation in operations {
                if let embedded_hal::i2c::Operation::Read(bytes) = operation {
                    bytes.fill(0);
                }
            }
            Ok(())
        }
    }

    #[test]
    fn legacy_decode_behavior_is_canonical() {
        assert_eq!(to_milli(2048, 16), 1000);
        let device = Icm45686::new(Mock, DEFAULT_ADDR, 16, 2000);
        assert_eq!(
            device.decode_accel(&[0x08, 0x00, 0x00, 0x00, 0xF8, 0x00]),
            [1000, 0, -1000]
        );
    }
}
