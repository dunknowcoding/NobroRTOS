//! SPIM0 master for the nRF52840 (EasyDMA), for SPI sensors such as a GY-9250
//! (MPU-9250) wired for SPI. SPI mode 3, MSB-first, ~1 Mbps. CS is driven manually as
//! a GPIO output (idle high) so a whole register burst stays under one chip-select.

use crate::bus::BusError;

const SPIM0_BASE: u32 = 0x4000_3000; // shared peripheral block with TWIM0
const GPIO_PORT0_BASE: u32 = 0x5000_0000;
const GPIO_PORT_STRIDE: u32 = 0x300;
const GPIO_PIN_CNF0: u32 = 0x700;
const GPIO_OUTSET: u32 = 0x508;
const GPIO_OUTCLR: u32 = 0x50C;

const SPIM_TASKS_START: u32 = 0x010;
const SPIM_TASKS_STOP: u32 = 0x014;
const SPIM_EVENTS_END: u32 = 0x118;
const SPIM_ENABLE: u32 = 0x500;
const SPIM_PSEL_SCK: u32 = 0x508;
const SPIM_PSEL_MOSI: u32 = 0x50C;
const SPIM_PSEL_MISO: u32 = 0x510;
const SPIM_FREQUENCY: u32 = 0x524;
const SPIM_RXD_PTR: u32 = 0x534;
const SPIM_RXD_MAXCNT: u32 = 0x538;
const SPIM_TXD_PTR: u32 = 0x544;
const SPIM_TXD_MAXCNT: u32 = 0x548;
const SPIM_CONFIG: u32 = 0x554;

const SPIM_ENABLE_DISABLED: u32 = 0;
const SPIM_ENABLE_ENABLED: u32 = 7;
const SPIM_FREQ_250K: u32 = 0x0400_0000; // 250 kbps - robust over jumper wiring
const SPIM_CONFIG_MODE3: u32 = 0b110; // CPOL=1, CPHA=1, MSB-first
const TIMEOUT_SPINS: u32 = 200_000;

/// Max bytes per single EasyDMA transfer here (one register burst). 64 covers the
/// MPU-9250's 14-byte accel+temp+gyro burst with headroom.
pub const SPIM_XFER_MAX: usize = 64;

fn reg(base: u32, off: u32) -> *mut u32 {
    (base + off) as *mut u32
}

fn gpio(pin: u32) -> (u32, u32) {
    (
        GPIO_PORT0_BASE + (pin >> 5) * GPIO_PORT_STRIDE,
        pin & 0x1F,
    )
}

fn spin(cycles: u32) {
    for _ in 0..cycles {
        cortex_m::asm::nop();
    }
}

/// SPIM0 master with a software-driven chip-select.
pub struct Spim0 {
    cs: u8,
}

impl Spim0 {
    /// Configure SPIM0 on the given raw nRF pin numbers (mode 3, 1 Mbps). CS is set up
    /// as a GPIO output idling high. Caller must own the `Spim0` lease.
    pub unsafe fn init(sck: u8, mosi: u8, miso: u8, cs: u8) -> Self {
        let base = SPIM0_BASE;
        *reg(base, SPIM_ENABLE) = SPIM_ENABLE_DISABLED;

        // CS: output, idle high.
        let (cs_port, cs_bit) = gpio(u32::from(cs));
        *reg(cs_port, GPIO_OUTSET) = 1 << cs_bit;
        *reg(cs_port, GPIO_PIN_CNF0 + cs_bit * 4) = 1; // DIR=output

        // SCK: output, idle high (CPOL=1); MOSI: output; MISO: input.
        let (sck_port, sck_bit) = gpio(u32::from(sck));
        *reg(sck_port, GPIO_OUTSET) = 1 << sck_bit;
        *reg(sck_port, GPIO_PIN_CNF0 + sck_bit * 4) = 1;
        let (mosi_port, mosi_bit) = gpio(u32::from(mosi));
        *reg(mosi_port, GPIO_PIN_CNF0 + mosi_bit * 4) = 1;
        let (miso_port, miso_bit) = gpio(u32::from(miso));
        *reg(miso_port, GPIO_PIN_CNF0 + miso_bit * 4) = 0; // input, connect buffer

        *reg(base, SPIM_PSEL_SCK) = u32::from(sck);
        *reg(base, SPIM_PSEL_MOSI) = u32::from(mosi);
        *reg(base, SPIM_PSEL_MISO) = u32::from(miso);
        *reg(base, SPIM_FREQUENCY) = SPIM_FREQ_250K;
        *reg(base, SPIM_CONFIG) = SPIM_CONFIG_MODE3;
        *reg(base, SPIM_ENABLE) = SPIM_ENABLE_ENABLED;

        Spim0 { cs }
    }

