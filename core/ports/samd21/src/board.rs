//! Exact Arduino Zero-compatible SAMD21G18A composition.
//!
//! The aliases follow the installed Arduino SAMD Zero variant.  In particular,
//! `Wire` is SERCOM3 on PA22/PA23 and the ICSP header is SERCOM4 on
//! PA12/PB10/PB11.  These are independent controller instances, so a PN532 on
//! each bus can be owned concurrently.

use atsamd_hal as hal;

use hal::clock::GenericClockController;
use hal::dmac::DmaController;
use hal::eic::{Ch7, Eic, EicPin, ExtInt, Sense};
use hal::gpio::{Pin, PullUpInterrupt, Reset, PA07};
use hal::pwm::Pwm1;
use hal::rtc::{Count32Mode, Rtc};
use hal::sercom::{i2c, spi, uart, Sercom0, Sercom3, Sercom4};
use hal::time::Hertz;
use hal::timer::TimerCounter4;
use hal::usb::{usb_device::bus::UsbBusAllocator, UsbBus};

hal::bsp_pins!(
    PA02 { name: a0 }
    PA06 { name: d8 }
    PA07 { name: d9 }
    PA10 {
        name: d1
        aliases: { AlternateC: UartTx }
    }
    PA11 {
        name: d0
        aliases: { AlternateC: UartRx }
    }
    PA12 {
        name: miso
        aliases: { AlternateD: Miso }
    }
    PA17 { name: d13 }
    PA22 {
        name: sda
        aliases: { AlternateC: Sda }
    }
    PA23 {
        name: scl
        aliases: { AlternateC: Scl }
    }
    PA24 {
        name: usb_dm
        aliases: { AlternateG: UsbDm }
    }
    PA25 {
        name: usb_dp
        aliases: { AlternateG: UsbDp }
    }
    PB10 {
        name: mosi
        aliases: { AlternateD: Mosi }
    }
    PB11 {
        name: sck
        aliases: { AlternateD: Sclk }
    }
);

pub const BOARD_ID: &str = "samd21-m0-mini-arduino-zero";
pub const CPU_HZ: u32 = 48_000_000;
pub const APP_FLASH_ORIGIN: u32 = 0x0000_2000;
pub const APP_FLASH_BYTES: u32 = 248 * 1024;

pub const PN532_I2C_ADDRESS: u8 = 0x24;
pub const PN532_SPI_SERCOM: u8 = 4;
pub const PN532_SPI_SS_DIGITAL_PIN: u8 = 8;
pub const PN532_SPI_IRQ_DIGITAL_PIN: u8 = 9;
pub const PN532_SPI_IRQ_EIC_LINE: u8 = 7;

/// The owner fixture connects PN532 RSTO to the board RESET net.  It is an
/// observed reset output, not a software-driven peripheral reset pin.
pub const PN532_RSTO_DRIVES_BOARD_RESET: bool = true;

pub type I2cPads = i2c::Pads<Sercom3, Sda, Scl>;
pub type I2c = i2c::I2c<i2c::Config<I2cPads>>;
pub type SpiPads = spi::Pads<Sercom4, Miso, Mosi, Sclk>;
pub type Spi = spi::Spi<spi::Config<SpiPads>, spi::Duplex>;
pub type UartPads = uart::Pads<Sercom0, UartRx, UartTx>;
pub type Uart = uart::Uart<uart::Config<UartPads>, uart::Duplex>;

pub fn clocks(
    gclk: hal::pac::Gclk,
    pm: &mut hal::pac::Pm,
    sysctrl: &mut hal::pac::Sysctrl,
    nvmctrl: &mut hal::pac::Nvmctrl,
) -> GenericClockController {
    GenericClockController::with_external_32kosc(gclk, pm, sysctrl, nvmctrl)
}

