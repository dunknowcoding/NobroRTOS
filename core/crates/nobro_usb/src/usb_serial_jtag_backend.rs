//! ESP32-C3/S3 native USB-Serial-JTAG backend (mountable via
//! `--features backend-usb-serial-jtag`).
//!
//! Unlike the other backends, this peripheral is a *fixed-function* CDC-ACM-style
//! bridge: descriptors (Espressif VID 0x303A) and the whole enumeration state machine
//! live in silicon, so there is nothing to configure from [`UsbConfig`] and no software
//! control pipe to run. The mountable surface maps directly onto the two data-path
//! registers (TRM ch. "USB Serial/JTAG Controller"):
//!   EP1       (+0x00)  byte FIFO, read = OUT data, write = IN data
//!   EP1_CONF  (+0x04)  bit0 WR_DONE (flush IN), bit1 IN_EP_DATA_FREE, bit2 OUT_EP_DATA_AVAIL
//!   INT_RAW   (+0x08)  bit1 SOF - the host is enumerated and sending frames
//! Host presence is detected from the SOF interrupt flag: a configured host sends a
//! start-of-frame every millisecond, so seeing SOF means the pipe is live.

use crate::{backend_id, CdcState, UsbConfig, UsbStack};

const BASE: usize = 0x6004_3000;
const EP1: *mut u32 = BASE as *mut u32;
const EP1_CONF: *mut u32 = (BASE + 0x04) as *mut u32;
const INT_RAW: *mut u32 = (BASE + 0x08) as *mut u32;
const INT_CLR: *mut u32 = (BASE + 0x14) as *mut u32;

const WR_DONE: u32 = 1 << 0;
const IN_EP_DATA_FREE: u32 = 1 << 1;
const OUT_EP_DATA_AVAIL: u32 = 1 << 2;
const SOF_INT: u32 = 1 << 1;
const OUT_RECV_PKT_INT: u32 = 1 << 2;

pub struct UsbSerialJtagCdc {
    _cfg: UsbConfig,
    seen_sof: bool,
}

impl UsbSerialJtagCdc {
    /// The peripheral needs no bring-up: it enumerates by itself as soon as VBUS is
    /// present. `cfg` identity strings are advisory only (silicon owns the descriptors).
    pub fn mount(cfg: &UsbConfig) -> Self {
        UsbSerialJtagCdc {
            _cfg: *cfg,
            seen_sof: false,
        }
    }
}

impl UsbStack for UsbSerialJtagCdc {
    fn poll(&mut self) -> CdcState {
        let raw = unsafe { INT_RAW.read_volatile() };
        if raw & SOF_INT != 0 {
            unsafe { INT_CLR.write_volatile(SOF_INT) };
            self.seen_sof = true;
        }
        if self.seen_sof {
            CdcState::Configured
        } else {
            CdcState::Disconnected
        }
    }

    fn write(&mut self, data: &[u8]) -> usize {
        let mut n = 0;
        for &b in data {
            if unsafe { EP1_CONF.read_volatile() } & IN_EP_DATA_FREE == 0 {
                break; // IN FIFO full (64 bytes); caller retries the rest
            }
            unsafe { EP1.write_volatile(u32::from(b)) };
            n += 1;
        }
        if n > 0 {
            unsafe { EP1_CONF.write_volatile(WR_DONE) }; // hand the IN FIFO to the host
        }
        n
    }

    fn read(&mut self, buf: &mut [u8]) -> usize {
        // Gate on the RECV_PKT interrupt, not just DATA_AVAIL: the IN and OUT endpoints
        // share FIFO RAM in silicon, and bus events (e.g. the host opening the port) can
        // leave DATA_AVAIL raised over stale IN bytes. RECV_PKT only fires for a real
        // OUT packet from the host.
        if unsafe { INT_RAW.read_volatile() } & OUT_RECV_PKT_INT == 0 {
            return 0;
        }
        unsafe { INT_CLR.write_volatile(OUT_RECV_PKT_INT) };
        let mut n = 0;
        while n < buf.len() && unsafe { EP1_CONF.read_volatile() } & OUT_EP_DATA_AVAIL != 0 {
            buf[n] = unsafe { EP1.read_volatile() } as u8;
            n += 1;
        }
        n
    }

    fn configured(&self) -> bool {
        self.seen_sof
    }

    fn backend_id(&self) -> u32 {
        backend_id::USB_SERIAL_JTAG
    }
}
