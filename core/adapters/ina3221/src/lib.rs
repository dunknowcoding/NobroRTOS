//! INA3221 3-channel power monitor driver, generic over `embedded_hal::i2c::I2c`.
//!
//! Runs on NobroRTOS via the `NobroI2c` adapter (or any embedded-hal bus). Decodes the
//! per-channel bus voltage (LSB 8 mV) and shunt voltage (LSB 40 uV) into bus mV and
//! current uA given the shunt resistor. Matches the third-party `ina3221` app's readings
//! (verified: bus ~4704 mV, ~25.6 mA on a 100 mOhm shunt).
#![cfg_attr(not(test), no_std)]

use embedded_hal::i2c::I2c;

pub const DEFAULT_ADDR: u8 = 0x40;
pub const REG_MANUF_ID: u8 = 0xFE;
pub const MANUF_TI: u16 = 0x5449; // "TI"

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InaChannel {
    pub bus_mv: i32,
    pub current_ua: i32,
}

pub struct Ina3221<I> {
    i2c: I,
    addr: u8,
    shunt_mohm: u32,
}

impl<I: I2c> Ina3221<I> {
    pub fn new(i2c: I, addr: u8, shunt_mohm: u32) -> Self {
        Self {
            i2c,
            addr,
            shunt_mohm,
        }
    }

    fn read_reg(&mut self, reg: u8) -> Result<u16, I::Error> {
        let mut b = [0u8; 2];
        self.i2c.write_read(self.addr, &[reg], &mut b)?;
        Ok(u16::from_be_bytes(b))
    }

    pub fn manufacturer_id(&mut self) -> Result<u16, I::Error> {
        self.read_reg(REG_MANUF_ID)
    }

    /// Read channel `ch` (0..3): shunt reg = 1 + 2*ch, bus reg = 2 + 2*ch.
    pub fn read_channel(&mut self, ch: u8) -> Result<InaChannel, I::Error> {
        let shunt_raw = self.read_reg(1 + 2 * ch)? as i16;
        let bus_raw = self.read_reg(2 + 2 * ch)? as i16;
        let bus_mv = (i32::from(bus_raw >> 3)) * 8; // 8 mV/LSB
        let shunt_uv = (i32::from(shunt_raw >> 3)) * 40; // 40 uV/LSB
        let current_ua = if self.shunt_mohm > 0 {
            shunt_uv * 1000 / self.shunt_mohm as i32
        } else {
            0
        };
        Ok(InaChannel { bus_mv, current_ua })
    }

    pub fn read_all(&mut self) -> Result<[InaChannel; 3], I::Error> {
        Ok([
            self.read_channel(0)?,
            self.read_channel(1)?,
            self.read_channel(2)?,
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_hal::i2c::{Error, ErrorKind, ErrorType, I2c, Operation};

    struct Mock;
    #[derive(Debug)]
    struct E;
    impl Error for E {
        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }
    impl ErrorType for Mock {
        type Error = E;
    }
    impl I2c for Mock {
        fn transaction(&mut self, _a: u8, ops: &mut [Operation<'_>]) -> Result<(), E> {
            let mut reg = 0u8;
            for op in ops {
                match op {
                    Operation::Write(b) => reg = b[0],
                    Operation::Read(buf) => {
                        let v: u16 = match reg {
                            0xFE => MANUF_TI,
                            0x02 => 588 << 3, // bus ch1 -> 588*8 = 4704 mV
                            0x01 => 64 << 3,  // shunt ch1 -> 64*40 = 2560 uV
                            _ => 0,
                        };
                        buf.copy_from_slice(&v.to_be_bytes());
                    }
                    _ => {}
                }
            }
            Ok(())
        }
    }

    #[test]
    fn decodes_bus_and_current_like_the_live_module() {
        let mut ina = Ina3221::new(Mock, DEFAULT_ADDR, 100); // 100 mOhm shunt
        assert_eq!(ina.manufacturer_id().unwrap(), MANUF_TI);
        let ch = ina.read_channel(0).unwrap();
        assert_eq!(ch.bus_mv, 4704); // matches the ESP32 reading
        assert_eq!(ch.current_ua, 25_600); // 2560 uV / 100 mOhm = 25.6 mA
    }
}