pub fn event_controller(
    clocks: &mut GenericClockController,
    eic: hal::pac::Eic,
    pm: &mut hal::pac::Pm,
) -> Eic {
    let gclk0 = clocks.gclk0();
    let clock = clocks
        .eic(&gclk0)
        .expect("EIC clock must be claimed once by the event owner");
    Eic::new(pm, clock, eic)
}

pub type Pn532Irq = ExtInt<Pin<PA07, PullUpInterrupt>, Ch7>;

/// Bind D9/PA07 to EIC EXTINT7 and publish a filtered falling-edge event.
///
/// PN532 IRQ is active low. The pull-up makes an unattached or resetting
/// reader fail high, while edge sensing prevents a permanently asserted line
/// from repeatedly pacing DMAC channel 0.
pub fn pn532_irq(eic: Eic, d9: Pin<PA07, Reset>) -> Pn532Irq {
    let channels = eic.split();
    let mut irq = d9.into_pull_up_ei(channels.7);
    irq.sense(Sense::Fall);
    irq.filter(true);
    irq.enable_event();
    irq
}

pub fn dma_controller(dmac: hal::pac::Dmac, pm: &mut hal::pac::Pm) -> DmaController {
    DmaController::init(dmac, pm)
}

/// Route EIC EXTINT7 (D9 on this composition) to DMAC channel 0 through
/// asynchronous EVSYS channel 0.
///
/// The caller still owns the typed EIC channel and must enable its event
/// output. EVSYS uses channel+1 in USER, so USER_DMAC_CH_0 selects value 1.
pub fn pn532_irq_event_route(evsys: hal::pac::Evsys, pm: &mut hal::pac::Pm) -> hal::pac::Evsys {
    pm.apbcmask().modify(|_, w| w.evsys_().set_bit());
    evsys.channel().write(|w| unsafe {
        // CHANNEL=0, EVGEN=EIC_EXTINT_7 (19), PATH=asynchronous (2).
        w.bits((19u32 << 16) | (2u32 << 24))
    });
    evsys.user().write(|w| unsafe {
        // USER=DMAC_CH_0 (0), CHANNEL=EVSYS channel 0 + 1.
        w.bits(1u16 << 8)
    });
    evsys
}

pub fn rtc_timebase(
    clocks: &mut GenericClockController,
    rtc: hal::pac::Rtc,
    pm: &mut hal::pac::Pm,
) -> Rtc<Count32Mode> {
    let gclk1 = clocks.gclk1();
    let clock = clocks
        .rtc(&gclk1)
        .expect("RTC clock must be claimed once by the time owner");
    Rtc::count32_mode(rtc, clock.freq(), pm)
}

pub fn tc4_alarm(
    clocks: &mut GenericClockController,
    tc4: hal::pac::Tc4,
    pm: &mut hal::pac::Pm,
) -> TimerCounter4 {
    let gclk0 = clocks.gclk0();
    let clock = clocks
        .tc4_tc5(&gclk0)
        .expect("TC4/TC5 clock must be claimed once by the deadline owner");
    TimerCounter4::tc4_(&clock, tc4, pm)
}

pub fn tcc1_pwm(
    clocks: &mut GenericClockController,
    frequency: Hertz,
    tcc1: hal::pac::Tcc1,
    pm: &mut hal::pac::Pm,
) -> Pwm1 {
    let gclk0 = clocks.gclk0();
    let clock = clocks
        .tcc0_tcc1(&gclk0)
        .expect("TCC0/TCC1 clock must be claimed once by the PWM owner");
    Pwm1::new(&clock, frequency, tcc1, pm)
}

pub fn i2c_master(
    clocks: &mut GenericClockController,
    baud: Hertz,
    sercom: hal::pac::Sercom3,
    pm: &hal::pac::Pm,
    sda: impl Into<Sda>,
    scl: impl Into<Scl>,
) -> I2c {
    let gclk0 = clocks.gclk0();
    let clock = clocks
        .sercom3_core(&gclk0)
        .expect("SERCOM3 clock must be claimed once by the I2C owner");
    let pads = i2c::Pads::new(sda.into(), scl.into());
    i2c::Config::new(pm, sercom, pads, clock.freq())
        .baud(baud)
        .enable()
}

