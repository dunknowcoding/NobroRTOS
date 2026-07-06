//! ICM-45686 (TDK InvenSense) 6-axis IMU decode, generic over `embedded_hal::i2c::I2c`.
//!
//! The raw->physical conversion (`raw * full_scale * 1000 / 32768`) is the standard 16-bit
//! signed decode and is host-tested. This pairs with the Nano's ICM45686 Arduino sketch
//! that the multi-board collector already ingests (the interop path). Register addresses
//! are provided as datasheet defaults and should be confirmed on hardware.
#![cfg_attr(not(test), no_std)]

use embedded_hal::i2c::I2c;

pub const DEFAULT_ADDR: u8 = 0x68;
pub const REG_WHO_AM_I: u8 = 0x72;
pub const DEVICE_ID: u8 = 0xE9; // ICM-45686 (confirm against datasheet)

/// Decode one 16-bit signed axis to milli-units given the full scale (g for accel, dps
/// for gyro): `raw * full_scale * 1000 / 32768`.
pub fn to_milli(raw: i16, full_scale: i32) -> i32 {
    ((i64::from(raw) * i64::from(full_scale) * 1000) / 32768) as i32
}

#[derive(Clone, Copy, Debug)]
pub struct Icm45686<I> {
    i2c: I,
    addr: u8,
    pub accel_fs_g: i32,
    pub gyro_fs_dps: i32,
}

impl<I: I2c> Icm45686<I> {
    pub fn new(i2c: I, addr: u8, accel_fs_g: i32, gyro_fs_dps: i32) -> Self {
        Self {
            i2c,
            addr,
            accel_fs_g,
            gyro_fs_dps,
        }
    }
    fn read(&mut self, reg: u8, buf: &mut [u8]) -> Result<(), I::Error> {
        self.i2c.write_read(self.addr, &[reg], buf)
    }
    pub fn who_am_i(&mut self) -> Result<u8, I::Error> {
        let mut b = [0u8; 1];
        self.read(REG_WHO_AM_I, &mut b)?;
        Ok(b[0])
    }
    /// Decode a 6-byte big-endian accel block into [x, y, z] milli-g.
    pub fn decode_accel(&self, raw: &[u8; 6]) -> [i32; 3] {
        [
            to_milli(i16::from_be_bytes([raw[0], raw[1]]), self.accel_fs_g),
            to_milli(i16::from_be_bytes([raw[2], raw[3]]), self.accel_fs_g),
            to_milli(i16::from_be_bytes([raw[4], raw[5]]), self.accel_fs_g),
        ]
    }
    /// Decode a 6-byte big-endian gyro block into [x, y, z] milli-dps.
    pub fn decode_gyro(&self, raw: &[u8; 6]) -> [i32; 3] {
        [
            to_milli(i16::from_be_bytes([raw[0], raw[1]]), self.gyro_fs_dps),
            to_milli(i16::from_be_bytes([raw[2], raw[3]]), self.gyro_fs_dps),
            to_milli(i16::from_be_bytes([raw[4], raw[5]]), self.gyro_fs_dps),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_scales_full_range_correctly() {
        // half of full scale -> half the range in milli-units
        assert_eq!(to_milli(2048, 16), 1000); // 2048 LSB at +/-16 g == 1 g == 1000 mg
        assert_eq!(to_milli(16384, 2000), 1000_000); // half-scale at +/-2000 dps == 1000 dps
        assert_eq!(to_milli(-16384, 16), -8000); // -0.5 * 16 g
    }

    #[test]
    fn decode_blocks_are_be16() {
        // a dummy device configured +/-16 g, +/-2000 dps
        struct M;
        #[derive(Debug)]
        struct E;
        impl embedded_hal::i2c::Error for E {
            fn kind(&self) -> embedded_hal::i2c::ErrorKind {
                embedded_hal::i2c::ErrorKind::Other
            }
        }
        impl embedded_hal::i2c::ErrorType for M {
            type Error = E;
        }
        impl embedded_hal::i2c::I2c for M {
            fn transaction(
                &mut self,
                _a: u8,
                _o: &mut [embedded_hal::i2c::Operation<'_>],
            ) -> Result<(), E> {
                Ok(())
            }
        }
        let dev = Icm45686::new(M, DEFAULT_ADDR, 16, 2000);
        // x = 2048 (1 g), y = 0, z = -2048 (-1 g)
        let a = dev.decode_accel(&[0x08, 0x00, 0x00, 0x00, 0xF8, 0x00]);
        assert_eq!(a, [1000, 0, -1000]);
    }
}
