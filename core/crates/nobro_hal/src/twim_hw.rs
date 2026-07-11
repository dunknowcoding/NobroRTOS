//! TWI0 master in legacy mode matching ArduinoNRF `Wire.cpp`.

use crate::bus::BusError;

const TWI0_BASE: u32 = 0x4000_3000;
const GPIO_PORT0_BASE: u32 = 0x5000_0000;
const GPIO_PORT_STRIDE: u32 = 0x300;
const GPIO_PIN_CNF0: u32 = 0x700;

const TWI_TASKS_STARTRX: u32 = 0x000;
const TWI_TASKS_STARTTX: u32 = 0x008;
const TWI_TASKS_STOP: u32 = 0x014;
const TWI_TASKS_RESUME: u32 = 0x020;
const TWI_SHORTS: u32 = 0x200;
const TWI_SHORTS_BB_SUSPEND: u32 = 1 << 0;
const TWI_SHORTS_BB_STOP: u32 = 1 << 1;
const TWI_EVENTS_RXDREADY: u32 = 0x108;
const TWI_EVENTS_TXDSENT: u32 = 0x11C;
const TWI_EVENTS_ERROR: u32 = 0x124;
const TWI_EVENTS_STOPPED: u32 = 0x104;
const TWI_ERRORSRC: u32 = 0x4C4;
const TWI_ENABLE: u32 = 0x500;
const TWI_PSELSCL: u32 = 0x508;
const TWI_PSELSDA: u32 = 0x50C;
const TWI_RXD: u32 = 0x518;
const TWI_TXD: u32 = 0x51C;
const TWI_FREQUENCY: u32 = 0x524;
const TWI_ADDRESS: u32 = 0x588;

const TWI_ENABLE_DISABLED: u32 = 0;
const TWI_ENABLE_ENABLED: u32 = 5;
const TWI_FREQUENCY_400K: u32 = 0x0640_0000;
const TIMEOUT_SPINS: u32 = 200_000;

fn reg(base: u32, off: u32) -> *mut u32 {
    (base + off) as *mut u32
}

fn clear_event(base: u32, off: u32) {
    unsafe {
        *reg(base, off) = 0;
    }
}

fn wait_event(base: u32, off: u32) -> Result<(), BusError> {
    for _ in 0..TIMEOUT_SPINS {
        unsafe {
            if *reg(base, TWI_EVENTS_ERROR) != 0 {
                return Err(BusError::Nack);
            }
            if *reg(base, off) != 0 {
                return Ok(());
            }
        }
        cortex_m::asm::nop();
    }
    Err(BusError::Timeout)
}

fn configure_open_drain(raw_pin: u32) {
    let base = GPIO_PORT0_BASE + (raw_pin >> 5) * GPIO_PORT_STRIDE;
    let pin = raw_pin & 0x1F;
    unsafe {
        // PULL=up, DRIVE=S0D1 (open drain), required for slave ACK.
        *reg(base, GPIO_PIN_CNF0 + pin * 4) = (3 << 2) | (6 << 8);
    }
}

fn recover_bus(sda: u8, scl: u8) {
    configure_open_drain(u32::from(sda));
    configure_open_drain(u32::from(scl));
    // Clock out a stuck slave (9 pulses) if SDA is low.
    for _ in 0..9 {
        unsafe {
            let scl_port = GPIO_PORT0_BASE + (u32::from(scl) >> 5) * GPIO_PORT_STRIDE;
            let scl_bit = u32::from(scl) & 0x1F;
            let sda_port = GPIO_PORT0_BASE + (u32::from(sda) >> 5) * GPIO_PORT_STRIDE;
            let sda_bit = u32::from(sda) & 0x1F;
            *reg(scl_port, 0x504) = 1 << scl_bit; // OUTCLR
            for _ in 0..100 {
                cortex_m::asm::nop();
            }
            *reg(scl_port, 0x508) = 1 << scl_bit; // OUTSET
            if (*reg(sda_port, 0x510 + sda_bit * 4) & 1) != 0 {
                break;
            }
        }
    }
}

pub struct Twim0;

impl Twim0 {
    /// # Safety
    /// Caller must own the Twim0 lease; `sda`/`scl` must be the board's wired I2C
    /// pins. Runs the 9-pulse bus recovery (drives SCL as GPIO) before enabling TWI.
    pub unsafe fn init(sda: u8, scl: u8) {
        recover_bus(sda, scl);
        let base = TWI0_BASE;
        *reg(base, TWI_ENABLE) = TWI_ENABLE_DISABLED;
        *reg(base, TWI_PSELSDA) = u32::from(sda);
        *reg(base, TWI_PSELSCL) = u32::from(scl);
        *reg(base, TWI_FREQUENCY) = TWI_FREQUENCY_400K;
        *reg(base, TWI_ENABLE) = TWI_ENABLE_ENABLED;
        configure_open_drain(u32::from(sda));
        configure_open_drain(u32::from(scl));
    }

