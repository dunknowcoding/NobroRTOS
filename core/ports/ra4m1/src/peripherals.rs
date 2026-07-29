//! Native RA4M1 peripheral providers for the UNO R4 WiFi composition.
//!
//! Pin identities and register order follow the installed Arduino Renesas core
//! and its FSP definitions. Operations are synchronous but bounded; a stopped
//! peripheral, NACK, or malformed request returns a typed error.

use nobro_hal::{
    HalAdcChannel, HalByteIo, HalI2c, HalPwmChannel, HalSpi, LeaseClass, LeaseError, LeaseId,
    TransferMode,
};

use crate::lease::{Ra4m1LeaseGuard, Ra4m1Leases};

#[cfg(target_arch = "arm")]
const DEFAULT_POLLS: u32 = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeripheralError {
    InvalidConfig,
    LengthMismatch,
    Lease(LeaseError),
    Timeout,
    Nack,
    ArbitrationLost,
    Overrun,
    NotReady,
    UnsupportedOnHost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PollBudget {
    remaining: u32,
}

impl PollBudget {
    const fn new(polls: u32) -> Self {
        Self { remaining: polls }
    }

    fn step(&mut self) -> Result<(), PeripheralError> {
        if self.remaining == 0 {
            Err(PeripheralError::Timeout)
        } else {
            self.remaining -= 1;
            Ok(())
        }
    }
}

pub struct Ra4m1Adc {
    _lease: Ra4m1LeaseGuard,
    channel: u8,
}

impl Ra4m1Adc {
    /// UNO R4 A0: P014, ADC0 channel 9, 12-bit single scan.
    pub fn try_a0(owner: u8) -> Result<Self, PeripheralError> {
        let lease = Ra4m1Leases::acquire_guard(LeaseId::PRIMARY_ADC, owner)
            .map_err(PeripheralError::Lease)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            start_module(MSTPCRD, 1 << 16);
            configure_pfs(P014_PFS, PFS_ANALOG);
            write16(ADC0 + ADCSR, 0);
            write16(ADC0 + ADANSA0, 1 << 9);
            write16(ADC0 + ADCER, 0);
            write8(ADC0 + ADSSTR_BASE + 9, 20);
        }
        Ok(Self {
            _lease: lease,
            channel: 9,
        })
    }

    pub const fn channel(&self) -> u8 {
        self.channel
    }
}

impl HalAdcChannel for Ra4m1Adc {
    type Error = PeripheralError;

    fn max_sample(&self) -> u16 {
        4095
    }

    fn read(&mut self) -> Result<u16, Self::Error> {
        self._lease.ensure_live().map_err(PeripheralError::Lease)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            write16(ADC0 + ADCSR, read16(ADC0 + ADCSR) | ADCSR_ADST);
            let mut budget = PollBudget::new(DEFAULT_POLLS);
            while read16(ADC0 + ADCSR) & ADCSR_ADST != 0 {
                budget.step()?;
                core::hint::spin_loop();
            }
            return Ok(read16(ADC0 + ADDR_BASE + usize::from(self.channel) * 2) & 0x0fff);
        }
        #[cfg(not(target_arch = "arm"))]
        Err(PeripheralError::UnsupportedOnHost)
    }
}

pub struct Ra4m1Pwm {
    _lease: Ra4m1LeaseGuard,
    period_ticks: u32,
    duty: u16,
}

