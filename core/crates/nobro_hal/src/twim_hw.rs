//! TWI0 compatibility path plus opt-in cancellation-safe TWIM0 EasyDMA futures.

use crate::bus::BusError;

const TWI0_BASE: u32 = 0x4000_3000;
const GPIO_PORT0_BASE: u32 = 0x5000_0000;
const GPIO_PORT_STRIDE: u32 = 0x300;
const GPIO_OUTSET: u32 = 0x508;
const GPIO_OUTCLR: u32 = 0x50c;
const GPIO_IN: u32 = 0x510;
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

fn gpio(raw_pin: u32) -> (u32, u32) {
    (
        GPIO_PORT0_BASE + (raw_pin >> 5) * GPIO_PORT_STRIDE,
        raw_pin & 0x1f,
    )
}

fn configure_open_drain(raw_pin: u32, output: bool) {
    let (base, pin) = gpio(raw_pin);
    unsafe {
        // PULL=up, DRIVE=S0D1 (open drain), required for slave ACK.
        *reg(base, GPIO_PIN_CNF0 + pin * 4) = u32::from(output) | (3 << 2) | (6 << 8);
    }
}

fn recover_bus(sda: u8, scl: u8) {
    let (sda_port, sda_bit) = gpio(u32::from(sda));
    let (scl_port, scl_bit) = gpio(u32::from(scl));
    configure_open_drain(u32::from(sda), false);
    configure_open_drain(u32::from(scl), false);
    unsafe {
        if *reg(sda_port, GPIO_IN) & (1 << sda_bit) != 0
            && *reg(scl_port, GPIO_IN) & (1 << scl_bit) != 0
        {
            return;
        }
        // Set the output latch before changing DIR, so releasing the
        // open-drain clock cannot produce a low glitch.
        *reg(scl_port, GPIO_OUTSET) = 1 << scl_bit;
    }
    configure_open_drain(u32::from(scl), true);

    // Clock out a stuck slave (9 pulses) if SDA is low.
    for _ in 0..9 {
        unsafe {
            *reg(scl_port, GPIO_OUTCLR) = 1 << scl_bit;
            for _ in 0..100 {
                cortex_m::asm::nop();
            }
            *reg(scl_port, GPIO_OUTSET) = 1 << scl_bit;
            for _ in 0..100 {
                cortex_m::asm::nop();
            }
            if *reg(sda_port, GPIO_IN) & (1 << sda_bit) != 0 {
                break;
            }
        }
    }

    // Generate a STOP (SDA low-to-high while SCL is released), then return
    // both pins to peripheral-owned input/open-drain configuration.
    unsafe {
        *reg(sda_port, GPIO_OUTCLR) = 1 << sda_bit;
    }
    configure_open_drain(u32::from(sda), true);
    for _ in 0..100 {
        cortex_m::asm::nop();
    }
    unsafe {
        *reg(scl_port, GPIO_OUTSET) = 1 << scl_bit;
    }
    for _ in 0..100 {
        cortex_m::asm::nop();
    }
    unsafe {
        *reg(sda_port, GPIO_OUTSET) = 1 << sda_bit;
    }
    for _ in 0..100 {
        cortex_m::asm::nop();
    }
    configure_open_drain(u32::from(sda), false);
    configure_open_drain(u32::from(scl), false);
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
        configure_open_drain(u32::from(sda), false);
        configure_open_drain(u32::from(scl), false);
    }

    /// # Safety
    /// Caller must hold a live initialized TWIM0 lease for the whole transaction.
    pub unsafe fn probe(addr: u8) -> bool {
        Self::write(addr, &[], true).is_ok()
    }

    /// # Safety
    /// Caller must hold a live initialized TWIM0 lease for the whole scan.
    pub unsafe fn scan<F: FnMut(u8)>(mut found: F) -> u8 {
        let mut count = 0u8;
        for addr in [0x68u8, 0x69] {
            if unsafe { Self::read_reg(addr, 0x75) }.is_ok() {
                found(addr);
                count = count.saturating_add(1);
            }
        }
        for addr in [0x76u8, 0x77] {
            if unsafe { Self::read_reg(addr, 0xD0) }.is_ok() {
                found(addr);
                count = count.saturating_add(1);
            }
        }
        count
    }

    /// # Safety
    /// Caller must hold a live initialized TWIM0 lease for the whole transaction.
    pub unsafe fn write_reg(addr: u8, reg_addr: u8, val: u8) -> Result<(), BusError> {
        Self::write(addr, &[reg_addr, val], true)
    }

    /// # Safety
    /// Caller must hold a live initialized TWIM0 lease for the whole transaction.
    pub unsafe fn read_reg(addr: u8, reg_addr: u8) -> Result<u8, BusError> {
        let mut buf = [0u8; 1];
        unsafe { Self::write_read(addr, &[reg_addr], &mut buf)? };
        Ok(buf[0])
    }

    /// # Safety
    /// Caller must hold a live initialized TWIM0 lease for both transaction phases.
    pub unsafe fn write_read(addr: u8, tx: &[u8], rx: &mut [u8]) -> Result<(), BusError> {
        if tx.is_empty() || rx.is_empty() {
            return Err(BusError::EmptyTransfer);
        }
        // Stop-start, not repeated-start, matches common MPU9250 bring-up.
        Self::write(addr, tx, true)?;
        Self::read(addr, rx, true)
    }

    /// Raw bus write of arbitrary bytes (STOP at the end). The general primitive an
    /// `embedded-hal` I2C adapter needs for `Operation::Write`.
    /// # Safety
    /// Caller must hold a live initialized TWIM0 lease for the whole transaction.
    pub unsafe fn write_bytes(addr: u8, data: &[u8]) -> Result<(), BusError> {
        Self::write(addr, data, true)
    }

    /// Raw bus read of `buf.len()` bytes (STOP at the end). The general primitive an
    /// `embedded-hal` I2C adapter needs for `Operation::Read`.
    /// # Safety
    /// Caller must hold a live initialized TWIM0 lease for the whole transaction.
    pub unsafe fn read_bytes(addr: u8, buf: &mut [u8]) -> Result<(), BusError> {
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

#[cfg(feature = "nrf-twim-async")]
mod async_provider {
    use core::future::Future;
    use core::marker::PhantomPinned;
    use core::pin::Pin;
    use core::sync::atomic::{AtomicU32, Ordering};
    use core::task::{Context, Poll};

    use cortex_m::peripheral::NVIC;

    use super::*;
    use crate::bus::TwimBus;
    use crate::completion::{CompletionCell, CompletionError};

    const TWIM_ENABLE_ENABLED: u32 = 6;
    const TWIM_EVENTS_LASTRX: u32 = 0x15c;
    const TWIM_EVENTS_LASTTX: u32 = 0x160;
    const TWIM_INTENSET: u32 = 0x304;
    const TWIM_INTENCLR: u32 = 0x308;
    const TWIM_RXD_PTR: u32 = 0x534;
    const TWIM_RXD_MAXCNT: u32 = 0x538;
    const TWIM_RXD_AMOUNT: u32 = 0x53c;
    const TWIM_TXD_PTR: u32 = 0x544;
    const TWIM_TXD_MAXCNT: u32 = 0x548;
    const TWIM_TXD_AMOUNT: u32 = 0x54c;
    const TWIM_SHORT_LASTTX_STARTRX: u32 = 1 << 7;
    const TWIM_SHORT_LASTTX_STOP: u32 = 1 << 9;
    const TWIM_SHORT_LASTRX_STOP: u32 = 1 << 12;
    const TWIM_INT_STOPPED: u32 = 1 << 1;
    const TWIM_INT_ERROR: u32 = 1 << 9;
    const TWIM_INTERRUPT_MASK: u32 = TWIM_INT_STOPPED | TWIM_INT_ERROR;

    /// Bounded staging capacity for one EasyDMA I2C phase.
    pub const TWIM_XFER_MAX: usize = 64;

    static TWIM0_COMPLETION: CompletionCell = CompletionCell::new();
    static TWIM0_ERROR: AtomicU32 = AtomicU32::new(0);

    pub(crate) fn async_busy() -> bool {
        TWIM0_COMPLETION.is_busy()
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum TransferMode {
        Write,
        Read,
        WriteRead,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct TwimTransferSpec {
        mode: TransferMode,
        tx_len: usize,
        rx_len: usize,
    }

    impl TwimTransferSpec {
        fn new(address: u8, tx_len: usize, rx_len: usize) -> Result<Self, BusError> {
            if address > 0x7f {
                return Err(BusError::InvalidAddress);
            }
            if tx_len > TWIM_XFER_MAX || rx_len > TWIM_XFER_MAX {
                return Err(BusError::TransferTooLong);
            }
            let mode = match (tx_len, rx_len) {
                (0, 0) => return Err(BusError::EmptyTransfer),
                (0, _) => TransferMode::Read,
                (_, 0) => TransferMode::Write,
                (_, _) => TransferMode::WriteRead,
            };
            Ok(Self {
                mode,
                tx_len,
                rx_len,
            })
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum TransferState {
        New,
        InFlight,
        Done,
    }

    /// Cancellation-safe TWIM0 EasyDMA transaction.
    ///
    /// The future owns fixed RAM staging arrays because nRF EasyDMA cannot read
    /// arbitrary flash-backed slices. Dropping an in-flight future disables the
    /// shared interrupt, stops and disables TWIM0, and only then lets those arrays
    /// leave scope. The legacy synchronous TWI mode is restored on every exit.
    pub struct TwimTransfer<'a> {
        bus: &'a TwimBus,
        address: u8,
        tx: &'a [u8],
        rx: Option<&'a mut [u8]>,
        tx_dma: [u8; TWIM_XFER_MAX],
        rx_dma: [u8; TWIM_XFER_MAX],
        spec: Result<TwimTransferSpec, BusError>,
        state: TransferState,
        _pinned: PhantomPinned,
    }

    impl<'a> TwimTransfer<'a> {
        fn new(bus: &'a TwimBus, address: u8, tx: &'a [u8], rx: Option<&'a mut [u8]>) -> Self {
            let rx_len = rx.as_ref().map_or(0, |buffer| buffer.len());
            Self {
                bus,
                address,
                tx,
                rx,
                tx_dma: [0; TWIM_XFER_MAX],
                rx_dma: [0; TWIM_XFER_MAX],
                spec: TwimTransferSpec::new(address, tx.len(), rx_len),
                state: TransferState::New,
                _pinned: PhantomPinned,
            }
        }

        unsafe fn start(&mut self, cx: &Context<'_>) -> Result<(), BusError> {
            self.bus.ensure_live()?;
            let spec = self.spec?;
            self.tx_dma[..spec.tx_len].copy_from_slice(self.tx);
            if let Err(CompletionError::Busy) = TWIM0_COMPLETION.arm(cx.waker()) {
                return Err(BusError::Busy);
            }

            let base = TWI0_BASE;
            TWIM0_ERROR.store(0, Ordering::Release);
            *reg(base, TWIM_INTENCLR) = 0xffff_ffff;
            *reg(base, TWI_ENABLE) = TWI_ENABLE_DISABLED;
            *reg(base, TWI_SHORTS) = 0;
            clear_event(base, TWI_EVENTS_STOPPED);
            clear_event(base, TWI_EVENTS_ERROR);
            clear_event(base, TWIM_EVENTS_LASTRX);
            clear_event(base, TWIM_EVENTS_LASTTX);
            *reg(base, TWI_ERRORSRC) = 0x7;
            *reg(base, TWI_ADDRESS) = u32::from(self.address);
            *reg(base, TWIM_TXD_PTR) = self.tx_dma.as_ptr() as u32;
            *reg(base, TWIM_TXD_MAXCNT) = spec.tx_len as u32;
            *reg(base, TWIM_RXD_PTR) = self.rx_dma.as_mut_ptr() as u32;
            *reg(base, TWIM_RXD_MAXCNT) = spec.rx_len as u32;
            *reg(base, TWI_ENABLE) = TWIM_ENABLE_ENABLED;

            let mut core = cortex_m::Peripherals::steal();
            core.NVIC.set_priority(
                nrf52840_pac::Interrupt::SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0,
                self.bus.interrupt_priority().raw(),
            );
            NVIC::unpend(nrf52840_pac::Interrupt::SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0);
            NVIC::unmask(nrf52840_pac::Interrupt::SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0);
            *reg(base, TWIM_INTENSET) = TWIM_INTERRUPT_MASK;
            *reg(base, TWI_SHORTS) = match spec.mode {
                TransferMode::Write => TWIM_SHORT_LASTTX_STOP,
                TransferMode::Read => TWIM_SHORT_LASTRX_STOP,
                TransferMode::WriteRead => TWIM_SHORT_LASTTX_STARTRX | TWIM_SHORT_LASTRX_STOP,
            };
            self.state = TransferState::InFlight;
            cortex_m::asm::dsb();
            *reg(
                base,
                if spec.mode == TransferMode::Read {
                    TWI_TASKS_STARTRX
                } else {
                    TWI_TASKS_STARTTX
                },
            ) = 1;
            Ok(())
        }

        unsafe fn restore_legacy_mode() {
            let base = TWI0_BASE;
            *reg(base, TWIM_INTENCLR) = 0xffff_ffff;
            *reg(base, TWI_SHORTS) = 0;
            *reg(base, TWI_ENABLE) = TWI_ENABLE_DISABLED;
            cortex_m::asm::dsb();
            *reg(base, TWI_ENABLE) = TWI_ENABLE_ENABLED;
            clear_event(base, TWI_EVENTS_STOPPED);
            clear_event(base, TWI_EVENTS_ERROR);
            *reg(base, TWI_ERRORSRC) = 0x7;
        }

        unsafe fn stop_dma() {
            let base = TWI0_BASE;
            *reg(base, TWIM_INTENCLR) = 0xffff_ffff;
            clear_event(base, TWI_EVENTS_STOPPED);
            *reg(base, TWI_TASKS_STOP) = 1;
            for _ in 0..TIMEOUT_SPINS {
                if *reg(base, TWI_EVENTS_STOPPED) != 0 {
                    break;
                }
                cortex_m::asm::nop();
            }
            Self::restore_legacy_mode();
        }

        unsafe fn finish(&mut self) -> Result<(), BusError> {
            let spec = self.spec?;
            let errors = TWIM0_ERROR.swap(0, Ordering::AcqRel);
            let tx_amount = *reg(TWI0_BASE, TWIM_TXD_AMOUNT) as usize;
            let rx_amount = *reg(TWI0_BASE, TWIM_RXD_AMOUNT) as usize;
            let result = if errors & 0x6 != 0 {
                Err(BusError::Nack)
            } else if errors != 0 || tx_amount != spec.tx_len || rx_amount != spec.rx_len {
                Err(BusError::Timeout)
            } else {
                if let Some(output) = self.rx.as_deref_mut() {
                    output.copy_from_slice(&self.rx_dma[..spec.rx_len]);
                }
                Ok(())
            };
            Self::restore_legacy_mode();
            self.state = TransferState::Done;
            result
        }
    }

    impl Future for TwimTransfer<'_> {
        type Output = Result<(), BusError>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            // SAFETY: the future is !Unpin and cancellation stops EasyDMA before
            // its staging arrays may move or leave scope.
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
                    if !TWIM0_COMPLETION.poll_complete(cx) {
                        return Poll::Pending;
                    }
                    Poll::Ready(unsafe { this.finish() })
                }
                TransferState::Done => panic!("TwimTransfer polled after completion"),
            }
        }
    }

    impl Drop for TwimTransfer<'_> {
        fn drop(&mut self) {
            if self.state != TransferState::InFlight {
                return;
            }
            unsafe {
                *reg(TWI0_BASE, TWIM_INTENCLR) = 0xffff_ffff;
                cortex_m::asm::dsb();
            }
            TWIM0_COMPLETION.cancel();
            unsafe {
                Self::stop_dma();
            }
            self.state = TransferState::Done;
        }
    }

    impl TwimBus {
        pub fn write_async<'a>(&'a self, address: u8, bytes: &'a [u8]) -> TwimTransfer<'a> {
            TwimTransfer::new(self, address, bytes, None)
        }

        pub fn read_async<'a>(&'a self, address: u8, bytes: &'a mut [u8]) -> TwimTransfer<'a> {
            TwimTransfer::new(self, address, &[], Some(bytes))
        }

        pub fn write_read_async<'a>(
            &'a self,
            address: u8,
            write: &'a [u8],
            read: &'a mut [u8],
        ) -> TwimTransfer<'a> {
            TwimTransfer::new(self, address, write, Some(read))
        }

        pub async fn read_reg_async(&self, address: u8, register: u8) -> Result<u8, BusError> {
            let write = [register];
            let mut read = [0];
            self.write_read_async(address, &write, &mut read).await?;
            Ok(read[0])
        }

        pub async fn write_reg_async(
            &self,
            address: u8,
            register: u8,
            value: u8,
        ) -> Result<(), BusError> {
            self.write_async(address, &[register, value]).await
        }

        pub async fn read_burst_async(
            &self,
            address: u8,
            register: u8,
            output: &mut [u8],
        ) -> Result<(), BusError> {
            self.write_read_async(address, &[register], output).await
        }
    }

    /// Shared SPIM0/TWIM0 vector half for the EasyDMA I2C path.
    #[cfg(target_arch = "arm")]
    pub(crate) unsafe fn on_interrupt() {
        if !TWIM0_COMPLETION.is_busy() {
            return;
        }
        let base = TWI0_BASE;
        if *reg(base, TWI_EVENTS_ERROR) != 0 {
            let errors = *reg(base, TWI_ERRORSRC);
            TWIM0_ERROR.fetch_or(errors, Ordering::AcqRel);
            *reg(base, TWI_ERRORSRC) = errors;
            clear_event(base, TWI_EVENTS_ERROR);
            *reg(base, TWI_TASKS_STOP) = 1;
        }
        if *reg(base, TWI_EVENTS_STOPPED) == 0 {
            return;
        }
        *reg(base, TWIM_INTENCLR) = TWIM_INTERRUPT_MASK;
        clear_event(base, TWI_EVENTS_STOPPED);
        cortex_m::asm::dsb();
        TWIM0_COMPLETION.complete_from_isr();
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn transfer_spec_rejects_unrepresentable_requests() {
            assert_eq!(
                TwimTransferSpec::new(0x80, 1, 1),
                Err(BusError::InvalidAddress)
            );
            assert_eq!(
                TwimTransferSpec::new(0x68, 0, 0),
                Err(BusError::EmptyTransfer)
            );
            assert_eq!(
                TwimTransferSpec::new(0x68, TWIM_XFER_MAX + 1, 0),
                Err(BusError::TransferTooLong)
            );
            assert_eq!(
                TwimTransferSpec::new(0x68, 0, TWIM_XFER_MAX + 1),
                Err(BusError::TransferTooLong)
            );
        }

        #[test]
        fn transfer_spec_selects_bounded_dma_sequence() {
            assert_eq!(
                TwimTransferSpec::new(0x68, 1, 0).unwrap().mode,
                TransferMode::Write
            );
            assert_eq!(
                TwimTransferSpec::new(0x68, 0, 1).unwrap().mode,
                TransferMode::Read
            );
            assert_eq!(
                TwimTransferSpec::new(0x68, 1, 14).unwrap().mode,
                TransferMode::WriteRead
            );
        }
    }
}

#[cfg(feature = "nrf-twim-async")]
pub(crate) use async_provider::async_busy;
#[cfg(all(feature = "nrf-twim-async", target_arch = "arm"))]
pub(crate) use async_provider::on_interrupt;
#[cfg(feature = "nrf-twim-async")]
pub use async_provider::{TwimTransfer, TWIM_XFER_MAX};

#[cfg(not(feature = "nrf-twim-async"))]
pub(crate) const fn async_busy() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpio_addressing_keeps_port_and_bit_independent() {
        assert_eq!(gpio(0), (GPIO_PORT0_BASE, 0));
        assert_eq!(gpio(31), (GPIO_PORT0_BASE, 31));
        assert_eq!(gpio(32), (GPIO_PORT0_BASE + GPIO_PORT_STRIDE, 0));
        assert_eq!(gpio(47), (GPIO_PORT0_BASE + GPIO_PORT_STRIDE, 15));
        assert_eq!(GPIO_OUTCLR, 0x50c);
        assert_eq!(GPIO_IN, 0x510);
    }
}