pub fn spi_master(
    clocks: &mut GenericClockController,
    baud: Hertz,
    sercom: hal::pac::Sercom4,
    pm: &hal::pac::Pm,
    sck: impl Into<Sclk>,
    mosi: impl Into<Mosi>,
    miso: impl Into<Miso>,
) -> Spi {
    let gclk0 = clocks.gclk0();
    let clock = clocks
        .sercom4_core(&gclk0)
        .expect("SERCOM4 clock must be claimed once by the SPI owner");
    let pads = spi::Pads::default()
        .data_in(miso.into())
        .data_out(mosi.into())
        .sclk(sck.into());
    spi::Config::new(pm, sercom, pads, clock.freq())
        .baud(baud)
        .spi_mode(spi::MODE_0)
        .enable()
}

pub fn uart(
    clocks: &mut GenericClockController,
    baud: Hertz,
    sercom: hal::pac::Sercom0,
    pm: &hal::pac::Pm,
    rx: impl Into<UartRx>,
    tx: impl Into<UartTx>,
) -> Uart {
    let gclk0 = clocks.gclk0();
    let clock = clocks
        .sercom0_core(&gclk0)
        .expect("SERCOM0 clock must be claimed once by the UART owner");
    let pads = uart::Pads::default().rx(rx.into()).tx(tx.into());
    uart::Config::new(pm, sercom, pads, clock.freq())
        .baud(baud, uart::BaudMode::Fractional(uart::Oversampling::Bits16))
        .enable()
}

pub fn usb_allocator(
    usb: hal::pac::Usb,
    clocks: &mut GenericClockController,
    pm: &mut hal::pac::Pm,
    dm: impl Into<UsbDm>,
    dp: impl Into<UsbDp>,
) -> UsbBusAllocator<UsbBus> {
    let gclk0 = clocks.gclk0();
    let clock = clocks
        .usb(&gclk0)
        .expect("USB clock must be claimed once by the USB owner");
    UsbBusAllocator::new(UsbBus::new(&clock, pm, dm.into(), dp.into(), usb))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pn532I2cBinding {
    pub sercom: u8,
    pub address: u8,
}

impl Pn532I2cBinding {
    pub const EXACT: Self = Self {
        sercom: 3,
        address: PN532_I2C_ADDRESS,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pn532SpiIrqBinding {
    pub sercom: u8,
    pub ss_pin: u8,
    pub irq_pin: u8,
    pub eic_line: u8,
}

impl Pn532SpiIrqBinding {
    pub const EXACT: Self = Self {
        sercom: PN532_SPI_SERCOM,
        ss_pin: PN532_SPI_SS_DIGITAL_PIN,
        irq_pin: PN532_SPI_IRQ_DIGITAL_PIN,
        eic_line: PN532_SPI_IRQ_EIC_LINE,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pn532_instances_are_independent_and_exact() {
        assert_eq!(Pn532I2cBinding::EXACT.sercom, 3);
        assert_eq!(Pn532SpiIrqBinding::EXACT.sercom, 4);
        assert_ne!(
            Pn532I2cBinding::EXACT.sercom,
            Pn532SpiIrqBinding::EXACT.sercom
        );
        assert_eq!(Pn532SpiIrqBinding::EXACT.eic_line, 7);
        assert!(PN532_RSTO_DRIVES_BOARD_RESET);
    }

    #[test]
    fn bootloader_reservation_matches_zero_layout() {
        assert_eq!(APP_FLASH_ORIGIN, 8 * 1024);
        assert_eq!(APP_FLASH_ORIGIN + APP_FLASH_BYTES, 256 * 1024);
    }
}