impl Ra4m1Pwm {
    /// UNO R4 D5: P107 / GPT0 output A.
    pub fn try_d5(owner: u8, frequency_hz: u32) -> Result<Self, PeripheralError> {
        if frequency_hz == 0 {
            return Err(PeripheralError::InvalidConfig);
        }
        let period_ticks = 48_000_000u32
            .checked_div(frequency_hz)
            .filter(|ticks| *ticks >= 2)
            .ok_or(PeripheralError::InvalidConfig)?;
        let lease = Ra4m1Leases::acquire_guard(LeaseId::PRIMARY_PWM, owner)
            .map_err(PeripheralError::Lease)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            start_module(MSTPCRD, 1 << 5);
            configure_pfs(P107_PFS, PFS_GPT);
            write32(GPT0 + GTSTP, 1);
            write32(GPT0 + GTWP, GPT_WRITE_UNLOCKED);
            write32(GPT0 + GTCR, 0);
            write32(GPT0 + GTUDDTYC, 3);
            write32(GPT0 + GTUDDTYC, 1);
            write32(GPT0 + GTIOR, GTIOR_PWM_A);
            write32(GPT0 + GTBER, 0);
            write32(GPT0 + GTPR, period_ticks - 1);
            write32(GPT0 + GTPBR, period_ticks - 1);
            write32(GPT0 + GTCCRA, 0);
            write32(GPT0 + GTCNT, 0);
            write32(GPT0 + GTCLR, 1);
            write32(GPT0 + GTWP, GPT_WRITE_PROTECTED);
            write32(GPT0 + GTSTR, 1);
        }
        Ok(Self {
            _lease: lease,
            period_ticks,
            duty: 0,
        })
    }

    pub const fn period_ticks(&self) -> u32 {
        self.period_ticks
    }
}

impl HalPwmChannel for Ra4m1Pwm {
    type Error = PeripheralError;

    fn max_duty(&self) -> u16 {
        u16::MAX
    }

    fn set_duty(&mut self, duty: u16) -> Result<(), Self::Error> {
        self._lease.ensure_live().map_err(PeripheralError::Lease)?;
        let _compare = (u64::from(self.period_ticks) * u64::from(duty) / u64::from(u16::MAX))
            .min(u64::from(self.period_ticks - 1)) as u32;
        #[cfg(target_arch = "arm")]
        unsafe {
            write32(GPT0 + GTCCRA, _compare);
        }
        self.duty = duty;
        Ok(())
    }
}

pub struct Ra4m1Spi {
    _lease: Ra4m1LeaseGuard,
}

impl Ra4m1Spi {
    /// UNO R4 header SPI0: D11/P411 MOSI, D12/P410 MISO, D13/P102 SCK.
    ///
    /// Slave-select remains application-owned GPIO so several logical devices
    /// can share this one physical bus without hidden chip-select policy.
    pub fn try_uno_header(owner: u8, frequency_hz: u32) -> Result<Self, PeripheralError> {
        let divisor = spi_divisor(frequency_hz)?;
        let lease = Ra4m1Leases::acquire_guard(LeaseId::PRIMARY_SPI, owner)
            .map_err(PeripheralError::Lease)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            start_module(MSTPCRB, 1 << 19);
            configure_pfs(P411_PFS, PFS_SPI);
            configure_pfs(P410_PFS, PFS_SPI);
            configure_pfs(P102_PFS, PFS_SPI);
            write8(SPI0 + SPCR, 0);
            write8(SPI0 + SSLP, 0);
            write8(SPI0 + SPPCR, 0);
            write8(SPI0 + SPSCR, 0);
            write8(SPI0 + SPBR, divisor);
            write8(SPI0 + SPDCR, 1 << 6);
            write8(SPI0 + SPCKD, 0);
            write8(SPI0 + SSLND, 0);
            write8(SPI0 + SPND, 0);
            write8(SPI0 + SPCR2, 1);
            write16(SPI0 + SPCMD0, (7 << 8) | (1 << 13) | (1 << 14) | (1 << 15));
            write8(SPI0 + SPSR, read8(SPI0 + SPSR) & !SPI_ERROR_MASK);
            write8(SPI0 + SPCR, SPCR_MSTR | SPCR_SPE);
        }
        let _ = divisor;
        Ok(Self { _lease: lease })
    }
}

impl HalSpi for Ra4m1Spi {
    type Error = PeripheralError;
    const TRANSFER_MODE: TransferMode = TransferMode::Polling;

    fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> Result<(), Self::Error> {
        self._lease.ensure_live().map_err(PeripheralError::Lease)?;
        if write.len() != read.len() {
            return Err(PeripheralError::LengthMismatch);
        }
        #[cfg(target_arch = "arm")]
        unsafe {
            let mut budget = PollBudget::new(DEFAULT_POLLS);
            for (&tx, rx) in write.iter().zip(read.iter_mut()) {
                while read8(SPI0 + SPSR) & SPSR_SPTEF == 0 {
                    spi_fault()?;
                    budget.step()?;
                }
                write8(SPI0 + SPDR, tx);
                while read8(SPI0 + SPSR) & SPSR_SPRF == 0 {
                    spi_fault()?;
                    budget.step()?;
                }
                *rx = read8(SPI0 + SPDR);
            }
            return spi_fault();
        }
        #[cfg(not(target_arch = "arm"))]
        Err(PeripheralError::UnsupportedOnHost)
    }
}

