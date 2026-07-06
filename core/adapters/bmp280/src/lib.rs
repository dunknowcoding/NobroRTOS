//! BMP280 pressure + temperature sensor driver, generic over `embedded_hal::i2c::I2c`.
//!
//! Runs on NobroRTOS via the `NobroI2c` adapter. The temperature/pressure compensation is
//! the Bosch BMP280 datasheet fixed-point algorithm; the temperature path is host-verified
//! against the datasheet's reference vector (t_fine = 128422, T = 25.08 C).
#![cfg_attr(not(test), no_std)]

use embedded_hal::i2c::I2c;

pub const DEFAULT_ADDR: u8 = 0x76;
pub const REG_ID: u8 = 0xD0;
pub const CHIP_ID: u8 = 0x58; // BMP280

/// Factory calibration coefficients (read from 0x88..0x9F).
#[derive(Clone, Copy, Debug, Default)]
pub struct Bmp280Calib {
    pub t1: u16,
    pub t2: i16,
    pub t3: i16,
    pub p1: u16,
    pub p2: i16,
    pub p3: i16,
    pub p4: i16,
    pub p5: i16,
    pub p6: i16,
    pub p7: i16,
    pub p8: i16,
    pub p9: i16,
}

/// Compensate a raw 20-bit temperature reading. Returns (t_fine, temp in 0.01 C).
pub fn compensate_temp(c: &Bmp280Calib, adc_t: i32) -> (i32, i32) {
    let t1 = i32::from(c.t1);
    let var1 = (((adc_t >> 3) - (t1 << 1)) * i32::from(c.t2)) >> 11;
    let var2 = ((((adc_t >> 4) - t1) * ((adc_t >> 4) - t1)) >> 12) * i32::from(c.t3) >> 14;
    let t_fine = var1 + var2;
    let t = (t_fine * 5 + 128) >> 8;
    (t_fine, t)
}

/// Compensate a raw 20-bit pressure reading (64-bit path). Returns pressure in Pa.
pub fn compensate_pressure(c: &Bmp280Calib, adc_p: i32, t_fine: i32) -> u32 {
    let mut var1 = i64::from(t_fine) - 128_000;
    let mut var2 = var1 * var1 * i64::from(c.p6);
    var2 += (var1 * i64::from(c.p5)) << 17;
    var2 += i64::from(c.p4) << 35;
    var1 = ((var1 * var1 * i64::from(c.p3)) >> 8) + ((var1 * i64::from(c.p2)) << 12);
    var1 = (((1i64 << 47) + var1) * i64::from(c.p1)) >> 33;
    if var1 == 0 {
        return 0;
    }
    let mut p = 1_048_576i64 - i64::from(adc_p);
    p = (((p << 31) - var2) * 3125) / var1;
    var1 = (i64::from(c.p9) * (p >> 13) * (p >> 13)) >> 25;
    var2 = (i64::from(c.p8) * p) >> 19;
    p = ((p + var1 + var2) >> 8) + (i64::from(c.p7) << 4);
    (p / 256) as u32 // Q24.8 -> Pa
}

pub struct Bmp280<I> {
    i2c: I,
    addr: u8,
    calib: Bmp280Calib,
}

impl<I: I2c> Bmp280<I> {
    pub fn new(i2c: I, addr: u8) -> Self {
        Self {
            i2c,
            addr,
            calib: Bmp280Calib::default(),
        }
    }
    fn read(&mut self, reg: u8, buf: &mut [u8]) -> Result<(), I::Error> {
        self.i2c.write_read(self.addr, &[reg], buf)
    }
    pub fn chip_id(&mut self) -> Result<u8, I::Error> {
        let mut b = [0u8; 1];
        self.read(REG_ID, &mut b)?;
        Ok(b[0])
    }
    /// Load the 24-byte calibration block at 0x88 (little-endian words).
    pub fn load_calibration(&mut self) -> Result<(), I::Error> {
        let mut b = [0u8; 24];
        self.read(0x88, &mut b)?;
        let le16 = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        self.calib = Bmp280Calib {
            t1: le16(0),
            t2: le16(2) as i16,
            t3: le16(4) as i16,
            p1: le16(6),
            p2: le16(8) as i16,
            p3: le16(10) as i16,
            p4: le16(12) as i16,
            p5: le16(14) as i16,
            p6: le16(16) as i16,
            p7: le16(18) as i16,
            p8: le16(20) as i16,
            p9: le16(22) as i16,
        };
        Ok(())
    }
    pub fn calibration(&self) -> Bmp280Calib {
        self.calib
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Bosch BMP280 datasheet reference calibration + reference conversions.
    fn datasheet_calib() -> Bmp280Calib {
        Bmp280Calib {
            t1: 27504,
            t2: 26435,
            t3: -1000,
            p1: 36477,
            p2: -10685,
            p3: 3024,
            p4: 2855,
            p5: 140,
            p6: -7,
            p7: 15500,
            p8: -14600,
            p9: 6000,
        }
    }

    #[test]
    fn temp_matches_datasheet_reference() {
        let (t_fine, t) = compensate_temp(&datasheet_calib(), 519_888);
        assert_eq!(t_fine, 128_422);
        assert_eq!(t, 2508); // 25.08 C
    }

    #[test]
    fn pressure_is_plausible_sea_level() {
        let p = compensate_pressure(&datasheet_calib(), 415_148, 128_422);
        assert!(
            (99_000..=101_500).contains(&p),
            "pressure {p} Pa out of range"
        );
    }
}