    /// Assert chip-select (active low).
    pub fn select(&self) {
        let (port, bit) = gpio(u32::from(self.cs));
        unsafe {
            *reg(port, GPIO_OUTCLR) = 1 << bit;
        }
    }

    /// Release chip-select.
    pub fn deselect(&self) {
        let (port, bit) = gpio(u32::from(self.cs));
        unsafe {
            *reg(port, GPIO_OUTSET) = 1 << bit;
        }
    }

    /// One full-duplex EasyDMA transfer of `n = min(tx.len(), rx.len())` bytes, WITHOUT
    /// touching chip-select (caller brackets with select()/deselect()). EasyDMA needs
    /// RAM buffers, so the bytes are staged through stack buffers.
    pub fn transfer_held(&self, tx: &[u8], rx: &mut [u8]) -> Result<(), BusError> {
        let n = tx.len().min(rx.len());
        if n == 0 || n > SPIM_XFER_MAX {
            return Err(BusError::Timeout);
        }
        let base = SPIM0_BASE;
        let mut txbuf = [0u8; SPIM_XFER_MAX];
        let mut rxbuf = [0u8; SPIM_XFER_MAX];
        txbuf[..n].copy_from_slice(&tx[..n]);
        let result = unsafe {
            *reg(base, SPIM_TXD_PTR) = txbuf.as_ptr() as u32;
            *reg(base, SPIM_TXD_MAXCNT) = n as u32;
            *reg(base, SPIM_RXD_PTR) = rxbuf.as_mut_ptr() as u32;
            *reg(base, SPIM_RXD_MAXCNT) = n as u32;
            *reg(base, SPIM_EVENTS_END) = 0;
            cortex_m::asm::dsb(); // buffers committed before DMA starts
            *reg(base, SPIM_TASKS_START) = 1;
            let mut out = Err(BusError::Timeout);
            for _ in 0..TIMEOUT_SPINS {
                if *reg(base, SPIM_EVENTS_END) != 0 {
                    out = Ok(());
                    break;
                }
                cortex_m::asm::nop();
            }
            *reg(base, SPIM_TASKS_STOP) = 1;
            cortex_m::asm::dsb();
            out
        };
        rx[..n].copy_from_slice(&rxbuf[..n]);
        result
    }

    /// Full-duplex transfer bracketed by chip-select (one standalone transaction). A
    /// CS setup delay (after assert) and recovery delay (after release) give the slave
    /// the inter-transaction time it needs - without them, rapid back-to-back register
    /// reads on the MPU-9250 return stale/garbage data even though isolated reads work.
    pub fn transfer(&self, tx: &[u8], rx: &mut [u8]) -> Result<(), BusError> {
        self.select();
        spin(2_000); // ~CS-to-clock setup
        let r = self.transfer_held(tx, rx);
        self.deselect();
        spin(2_000); // ~CS recovery before the next transaction
        r
    }

    /// MPU-9250 register read: clock `0x80 | reg` then a dummy byte; the 2nd RX byte is
    /// the value.
    pub fn read_reg(&self, reg_addr: u8) -> Result<u8, BusError> {
        let mut rx = [0u8; 2];
        self.transfer(&[0x80 | reg_addr, 0x00], &mut rx)?;
        Ok(rx[1])
    }

    /// MPU-9250 register write.
    pub fn write_reg(&self, reg_addr: u8, val: u8) -> Result<(), BusError> {
        let mut rx = [0u8; 2];
        self.transfer(&[reg_addr & 0x7F, val], &mut rx)
    }

    /// Burst read `buf.len()` consecutive registers starting at `reg_addr`.
    pub fn read_burst(&self, reg_addr: u8, buf: &mut [u8]) -> Result<(), BusError> {
        let n = buf.len();
        if n == 0 || n + 1 > SPIM_XFER_MAX {
            return Err(BusError::Timeout);
        }
        let mut tx = [0u8; SPIM_XFER_MAX];
        let mut rx = [0u8; SPIM_XFER_MAX];
        tx[0] = 0x80 | reg_addr;
        self.transfer(&tx[..n + 1], &mut rx[..n + 1])?;
        buf.copy_from_slice(&rx[1..n + 1]);
        Ok(())
    }
}