pub struct Ra4m1I2c {
    _lease: Ra4m1LeaseGuard,
}

impl Ra4m1I2c {
    /// UNO R4 Wire pins: A4/P101 SDA and A5/P100 SCL on IIC1.
    pub fn try_uno_wire(owner: u8) -> Result<Self, PeripheralError> {
        let lease = Ra4m1Leases::acquire_guard(LeaseId::SECONDARY_I2C, owner)
            .map_err(PeripheralError::Lease)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            start_module(MSTPCRB, 1 << 8);
            configure_pfs(P101_PFS, PFS_IIC);
            configure_pfs(P100_PFS, PFS_IIC);
            reset_iic1()?;
        }
        Ok(Self { _lease: lease })
    }

    fn transact(
        &mut self,
        address: u8,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<(), PeripheralError> {
        self._lease.ensure_live().map_err(PeripheralError::Lease)?;
        if address >= 0x80 || (write.is_empty() && read.is_empty()) {
            return Err(PeripheralError::InvalidConfig);
        }
        #[cfg(target_arch = "arm")]
        unsafe {
            let result = iic1_transaction(address, write, read);
            if result.is_err() {
                let _ = reset_iic1();
            }
            return result;
        }
        #[cfg(not(target_arch = "arm"))]
        Err(PeripheralError::UnsupportedOnHost)
    }
}

impl HalI2c for Ra4m1I2c {
    type Error = PeripheralError;
    const TRANSFER_MODE: TransferMode = TransferMode::Polling;

    fn write(&mut self, address: u8, bytes: &[u8]) -> Result<(), Self::Error> {
        self.transact(address, bytes, &mut [])
    }

    fn read(&mut self, address: u8, bytes: &mut [u8]) -> Result<(), Self::Error> {
        self.transact(address, &[], bytes)
    }

    fn write_read(
        &mut self,
        address: u8,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<(), Self::Error> {
        self.transact(address, write, read)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UartInstance {
    WifiController,
    Header,
    UsbBridge,
}

impl UartInstance {
    const fn channel(self) -> u8 {
        match self {
            Self::WifiController => 1,
            Self::Header => 2,
            Self::UsbBridge => 9,
        }
    }

    const fn base(self) -> usize {
        match self {
            Self::WifiController => 0x4007_0020,
            Self::Header => 0x4007_0040,
            Self::UsbBridge => 0x4007_0120,
        }
    }
}

pub struct Ra4m1Uart {
    _lease: Ra4m1LeaseGuard,
    instance: UartInstance,
}

impl Ra4m1Uart {
    pub fn try_new(owner: u8, instance: UartInstance) -> Result<Self, PeripheralError> {
        let id = LeaseId::new(LeaseClass::Uart, instance.channel());
        let lease = Ra4m1Leases::acquire_guard(id, owner).map_err(PeripheralError::Lease)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            start_module(MSTPCRB, 1 << (31 - instance.channel()));
            match instance {
                UartInstance::WifiController => {
                    configure_pfs(P501_PFS, PFS_SCI_ODD);
                    configure_pfs(P502_PFS, PFS_SCI_ODD);
                }
                UartInstance::Header => {
                    configure_pfs(P301_PFS, PFS_SCI_EVEN);
                    configure_pfs(P302_PFS, PFS_SCI_EVEN);
                }
                UartInstance::UsbBridge => {
                    configure_pfs(P109_PFS, PFS_SCI_ODD);
                    configure_pfs(P110_PFS, PFS_SCI_ODD);
                }
            }
            initialize_sci(instance.base());
        }
        Ok(Self {
            _lease: lease,
            instance,
        })
    }

    pub const fn instance(&self) -> UartInstance {
        self.instance
    }
}

impl HalByteIo for Ra4m1Uart {
    type Error = PeripheralError;

    fn read_available(&mut self, _bytes: &mut [u8]) -> Result<usize, Self::Error> {
        self._lease.ensure_live().map_err(PeripheralError::Lease)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            let base = self.instance.base();
            let mut count = 0;
            while count < _bytes.len() {
                let status = read8(base + SCI_SSR);
                if status & SCI_ERROR_MASK != 0 {
                    recover_sci(base);
                    return Err(PeripheralError::Overrun);
                }
                if status & SCI_RDRF == 0 {
                    break;
                }
                _bytes[count] = read8(base + SCI_RDR);
                count += 1;
            }
            return Ok(count);
        }
        #[cfg(not(target_arch = "arm"))]
        Err(PeripheralError::UnsupportedOnHost)
    }

    fn write_all(&mut self, _bytes: &[u8]) -> Result<(), Self::Error> {
        self._lease.ensure_live().map_err(PeripheralError::Lease)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            let base = self.instance.base();
            let mut budget = PollBudget::new(DEFAULT_POLLS);
            for &byte in _bytes {
                while read8(base + SCI_SSR) & SCI_TDRE == 0 {
                    budget.step()?;
                }
                write8(base + SCI_TDR, byte);
            }
            return Ok(());
        }
        #[cfg(not(target_arch = "arm"))]
        Err(PeripheralError::UnsupportedOnHost)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self._lease.ensure_live().map_err(PeripheralError::Lease)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            let mut budget = PollBudget::new(DEFAULT_POLLS);
            while read8(self.instance.base() + SCI_SSR) & SCI_TEND == 0 {
                budget.step()?;
            }
            return Ok(());
        }
        #[cfg(not(target_arch = "arm"))]
        Err(PeripheralError::UnsupportedOnHost)
    }
}

