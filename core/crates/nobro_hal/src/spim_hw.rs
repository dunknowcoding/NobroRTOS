//! SPIM0 master for the nRF52840 (EasyDMA), for SPI sensors such as a GY-9250
//! (MPU-9250) wired for SPI. SPI mode 3, MSB-first, ~1 Mbps. CS is driven manually as
//! a GPIO output (idle high) so a whole register burst stays under one chip-select.

use core::future::Future;
use core::marker::PhantomPinned;
use core::pin::Pin;
use core::task::{Context, Poll};

use cortex_m::peripheral::NVIC;

use crate::bus::BusError;
use crate::completion::{CompletionCell, CompletionError};
use crate::lease::{LeaseError, LeaseGuard, Resource, ResourceLease};

const SPIM0_BASE: u32 = 0x4000_3000; // shared peripheral block with TWIM0
const GPIO_PORT0_BASE: u32 = 0x5000_0000;
const GPIO_PORT_STRIDE: u32 = 0x300;
const GPIO_PIN_CNF0: u32 = 0x700;
const GPIO_OUTSET: u32 = 0x508;
const GPIO_OUTCLR: u32 = 0x50C;

const SPIM_TASKS_START: u32 = 0x010;
const SPIM_TASKS_STOP: u32 = 0x014;
const SPIM_EVENTS_STOPPED: u32 = 0x104;
const SPIM_EVENTS_END: u32 = 0x118;
const SPIM_INTENSET: u32 = 0x304;
const SPIM_INTENCLR: u32 = 0x308;
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
const SPIM_INT_END: u32 = 1 << 6;
const TIMEOUT_SPINS: u32 = 200_000;
#[cfg(feature = "board-nicenano-s140")]
const SPIM0_PRIORITY_RAW: u8 = 6 << 5;
#[cfg(not(feature = "board-nicenano-s140"))]
const SPIM0_PRIORITY_RAW: u8 = 3 << 5;

static SPIM0_COMPLETION: CompletionCell = CompletionCell::new();

/// Max bytes per single EasyDMA transfer here (one register burst). 64 covers the
/// MPU-9250's 14-byte accel+temp+gyro burst with headroom.
pub const SPIM_XFER_MAX: usize = 64;

fn reg(base: u32, off: u32) -> *mut u32 {
    (base + off) as *mut u32
}

fn gpio(pin: u32) -> (u32, u32) {
    (GPIO_PORT0_BASE + (pin >> 5) * GPIO_PORT_STRIDE, pin & 0x1F)
}

fn spin(cycles: u32) {
    for _ in 0..cycles {
        cortex_m::asm::nop();
    }
}

/// SPIM0 master with a software-driven chip-select.
pub struct Spim0 {
    cs: u8,
    lease: LeaseGuard,
}

impl Spim0 {
    /// Configure SPIM0 on the given raw nRF pin numbers (mode 3, 1 Mbps). CS is set up
    /// as a GPIO output idling high. Caller must own the `Spim0` lease.
    ///
    /// # Safety
    /// Pins must be the board's wired SPI pins and not muxed to another peripheral;
    /// reprograms SPIM0's PSEL/ENABLE and the CS pin's GPIO config.
    pub unsafe fn acquire(
        owner: u8,
        sck: u8,
        mosi: u8,
        miso: u8,
        cs: u8,
    ) -> Result<Self, LeaseError> {
        let lease = ResourceLease::acquire_guard(Resource::Spim0, owner)?;
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

        Ok(Spim0 { cs, lease })
    }

    /// Assert chip-select (active low).
    pub fn select(&self) -> Result<(), BusError> {
        self.ensure_live()?;
        self.ensure_idle()?;
        let (port, bit) = gpio(u32::from(self.cs));
        unsafe {
            *reg(port, GPIO_OUTCLR) = 1 << bit;
        }
        Ok(())
    }

    /// Release chip-select.
    pub fn deselect(&self) -> Result<(), BusError> {
        self.ensure_live()?;
        self.ensure_idle()?;
        let (port, bit) = gpio(u32::from(self.cs));
        unsafe {
            *reg(port, GPIO_OUTSET) = 1 << bit;
        }
        Ok(())
    }