    pub fn probe(addr: u8) -> bool {
        Self::write(addr, &[], true).is_ok()
    }

    pub fn scan<F: FnMut(u8)>(mut found: F) -> u8 {
        let mut count = 0u8;
        for addr in [0x68u8, 0x69] {
            if Self::read_reg(addr, 0x75).is_ok() {
                found(addr);
                count = count.saturating_add(1);
            }
        }
        for addr in [0x76u8, 0x77] {
            if Self::read_reg(addr, 0xD0).is_ok() {
                found(addr);
                count = count.saturating_add(1);
            }
        }
        count
    }

    pub fn write_reg(addr: u8, reg_addr: u8, val: u8) -> Result<(), BusError> {
        Self::write(addr, &[reg_addr, val], true)
    }

    pub fn read_reg(addr: u8, reg_addr: u8) -> Result<u8, BusError> {
        let mut buf = [0u8; 1];
        Self::write_read(addr, &[reg_addr], &mut buf)?;
        Ok(buf[0])
    }

    pub fn write_read(addr: u8, tx: &[u8], rx: &mut [u8]) -> Result<(), BusError> {
        if tx.is_empty() || rx.is_empty() {
            return Err(BusError::Timeout);
        }
        // Stop-start, not repeated-start, matches common MPU9250 bring-up.
        Self::write(addr, tx, true)?;
        Self::read(addr, rx, true)
    }

    /// Raw bus write of arbitrary bytes (STOP at the end). The general primitive an
    /// `embedded-hal` I2C adapter needs for `Operation::Write`.
    pub fn write_bytes(addr: u8, data: &[u8]) -> Result<(), BusError> {
        Self::write(addr, data, true)
    }

    /// Raw bus read of `buf.len()` bytes (STOP at the end). The general primitive an
    /// `embedded-hal` I2C adapter needs for `Operation::Read`.
    pub fn read_bytes(addr: u8, buf: &mut [u8]) -> Result<(), BusError> {
        if buf.is_empty() {
            return Ok(());
        }
        Self::read(addr, buf, true)
    }

    fn write(addr: u8, data: &[u8], send_stop: bool) -> Result<(), BusError> {
        let base = TWI0_BASE;
        unsafe {
            *reg(base, TWI_ADDRESS) = u32::from(addr);
            *reg(base, TWI_ERRORSRC) = 0xFFFF_FFFF;
            clear_event(base, TWI_EVENTS_ERROR);
            clear_event(base, TWI_EVENTS_TXDSENT);
            clear_event(base, TWI_EVENTS_STOPPED);
            *reg(base, TWI_TASKS_STARTTX) = 1;

            for &byte in data {
                *reg(base, TWI_TXD) = u32::from(byte);
                clear_event(base, TWI_EVENTS_TXDSENT);
                wait_event(base, TWI_EVENTS_TXDSENT)?;
            }

            if send_stop {
                *reg(base, TWI_TASKS_STOP) = 1;
                clear_event(base, TWI_EVENTS_STOPPED);
                wait_event(base, TWI_EVENTS_STOPPED)?;
            }
        }
        Ok(())
    }

    fn read(addr: u8, buf: &mut [u8], send_stop: bool) -> Result<(), BusError> {
        let base = TWI0_BASE;
        let request = buf.len();
        unsafe {
            *reg(base, TWI_ADDRESS) = u32::from(addr);
            *reg(base, TWI_ERRORSRC) = 0xFFFF_FFFF;
            clear_event(base, TWI_EVENTS_ERROR);
            clear_event(base, TWI_EVENTS_RXDREADY);
            clear_event(base, TWI_EVENTS_STOPPED);

            let shorts = if send_stop && request == 1 {
                TWI_SHORTS_BB_STOP
            } else {
                TWI_SHORTS_BB_SUSPEND
            };
            *reg(base, TWI_SHORTS) = shorts;
            *reg(base, TWI_TASKS_STARTRX) = 1;

            for (i, slot) in buf.iter_mut().enumerate() {
                wait_event(base, TWI_EVENTS_RXDREADY)?;
                clear_event(base, TWI_EVENTS_RXDREADY);
                *slot = (*reg(base, TWI_RXD) & 0xFF) as u8;

                let remaining = request - i - 1;
                if send_stop && remaining == 1 {
                    *reg(base, TWI_SHORTS) = TWI_SHORTS_BB_STOP;
                }
                if remaining >= 1 {
                    *reg(base, TWI_TASKS_RESUME) = 1;
                }
            }

            if send_stop {
                clear_event(base, TWI_EVENTS_STOPPED);
                let stopped = wait_event(base, TWI_EVENTS_STOPPED);
                *reg(base, TWI_SHORTS) = 0;
                stopped?;
            } else {
                *reg(base, TWI_SHORTS) = 0;
            }
        }
        Ok(())
    }
}