fn spi_divisor(frequency_hz: u32) -> Result<u8, PeripheralError> {
    if frequency_hz == 0 {
        return Err(PeripheralError::InvalidConfig);
    }
    let divider = 48_000_000u32
        .saturating_add(frequency_hz - 1)
        .checked_div(frequency_hz)
        .ok_or(PeripheralError::InvalidConfig)?;
    let half = divider.saturating_add(1) / 2;
    let spbr = half.saturating_sub(1);
    u8::try_from(spbr).map_err(|_| PeripheralError::InvalidConfig)
}

#[cfg(target_arch = "arm")]
const PRCR: usize = 0x4001_E3FE;
#[cfg(target_arch = "arm")]
const MSTPCRB: usize = 0x4004_7000;
#[cfg(target_arch = "arm")]
const MSTPCRD: usize = 0x4004_7008;
#[cfg(target_arch = "arm")]
const PWPR: usize = 0x4004_0d03;
#[cfg(target_arch = "arm")]
const P014_PFS: usize = 0x4004_0800 + 14 * 4;
#[cfg(target_arch = "arm")]
const P100_PFS: usize = 0x4004_0840;
#[cfg(target_arch = "arm")]
const P101_PFS: usize = 0x4004_0844;
#[cfg(target_arch = "arm")]
const P102_PFS: usize = 0x4004_0848;
#[cfg(target_arch = "arm")]
const P107_PFS: usize = 0x4004_085c;
#[cfg(target_arch = "arm")]
const P109_PFS: usize = 0x4004_0864;
#[cfg(target_arch = "arm")]
const P110_PFS: usize = 0x4004_0868;
#[cfg(target_arch = "arm")]
const P301_PFS: usize = 0x4004_08c4;
#[cfg(target_arch = "arm")]
const P302_PFS: usize = 0x4004_08c8;
#[cfg(target_arch = "arm")]
const P410_PFS: usize = 0x4004_0908;
#[cfg(target_arch = "arm")]
const P411_PFS: usize = 0x4004_090c;
#[cfg(target_arch = "arm")]
const P501_PFS: usize = 0x4004_0944;
#[cfg(target_arch = "arm")]
const P502_PFS: usize = 0x4004_0948;
#[cfg(target_arch = "arm")]
const PFS_ANALOG: u32 = 1 << 15;
#[cfg(target_arch = "arm")]
const PFS_GPT: u32 = (3 << 24) | (1 << 16);
#[cfg(target_arch = "arm")]
const PFS_SCI_EVEN: u32 = (4 << 24) | (1 << 16);
#[cfg(target_arch = "arm")]
const PFS_SCI_ODD: u32 = (5 << 24) | (1 << 16);
#[cfg(target_arch = "arm")]
const PFS_SPI: u32 = (6 << 24) | (1 << 16);
#[cfg(target_arch = "arm")]
const PFS_IIC: u32 = (7 << 24) | (1 << 16) | (1 << 10) | (1 << 4);

