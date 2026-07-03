//! nRF52840 USB backend over the vendored `nrf-usbd` + `usbd-serial` CDC (the default for
//! the nRF dev boards, matching ArduinoNRF Layer-0's native `NrfUsbd`). Owns a `'static`
//! bus allocator so the `UsbDevice`/`SerialPort` can live inside the backend struct.

use nrf_usbd::{Usbd, UsbPeripheral};
use usb_device::device::{UsbDevice, UsbDeviceBuilder, UsbDeviceState, UsbVidPid};
use usb_device::{bus::UsbBusAllocator, device::StringDescriptors};
use usbd_serial::SerialPort;

use crate::{backend_id, CdcState, UsbConfig, UsbStack};

struct Nrf52840Usbd;
// nrf-usbd applies the mandatory USB errata itself; it only needs the register base.
unsafe impl UsbPeripheral for Nrf52840Usbd {
    const REGISTERS: *const () = 0x4002_7000 as *const ();
}

type Bus = Usbd<Nrf52840Usbd>;

// The allocator must outlive the device + serial (which borrow it), so it lives in a
// static. A board mounts a single USB stack, so a single slot is sufficient.
static mut ALLOC: Option<UsbBusAllocator<Bus>> = None;

const CLOCK: u32 = 0x4000_0000;
const POWER: u32 = 0x4000_0000; // POWER shares the base region on nRF52
const USBD: u32 = 0x4002_7000;

unsafe fn rd(a: u32) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}
unsafe fn wr(a: u32, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v);
}

/// Bring up the clock/VBUS/USBD from a clean state (raw registers, so this does not need
/// to own the PAC `Peripherals`). Mirrors the sequence proven on board1 + board5.
unsafe fn peripheral_clean_start() {
    // HFXO (USB needs the external 32 MHz crystal).
    wr(CLOCK + 0x000, 1); // TASKS_HFCLKSTART
    while rd(CLOCK + 0x100) == 0 {} // EVENTS_HFCLKSTARTED
    // Gate on VBUS present (do NOT wait on OUTPUTRDY - it never sets on VDD-powered boards).
    while rd(POWER + 0x438) & 1 == 0 {} // POWER.USBREGSTATUS.VBUSDETECT
    // Clean start: disconnect pullup, disable USBD, clear leftover events (a UF2
    // bootloader can hand USBD off dirty). No-op on an already-clean board.
    wr(USBD + 0x504, 0); // USBPULLUP.connect = disabled
    wr(USBD + 0x500, 0); // ENABLE = disabled
    wr(USBD + 0x10C, 0); // EVENTS_USBRESET
    wr(USBD + 0x158, 0); // EVENTS_USBEVENT
    wr(USBD + 0x104, 0); // EVENTS_EP0SETUP
    wr(USBD + 0x400, 0xFFFF_FFFF); // EVENTCAUSE (W1C)
    for _ in 0..400_000u32 {
        cortex_m::asm::nop();
    }
}

/// CDC-ACM over nrf-usbd.
pub struct NrfUsbdCdc {
    serial: SerialPort<'static, Bus>,
    dev: UsbDevice<'static, Bus>,
    ever_configured: bool,
}

impl NrfUsbdCdc {
    pub fn mount(cfg: &UsbConfig) -> Self {
        unsafe {
            peripheral_clean_start();
            ALLOC = Some(UsbBusAllocator::new(Usbd::new(Nrf52840Usbd)));
            let alloc: &'static UsbBusAllocator<Bus> =
                (*core::ptr::addr_of!(ALLOC)).as_ref().unwrap();
            let serial = SerialPort::new(alloc);
            let strings = StringDescriptors::default()
                .manufacturer(cfg.manufacturer)
                .product(cfg.product)
                .serial_number(cfg.serial);
            let dev = UsbDeviceBuilder::new(alloc, UsbVidPid(cfg.vid, cfg.pid))
                .strings(&[strings])
                .unwrap()
                .device_class(usbd_serial::USB_CLASS_CDC)
                .build();
            NrfUsbdCdc { serial, dev, ever_configured: false }
        }
    }
}

impl UsbStack for NrfUsbdCdc {
    fn poll(&mut self) -> CdcState {
        self.dev.poll(&mut [&mut self.serial]);
        let state = match self.dev.state() {
            UsbDeviceState::Default => CdcState::Default,
            UsbDeviceState::Addressed => CdcState::Addressed,
            UsbDeviceState::Configured => CdcState::Configured,
            UsbDeviceState::Suspend => CdcState::Disconnected,
        };
        if state == CdcState::Configured {
            self.ever_configured = true;
        }
        state
    }

    fn write(&mut self, data: &[u8]) -> usize {
        self.serial.write(data).unwrap_or(0)
    }

    fn read(&mut self, buf: &mut [u8]) -> usize {
        self.serial.read(buf).unwrap_or(0)
    }

    fn configured(&self) -> bool {
        self.ever_configured
    }

    fn backend_id(&self) -> u32 {
        backend_id::NRF_USBD
    }
}