    /// One full-duplex EasyDMA transfer of equally sized buffers, WITHOUT
    /// touching chip-select (caller brackets with select()/deselect()). EasyDMA needs
    /// RAM buffers, so the bytes are staged through stack buffers.
    pub fn transfer_held(&self, tx: &[u8], rx: &mut [u8]) -> Result<(), BusError> {
        self.ensure_live()?;
        self.ensure_idle()?;
        if tx.len() != rx.len() {
            return Err(BusError::LengthMismatch);
        }
        let n = tx.len();
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
        self.ensure_idle()?;
        self.select()?;
        spin(2_000); // ~CS-to-clock setup
        let r = self.transfer_held(tx, rx);
        self.deselect()?;
        spin(2_000); // ~CS recovery before the next transaction
        r
    }

    /// Start one interrupt-driven EasyDMA transaction.
    ///
    /// Awaiting this future parks the reactor task; SPIM0's END interrupt wakes
    /// it through the ordinary task waker. Dropping the future stops and
    /// disables DMA before its pinned staging buffers can leave scope, making
    /// deadline timeout and selection cancellation safe.
    pub fn transfer_async<'a>(&'a self, tx: &'a [u8], rx: &'a mut [u8]) -> SpimTransfer<'a> {
        SpimTransfer::new(self, tx, rx)
    }

    /// Read one register without polling the peripheral.
    pub async fn read_reg_async(&self, reg_addr: u8) -> Result<u8, BusError> {
        let tx = [0x80 | reg_addr, 0];
        let mut rx = [0; 2];
        self.transfer_async(&tx, &mut rx).await?;
        Ok(rx[1])
    }

    /// Write one register without polling the peripheral.
    pub async fn write_reg_async(&self, reg_addr: u8, value: u8) -> Result<(), BusError> {
        let tx = [reg_addr & 0x7f, value];
        let mut rx = [0; 2];
        self.transfer_async(&tx, &mut rx).await
    }

    /// Read consecutive registers without polling the peripheral.
    pub async fn read_burst_async(&self, reg_addr: u8, output: &mut [u8]) -> Result<(), BusError> {
        let len = output.len();
        if len == 0 || len + 1 > SPIM_XFER_MAX {
            return Err(BusError::Timeout);
        }
        let mut tx = [0; SPIM_XFER_MAX];
        let mut rx = [0; SPIM_XFER_MAX];
        tx[0] = 0x80 | reg_addr;
        self.transfer_async(&tx[..len + 1], &mut rx[..len + 1])
            .await?;
        output.copy_from_slice(&rx[1..len + 1]);
        Ok(())
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

    fn ensure_live(&self) -> Result<(), BusError> {
        self.lease.ensure_live().map_err(|_| BusError::LeaseDenied)
    }

    fn ensure_idle(&self) -> Result<(), BusError> {
        if SPIM0_COMPLETION.is_busy() {
            Err(BusError::Busy)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransferState {
    New,
    InFlight,
    Done,
}

/// Cancellation-safe future returned by [`Spim0::transfer_async`].
pub struct SpimTransfer<'a> {
    spi: &'a Spim0,
    tx: &'a [u8],
    rx: &'a mut [u8],
    tx_dma: [u8; SPIM_XFER_MAX],
    rx_dma: [u8; SPIM_XFER_MAX],
    len: usize,
    state: TransferState,
    _pinned: PhantomPinned,
}

impl<'a> SpimTransfer<'a> {
    fn new(spi: &'a Spim0, tx: &'a [u8], rx: &'a mut [u8]) -> Self {
        Self {
            spi,
            tx,
            rx,
            tx_dma: [0; SPIM_XFER_MAX],
            rx_dma: [0; SPIM_XFER_MAX],
            len: tx.len(),
            state: TransferState::New,
            _pinned: PhantomPinned,
        }
    }

    fn validate(&self) -> Result<(), BusError> {
        self.spi.ensure_live()?;
        if self.tx.len() != self.rx.len() {
            return Err(BusError::LengthMismatch);
        }
        if self.len == 0 || self.len > SPIM_XFER_MAX {
            return Err(BusError::Timeout);
        }
        Ok(())
    }

    unsafe fn start(&mut self, cx: &Context<'_>) -> Result<(), BusError> {
        self.validate()?;
        self.tx_dma[..self.len].copy_from_slice(self.tx);
        self.spi.select()?;
        spin(2_000);
        if let Err(CompletionError::Busy) = SPIM0_COMPLETION.arm(cx.waker()) {
            let _ = self.spi.deselect();
            spin(2_000);
            return Err(BusError::Busy);
        }

        let base = SPIM0_BASE;
        *reg(base, SPIM_INTENCLR) = SPIM_INT_END;
        *reg(base, SPIM_EVENTS_END) = 0;
        *reg(base, SPIM_EVENTS_STOPPED) = 0;
        *reg(base, SPIM_TXD_PTR) = self.tx_dma.as_ptr() as u32;
        *reg(base, SPIM_TXD_MAXCNT) = self.len as u32;
        *reg(base, SPIM_RXD_PTR) = self.rx_dma.as_mut_ptr() as u32;
        *reg(base, SPIM_RXD_MAXCNT) = self.len as u32;

        // Completion IRQs deliberately run below the process-wide BASEPRI
        // ceiling: CompletionCell stores a Waker under critical-section.
        let mut core = cortex_m::Peripherals::steal();
        core.NVIC.set_priority(
            nrf52840_pac::Interrupt::SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0,
            SPIM0_PRIORITY_RAW,
        );
        NVIC::unpend(nrf52840_pac::Interrupt::SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0);
        NVIC::unmask(nrf52840_pac::Interrupt::SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0);
        *reg(base, SPIM_INTENSET) = SPIM_INT_END;
        cortex_m::asm::dsb();
        *reg(base, SPIM_TASKS_START) = 1;
        self.state = TransferState::InFlight;
        Ok(())
    }

    unsafe fn stop_dma(&self) {
        let base = SPIM0_BASE;
        *reg(base, SPIM_INTENCLR) = SPIM_INT_END;
        *reg(base, SPIM_EVENTS_STOPPED) = 0;
        *reg(base, SPIM_TASKS_STOP) = 1;
        for _ in 0..TIMEOUT_SPINS {
            if *reg(base, SPIM_EVENTS_STOPPED) != 0 {
                break;
            }
            cortex_m::asm::nop();
        }
        // Disabling the peripheral is the final ownership barrier if STOPPED
        // was not observed. No DMA may retain a pointer into this future.
        *reg(base, SPIM_ENABLE) = SPIM_ENABLE_DISABLED;
        cortex_m::asm::dsb();
        *reg(base, SPIM_ENABLE) = SPIM_ENABLE_ENABLED;
    }

    unsafe fn finish(&mut self) -> Result<(), BusError> {
        *reg(SPIM0_BASE, SPIM_INTENCLR) = SPIM_INT_END;
        cortex_m::asm::dsb();
        self.rx[..self.len].copy_from_slice(&self.rx_dma[..self.len]);
        // poll_complete consumed the global completion state, so the owning
        // future may now release chip-select through the ordinary guarded API.
        self.spi.deselect()?;
        spin(2_000);
        self.state = TransferState::Done;
        Ok(())
    }
}

impl Future for SpimTransfer<'_> {
    type Output = Result<(), BusError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: the future is !Unpin; its EasyDMA staging arrays remain at
        // stable addresses from `start` until completion or the Drop barrier.
        let this = unsafe { self.get_unchecked_mut() };
        match this.state {
            TransferState::New => {
                if let Err(error) = unsafe { this.start(cx) } {
                    this.state = TransferState::Done;
                    return Poll::Ready(Err(error));
                }
                Poll::Pending
            }
            TransferState::InFlight => {
                if !SPIM0_COMPLETION.poll_complete(cx) {
                    return Poll::Pending;
                }
                Poll::Ready(unsafe { this.finish() })
            }
            TransferState::Done => panic!("SpimTransfer polled after completion"),
        }
    }
}

impl Drop for SpimTransfer<'_> {
    fn drop(&mut self) {
        if self.state != TransferState::InFlight {
            return;
        }
        SPIM0_COMPLETION.cancel();
        unsafe {
            self.stop_dma();
        }
        let _ = self.spi.deselect();
        self.state = TransferState::Done;
    }
}

/// Shared SPIM0/TWIM0 vector. The asynchronous SPIM path owns this vector only
/// while the SPIM0 lease and its future are active; the legacy TWIM0 path is
/// polling-only.
#[no_mangle]
#[allow(non_snake_case)]
#[cfg(target_arch = "arm")]
unsafe extern "C" fn SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0() {
    let base = SPIM0_BASE;
    if *reg(base, SPIM_EVENTS_END) == 0 {
        return;
    }
    *reg(base, SPIM_INTENCLR) = SPIM_INT_END;
    *reg(base, SPIM_EVENTS_END) = 0;
    cortex_m::asm::dsb();
    SPIM0_COMPLETION.complete_from_isr();
}