#[cfg(target_arch = "arm")]
const ADC0: usize = 0x4005_C000;
#[cfg(target_arch = "arm")]
const ADCSR: usize = 0x00;
#[cfg(target_arch = "arm")]
const ADANSA0: usize = 0x04;
#[cfg(target_arch = "arm")]
const ADCER: usize = 0x0e;
#[cfg(target_arch = "arm")]
const ADDR_BASE: usize = 0x20;
#[cfg(target_arch = "arm")]
const ADSSTR_BASE: usize = 0xe0;
#[cfg(target_arch = "arm")]
const ADCSR_ADST: u16 = 1 << 15;

#[cfg(target_arch = "arm")]
const GPT0: usize = 0x4007_8000;
#[cfg(target_arch = "arm")]
const GTWP: usize = 0x00;
#[cfg(target_arch = "arm")]
const GTSTR: usize = 0x04;
#[cfg(target_arch = "arm")]
const GTSTP: usize = 0x08;
#[cfg(target_arch = "arm")]
const GTCLR: usize = 0x0c;
#[cfg(target_arch = "arm")]
const GTCR: usize = 0x2c;
#[cfg(target_arch = "arm")]
const GTUDDTYC: usize = 0x30;
#[cfg(target_arch = "arm")]
const GTIOR: usize = 0x34;
#[cfg(target_arch = "arm")]
const GTBER: usize = 0x40;
#[cfg(target_arch = "arm")]
const GTCNT: usize = 0x48;
#[cfg(target_arch = "arm")]
const GTCCRA: usize = 0x4c;
#[cfg(target_arch = "arm")]
const GTPR: usize = 0x64;
#[cfg(target_arch = "arm")]
const GTPBR: usize = 0x68;
#[cfg(target_arch = "arm")]
const GPT_WRITE_UNLOCKED: u32 = 0xa500;
#[cfg(target_arch = "arm")]
const GPT_WRITE_PROTECTED: u32 = 0xa501;
#[cfg(target_arch = "arm")]
const GTIOR_PWM_A: u32 = 25 | (1 << 8);

#[cfg(target_arch = "arm")]
const SPI0: usize = 0x4007_2000;
#[cfg(target_arch = "arm")]
const SPCR: usize = 0x00;
#[cfg(target_arch = "arm")]
const SSLP: usize = 0x01;
#[cfg(target_arch = "arm")]
const SPPCR: usize = 0x02;
#[cfg(target_arch = "arm")]
const SPSR: usize = 0x03;
#[cfg(target_arch = "arm")]
const SPDR: usize = 0x04;
#[cfg(target_arch = "arm")]
const SPSCR: usize = 0x08;
#[cfg(target_arch = "arm")]
const SPBR: usize = 0x0a;
#[cfg(target_arch = "arm")]
const SPDCR: usize = 0x0b;
#[cfg(target_arch = "arm")]
const SPCKD: usize = 0x0c;
#[cfg(target_arch = "arm")]
const SSLND: usize = 0x0d;
#[cfg(target_arch = "arm")]
const SPND: usize = 0x0e;
#[cfg(target_arch = "arm")]
const SPCR2: usize = 0x0f;
#[cfg(target_arch = "arm")]
const SPCMD0: usize = 0x10;
#[cfg(target_arch = "arm")]
const SPCR_MSTR: u8 = 1 << 3;
#[cfg(target_arch = "arm")]
const SPCR_SPE: u8 = 1 << 6;
#[cfg(target_arch = "arm")]
const SPSR_SPTEF: u8 = 1 << 5;
#[cfg(target_arch = "arm")]
const SPSR_SPRF: u8 = 1 << 7;
#[cfg(target_arch = "arm")]
const SPI_ERROR_MASK: u8 = 0x1d;

#[cfg(target_arch = "arm")]
const IIC1: usize = 0x4005_3100;
#[cfg(target_arch = "arm")]
const ICCR1: usize = 0x00;
#[cfg(target_arch = "arm")]
const ICCR2: usize = 0x01;
#[cfg(target_arch = "arm")]
const ICMR1: usize = 0x02;
#[cfg(target_arch = "arm")]
const ICMR2: usize = 0x03;
#[cfg(target_arch = "arm")]
const ICMR3: usize = 0x04;
#[cfg(target_arch = "arm")]
const ICFER: usize = 0x05;
#[cfg(target_arch = "arm")]
const ICSER: usize = 0x06;
#[cfg(target_arch = "arm")]
const ICIER: usize = 0x07;
#[cfg(target_arch = "arm")]
const ICSR2: usize = 0x09;
#[cfg(target_arch = "arm")]
const ICBRL: usize = 0x10;
#[cfg(target_arch = "arm")]
const ICBRH: usize = 0x11;
#[cfg(target_arch = "arm")]
const ICDRT: usize = 0x12;
#[cfg(target_arch = "arm")]
const ICDRR: usize = 0x13;
#[cfg(target_arch = "arm")]
const ICSR2_TDRE: u8 = 1 << 7;
#[cfg(target_arch = "arm")]
const ICSR2_TEND: u8 = 1 << 6;
#[cfg(target_arch = "arm")]
const ICSR2_RDRF: u8 = 1 << 5;
#[cfg(target_arch = "arm")]
const ICSR2_NACKF: u8 = 1 << 4;
#[cfg(target_arch = "arm")]
const ICSR2_STOP: u8 = 1 << 3;
#[cfg(target_arch = "arm")]
const ICSR2_START: u8 = 1 << 2;
#[cfg(target_arch = "arm")]
const ICSR2_AL: u8 = 1 << 1;

#[cfg(target_arch = "arm")]
const SCI_SCR: usize = 0x02;
#[cfg(target_arch = "arm")]
const SCI_TDR: usize = 0x03;
#[cfg(target_arch = "arm")]
const SCI_SSR: usize = 0x04;
#[cfg(target_arch = "arm")]
const SCI_RDR: usize = 0x05;
#[cfg(target_arch = "arm")]
const SCI_SCMR: usize = 0x06;
#[cfg(target_arch = "arm")]
const SCI_SEMR: usize = 0x07;
#[cfg(target_arch = "arm")]
const SCI_TDRE: u8 = 1 << 7;
#[cfg(target_arch = "arm")]
const SCI_RDRF: u8 = 1 << 6;
#[cfg(target_arch = "arm")]
const SCI_TEND: u8 = 1 << 2;
#[cfg(target_arch = "arm")]
const SCI_ERROR_MASK: u8 = 0x38;

#[cfg(target_arch = "arm")]
unsafe fn start_module(register: usize, mask: u32) {
    let prior = read16(PRCR) & 0x0003;
    write16(PRCR, 0xa502);
    write32(register, read32(register) & !mask);
    write16(PRCR, 0xa500 | prior);
}

#[cfg(target_arch = "arm")]
unsafe fn configure_pfs(address: usize, value: u32) {
    write8(PWPR, 0);
    write8(PWPR, 0x40);
    write32(address, value);
    write8(PWPR, 0);
    write8(PWPR, 0x80);
}

#[cfg(target_arch = "arm")]
unsafe fn spi_fault() -> Result<(), PeripheralError> {
    let status = read8(SPI0 + SPSR);
    if status & SPI_ERROR_MASK == 0 {
        Ok(())
    } else {
        write8(SPI0 + SPSR, status & !SPI_ERROR_MASK);
        Err(PeripheralError::Overrun)
    }
}

#[cfg(target_arch = "arm")]
unsafe fn reset_iic1() -> Result<(), PeripheralError> {
    write8(IIC1 + ICCR1, 1 << 6);
    let mut budget = PollBudget::new(DEFAULT_POLLS);
    while read8(IIC1 + ICCR1) & (1 << 6) == 0 {
        budget.step()?;
    }
    write8(IIC1 + ICCR1, (1 << 7) | (1 << 6));
    write8(IIC1 + ICBRL, 0xe0 | 27);
    write8(IIC1 + ICBRH, 0xe0 | 26);
    write8(IIC1 + ICMR1, 2 << 4);
    write8(IIC1 + ICMR2, 0);
    write8(IIC1 + ICMR3, 0);
    write8(IIC1 + ICFER, 1);
    write8(IIC1 + ICSER, 0);
    write8(IIC1 + ICIER, 0);
    write8(IIC1 + ICCR1, 1 << 7);
    Ok(())
}

#[cfg(target_arch = "arm")]
unsafe fn iic_wait(mask: u8, set: bool, budget: &mut PollBudget) -> Result<(), PeripheralError> {
    loop {
        let status = read8(IIC1 + ICSR2);
        if status & ICSR2_AL != 0 {
            return Err(PeripheralError::ArbitrationLost);
        }
        if status & ICSR2_NACKF != 0 {
            return Err(PeripheralError::Nack);
        }
        if (status & mask != 0) == set {
            return Ok(());
        }
        budget.step()?;
        core::hint::spin_loop();
    }
}

#[cfg(target_arch = "arm")]
unsafe fn iic_start(repeated: bool, budget: &mut PollBudget) -> Result<(), PeripheralError> {
    if !repeated {
        while read8(IIC1 + ICCR2) & (1 << 7) != 0 {
            budget.step()?;
        }
    }
    write8(IIC1 + ICCR2, if repeated { 1 << 2 } else { 1 << 1 });
    iic_wait(ICSR2_START, true, budget)?;
    write8(IIC1 + ICSR2, read8(IIC1 + ICSR2) & !ICSR2_START);
    Ok(())
}

#[cfg(target_arch = "arm")]
unsafe fn iic_send(byte: u8, budget: &mut PollBudget) -> Result<(), PeripheralError> {
    iic_wait(ICSR2_TDRE, true, budget)?;
    write8(IIC1 + ICDRT, byte);
    Ok(())
}

#[cfg(target_arch = "arm")]
unsafe fn iic_stop(budget: &mut PollBudget) -> Result<(), PeripheralError> {
    write8(IIC1 + ICCR2, 1 << 3);
    iic_wait(ICSR2_STOP, true, budget)?;
    write8(IIC1 + ICSR2, read8(IIC1 + ICSR2) & !ICSR2_STOP);
    Ok(())
}

#[cfg(target_arch = "arm")]
unsafe fn iic1_transaction(
    address: u8,
    write: &[u8],
    read: &mut [u8],
) -> Result<(), PeripheralError> {
    let mut budget = PollBudget::new(DEFAULT_POLLS);
    iic_start(false, &mut budget)?;
    if !write.is_empty() || read.is_empty() {
        iic_send(address << 1, &mut budget)?;
        for &byte in write {
            iic_send(byte, &mut budget)?;
        }
    }
    if read.is_empty() {
        iic_wait(ICSR2_TEND, true, &mut budget)?;
        return iic_stop(&mut budget);
    }

    if !write.is_empty() {
        iic_wait(ICSR2_TEND, true, &mut budget)?;
        iic_start(true, &mut budget)?;
    }
    // A pure read sends the read address after the initial START. A write/read
    // transaction sends it after the repeated START above.
    iic_send((address << 1) | 1, &mut budget)?;
    if read.len() <= 2 {
        write8(IIC1 + ICMR3, read8(IIC1 + ICMR3) | (1 << 6));
    }
    iic_wait(ICSR2_RDRF, true, &mut budget)?;
    let _ = read8(IIC1 + ICDRR);
    let read_len = read.len();
    for (index, byte) in read.iter_mut().enumerate() {
        let remaining = read_len - index;
        if remaining == 3 {
            write8(IIC1 + ICMR3, read8(IIC1 + ICMR3) | (1 << 6));
        } else if remaining == 2 {
            write8(IIC1 + ICMR3, read8(IIC1 + ICMR3) | (1 << 4));
            write8(IIC1 + ICMR3, read8(IIC1 + ICMR3) | (1 << 3));
            write8(IIC1 + ICMR3, read8(IIC1 + ICMR3) & !(1 << 4));
        } else if remaining == 1 {
            write8(IIC1 + ICCR2, 1 << 3);
        }
        iic_wait(ICSR2_RDRF, true, &mut budget)?;
        *byte = read8(IIC1 + ICDRR);
    }
    write8(IIC1 + ICMR3, read8(IIC1 + ICMR3) & !(1 << 6));
    iic_wait(ICSR2_STOP, true, &mut budget)?;
    write8(IIC1 + ICSR2, read8(IIC1 + ICSR2) & !ICSR2_STOP);
    Ok(())
}

#[cfg(target_arch = "arm")]
unsafe fn initialize_sci(base: usize) {
    write8(base + SCI_SCR, 0);
    write8(base, 0);
    write8(base + SCI_SCMR, 0xf2);
    write8(base + SCI_SEMR, 0x40);
    write8(base + 1, crate::system::SCI_BRR);
    for _ in 0..2_000 {
        cortex_m::asm::nop();
    }
    write8(base + SCI_SSR, 0xc7);
    write8(base + SCI_SCR, 0x30);
}

#[cfg(target_arch = "arm")]
unsafe fn recover_sci(base: usize) {
    write8(base + SCI_SCR, 0);
    let _ = read8(base + SCI_RDR);
    write8(base + SCI_SSR, 0xc7);
    write8(base + SCI_SCR, 0x30);
}

#[cfg(target_arch = "arm")]
unsafe fn read8(address: usize) -> u8 {
    (address as *const u8).read_volatile()
}

#[cfg(target_arch = "arm")]
unsafe fn write8(address: usize, value: u8) {
    (address as *mut u8).write_volatile(value);
}

#[cfg(target_arch = "arm")]
unsafe fn read16(address: usize) -> u16 {
    (address as *const u16).read_volatile()
}

#[cfg(target_arch = "arm")]
unsafe fn write16(address: usize, value: u16) {
    (address as *mut u16).write_volatile(value);
}

#[cfg(target_arch = "arm")]
unsafe fn read32(address: usize) -> u32 {
    (address as *const u32).read_volatile()
}

#[cfg(target_arch = "arm")]
unsafe fn write32(address: usize, value: u32) {
    (address as *mut u32).write_volatile(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_budget_has_an_exact_terminal_boundary() {
        let mut budget = PollBudget::new(2);
        assert_eq!(budget.step(), Ok(()));
        assert_eq!(budget.step(), Ok(()));
        assert_eq!(budget.step(), Err(PeripheralError::Timeout));
    }

    #[test]
    fn spi_divisor_is_bounded_and_never_faster_than_requested() {
        assert_eq!(spi_divisor(4_000_000), Ok(5));
        assert_eq!(spi_divisor(24_000_000), Ok(0));
        assert_eq!(spi_divisor(0), Err(PeripheralError::InvalidConfig));
        assert_eq!(spi_divisor(1), Err(PeripheralError::InvalidConfig));
    }

    #[test]
    fn exact_uart_instances_do_not_conflate_controller_and_user_links() {
        assert_eq!(UartInstance::WifiController.channel(), 1);
        assert_eq!(UartInstance::Header.channel(), 2);
        assert_eq!(UartInstance::UsbBridge.channel(), 9);
        assert_ne!(
            UartInstance::WifiController.base(),
            UartInstance::UsbBridge.base()
        );
    }

    #[test]
    fn host_peripherals_fail_before_inventing_hardware_results() {
        let _lock = crate::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let adc = Ra4m1Adc::try_a0(51).unwrap();
        assert_eq!(adc.channel(), 9);
        drop(adc);

        let mut spi = Ra4m1Spi::try_uno_header(52, 4_000_000).unwrap();
        let mut rx = [0; 2];
        assert_eq!(
            spi.transfer(&[1], &mut rx),
            Err(PeripheralError::LengthMismatch)
        );
        assert_eq!(
            spi.transfer(&[1, 2], &mut rx),
            Err(PeripheralError::UnsupportedOnHost)
        );
    }
}
