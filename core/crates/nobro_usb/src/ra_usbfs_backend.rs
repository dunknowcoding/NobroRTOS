//! ESP-free RA4M1 (Arduino UNO R4) USBFS device backend for the mountable USB stack
//! (`--features backend-ra-usbfs`). Self-contained raw-register CDC-ACM, no vendored C:
//! the register sequences are ported from the RA hardware manual and cross-checked
//! against TinyUSB's proven `dcd_rusb2` driver. Like every nobro_usb backend it presents
//! the common [`UsbStack`] trait, so the RA4M1 port reports over its own native USB
//! exactly the way board5 does over nrf-usbd or the C3 over USB-Serial-JTAG.
//!
//! The peripheral is pipe-based: PIPE0 is the default control pipe (enumeration), and we
//! use PIPE1 (bulk IN) + PIPE2 (bulk OUT) for the CDC data endpoints. Enumeration is a
//! small state machine driven from `poll()`; `stage()` exposes how far it has progressed
//! so a probe-less board can blink the stage on its LED when USB is silent.

use crate::{backend_id, CdcState, UsbConfig, UsbStack};

const BASE: usize = 0x4009_0000;
macro_rules! r16 {
    ($name:ident, $off:expr) => {
        const $name: *mut u16 = (BASE + $off) as *mut u16;
    };
}
r16!(SYSCFG, 0x00);
r16!(DVSTCTR0, 0x08);
const CFIFO: *mut u16 = (BASE + 0x14) as *mut u16;
const CFIFO8: *mut u8 = (BASE + 0x14) as *mut u8;
r16!(CFIFOSEL, 0x20);
r16!(CFIFOCTR, 0x22);
r16!(INTENB0, 0x30);
r16!(BEMPENB, 0x3A);
r16!(INTSTS0, 0x40);
r16!(BRDYSTS, 0x46);
r16!(BEMPSTS, 0x4A);
r16!(USBADDR, 0x50);
r16!(USBREQ, 0x54);
r16!(USBVAL, 0x56);
r16!(USBINDX, 0x58);
r16!(USBLENG, 0x5A);
r16!(DCPMAXP, 0x5E);
r16!(DCPCTR, 0x60);
r16!(PIPESEL, 0x64);
r16!(PIPECFG, 0x68);
r16!(PIPEMAXP, 0x6C);
const PIPE1CTR: *mut u16 = (BASE + 0x70) as *mut u16;
const PIPE2CTR: *mut u16 = (BASE + 0x72) as *mut u16;

const MSTPCRB: *mut u32 = 0x4004_7000 as *mut u32;

// SYSCFG bits
const SYSCFG_SCKE: u16 = 1 << 10;
const SYSCFG_DCFM: u16 = 1 << 6;
const SYSCFG_DPRPU: u16 = 1 << 4;
const SYSCFG_USBE: u16 = 1 << 0;
const SYSCFG_DRPD: u16 = 1 << 5;
// INTSTS0 bits
const IS_VBSTS: u16 = 1 << 7;
const IS_DVST: u16 = 1 << 6;
const IS_CTRT: u16 = 1 << 15;
const IS_VALID: u16 = 1 << 3;
const CTSQ_MASK: u16 = 0x7;
// DVSTCTR0
const RHST_MASK: u16 = 0x7;
// CFIFOSEL / CFIFOCTR
const CF_RCNT: u16 = 1 << 15;
const CF_REW: u16 = 1 << 14;
const CF_ISEL: u16 = 1 << 5;
const CF_BVAL: u16 = 1 << 15;
const CF_BCLR: u16 = 1 << 14;
const CF_FRDY: u16 = 1 << 13;
const CF_DTLN_MASK: u16 = 0xFFF;
// DCPCTR / PIPExCTR
const CTR_CCPL: u16 = 1 << 2;
const PID_BUF: u16 = 1;
const PID_NAK: u16 = 0;
const CTR_BSTS: u16 = 1 << 7;

/// Progress markers so a silent board can signal where enumeration stalled.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    PoweredOff = 0,
    Attached = 1,   // pull-up asserted, waiting for reset
    Reset = 2,      // bus reset seen
    Addressed = 3,  // SET_ADDRESS handled
    Configured = 4, // SET_CONFIGURATION handled - CDC usable
}

// ---- USB descriptors (a minimal single-CDC-ACM device) ----
const VID: u16 = 0x1209; // pid.codes open VID
const PID: u16 = 0x0004;

#[rustfmt::skip]
const DEV_DESC: [u8; 18] = [
    18, 1, 0x00, 0x02, 0xEF, 0x02, 0x01, 64, // USB2.0, misc/IAD class, EP0 = 64
    (VID & 0xFF) as u8, (VID >> 8) as u8,
    (PID & 0xFF) as u8, (PID >> 8) as u8,
    0x00, 0x01, 1, 2, 0, 1, // bcdDevice, iMfr, iProduct, iSerial, numConfigs
];

#[rustfmt::skip]
const CFG_DESC: [u8; 75] = [
    9, 2, 75, 0, 2, 1, 0, 0x80, 50,            // config: 2 interfaces, bus powered
    8, 11, 0, 2, 2, 2, 1, 0,                    // IAD: CDC comm+data
    9, 4, 0, 0, 1, 2, 2, 1, 0,                  // interface 0: CDC comm
    5, 0x24, 0, 0x10, 0x01,                     // CDC header
    5, 0x24, 1, 0x00, 1,                        // CDC call mgmt
    4, 0x24, 2, 0x02,                           // CDC ACM
    5, 0x24, 6, 0, 1,                           // CDC union
    7, 5, 0x83, 3, 8, 0, 16,                    // EP3 IN interrupt (notifications)
    9, 4, 1, 0, 2, 10, 0, 0, 0,                 // interface 1: CDC data
    7, 5, 0x01, 2, 64, 0, 0,                    // EP1 OUT bulk
    7, 5, 0x82, 2, 64, 0, 0,                    // EP2 IN bulk
];

pub struct RaUsbfsCdc {
    _cfg: UsbConfig,
    stage: Stage,
    pending_addr: u8,
    rx: [u8; 64],
    rx_len: usize,
    rx_pos: usize,
}

impl RaUsbfsCdc {
    pub fn mount(cfg: &UsbConfig) -> Self {
        unsafe {
            // release USBFS from module-stop (MSTPCRB bit 11)
            MSTPCRB.write_volatile(MSTPCRB.read_volatile() & !(1 << 11));
            // Force a disconnect edge first: the bootloader may have left its own CDC
            // enumerated, so drop the D+ pull-up and hold long enough for the host to
            // notice the unplug before we re-enumerate as our device. (This drop is also
            // the decisive proof-of-execution signal - if the host's port vanishes, our
            // code is definitely running on the RA4M1.)
            SYSCFG.write_volatile(SYSCFG_USBE); // USBE on, DPRPU off
            let mut spin = 0u32; // ~600 ms hold at 8 MHz so the host sees the unplug
            while spin < 4_000_000 {
                core::hint::spin_loop();
                spin += 1;
            }
            // device controller bring-up (dcd_init, full-speed path)
            SYSCFG.write_volatile(SYSCFG_SCKE);
            while SYSCFG.read_volatile() & SYSCFG_SCKE == 0 {}
            SYSCFG.write_volatile((SYSCFG.read_volatile() & !SYSCFG_DRPD & !SYSCFG_DCFM) | SYSCFG_USBE);
            DCPMAXP.write_volatile(64); // EP0 max packet
            INTENB0.write_volatile(0);
            BEMPENB.write_volatile(0);
            // assert the D+ pull-up so the host begins enumeration (dcd_connect)
            SYSCFG.write_volatile(SYSCFG.read_volatile() | SYSCFG_DPRPU);
        }
        RaUsbfsCdc {
            _cfg: *cfg,
            stage: Stage::Attached,
            pending_addr: 0,
            rx: [0; 64],
            rx_len: 0,
            rx_pos: 0,
        }
    }

    /// How far enumeration has progressed - blink this on the LED when USB is silent.
    pub fn stage(&self) -> Stage {
        self.stage
    }

    // ---- control pipe (PIPE0) helpers via CFIFO ----

    fn ctrl_select_read(&self) {
        unsafe {
            CFIFOSEL.write_volatile(CF_RCNT | 0); // CURPIPE=0 (DCP), read direction
            while CFIFOSEL.read_volatile() & CF_ISEL != 0 {}
        }
    }

    fn ctrl_write_data(&self, data: &[u8]) {
        unsafe {
            CFIFOSEL.write_volatile(CF_ISEL | 0); // DCP, write direction
            while CFIFOSEL.read_volatile() & CF_ISEL == 0 {}
            CFIFOCTR.write_volatile(CF_BCLR); // clear buffer
            while CFIFOCTR.read_volatile() & CF_FRDY == 0 {}
            let mut i = 0;
            while i + 1 < data.len() {
                CFIFO.write_volatile(u16::from(data[i]) | (u16::from(data[i + 1]) << 8));
                i += 2;
            }
            if i < data.len() {
                CFIFO8.write_volatile(data[i]);
            }
            CFIFOCTR.write_volatile(CF_BVAL); // mark buffer valid to send
        }
    }

    /// Complete the control transfer status stage.
    fn ctrl_status_done(&self) {
        unsafe {
            DCPCTR.write_volatile(CTR_CCPL | PID_BUF);
        }
    }

    fn handle_setup(&mut self) {
        let (req, val, idx, len) = unsafe {
            (
                USBREQ.read_volatile(),
                USBVAL.read_volatile(),
                USBINDX.read_volatile(),
                USBLENG.read_volatile(),
            )
        };
        let _ = idx;
        let bm = (req & 0xFF) as u8;
        let brequest = (req >> 8) as u8;
        unsafe { INTSTS0.write_volatile(!IS_VALID) }; // clear VALID

        match (bm, brequest) {
            // GET_DESCRIPTOR (device-to-host, standard)
            (0x80, 0x06) => {
                let dtype = (val >> 8) as u8;
                let resp: &[u8] = match dtype {
                    1 => &DEV_DESC,
                    2 => &CFG_DESC,
                    3 => Self::string_desc((val & 0xFF) as u8),
                    _ => &[],
                };
                if resp.is_empty() {
                    self.ctrl_status_done();
                } else {
                    let n = (len as usize).min(resp.len());
                    self.ctrl_write_data(&resp[..n]);
                }
            }
            // SET_ADDRESS: RUSB2 latches the address itself; record for stage tracking
            (0x00, 0x05) => {
                self.pending_addr = (val & 0x7F) as u8;
                self.stage = Stage::Addressed;
                self.ctrl_status_done();
            }
            // SET_CONFIGURATION
            (0x00, 0x09) => {
                if val != 0 {
                    self.open_data_pipes();
                    self.stage = Stage::Configured;
                }
                self.ctrl_status_done();
            }
            // CDC SET_LINE_CODING / SET_CONTROL_LINE_STATE / others: ack
            _ => {
                self.ctrl_status_done();
            }
        }
    }

    fn string_desc(index: u8) -> &'static [u8] {
        // 0 = langid (en-US), 1 = "NOBRO", 2 = "NobroRTOS RA4M1"
        const LANGID: [u8; 4] = [4, 3, 0x09, 0x04];
        const MFR: [u8; 12] = [12, 3, b'N', 0, b'O', 0, b'B', 0, b'R', 0, b'O', 0];
        const PROD: [u8; 12] = [12, 3, b'R', 0, b'A', 0, b'4', 0, b'M', 0, b'1', 0];
        match index {
            0 => &LANGID,
            1 => &MFR,
            _ => &PROD,
        }
    }

    /// Configure PIPE1 (bulk OUT, EP1) and PIPE2 (bulk IN, EP2) for CDC data.
    fn open_data_pipes(&self) {
        unsafe {
            // PIPE1 = bulk, dir OUT, EP number 1
            PIPESEL.write_volatile(1);
            PIPECFG.write_volatile((0b01 << 14) | (0 << 4) | 1); // BULK, DIR=OUT, EPNUM=1
            PIPEMAXP.write_volatile(64);
            PIPE1CTR.write_volatile(PID_BUF);
            // PIPE2 = bulk, dir IN, EP number 2
            PIPESEL.write_volatile(2);
            PIPECFG.write_volatile((0b01 << 14) | (1 << 4) | 2); // BULK, DIR=IN, EPNUM=2
            PIPEMAXP.write_volatile(64);
            PIPE2CTR.write_volatile(PID_NAK);
            PIPESEL.write_volatile(0);
        }
    }
}

impl UsbStack for RaUsbfsCdc {
    fn poll(&mut self) -> CdcState {
        unsafe {
            let is = INTSTS0.read_volatile();
            // bus reset / device-state change
            if is & IS_DVST != 0 {
                INTSTS0.write_volatile(!IS_DVST);
                let rhst = DVSTCTR0.read_volatile() & RHST_MASK;
                if rhst == 2 || rhst == 4 {
                    // reset detected
                    if self.stage != Stage::Configured {
                        self.stage = Stage::Reset;
                    }
                    USBADDR.write_volatile(0);
                }
            }
            // control transfer: a SETUP packet is valid
            if (is & IS_CTRT != 0) && (is & IS_VALID != 0) {
                let _stage = is & CTSQ_MASK;
                self.handle_setup();
            }
        }
        match self.stage {
            Stage::Configured => CdcState::Configured,
            Stage::Addressed => CdcState::Addressed,
            Stage::Attached | Stage::Reset => CdcState::Default,
            Stage::PoweredOff => CdcState::Disconnected,
        }
    }

    fn write(&mut self, data: &[u8]) -> usize {
        if self.stage != Stage::Configured {
            return 0;
        }
        unsafe {
            // select PIPE2 (bulk IN) on CFIFO, write direction
            CFIFOSEL.write_volatile(CF_ISEL | 2);
            while (CFIFOSEL.read_volatile() & 0xF) != 2 {}
            if CFIFOCTR.read_volatile() & CF_FRDY == 0 {
                return 0; // FIFO busy
            }
            let n = data.len().min(64);
            let mut i = 0;
            while i + 1 < n {
                CFIFO.write_volatile(u16::from(data[i]) | (u16::from(data[i + 1]) << 8));
                i += 2;
            }
            if i < n {
                CFIFO8.write_volatile(data[i]);
            }
            CFIFOCTR.write_volatile(CF_BVAL);
            PIPE2CTR.write_volatile(PID_BUF);
            n
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> usize {
        // deliver any buffered bytes first
        if self.rx_pos < self.rx_len {
            let n = (self.rx_len - self.rx_pos).min(buf.len());
            buf[..n].copy_from_slice(&self.rx[self.rx_pos..self.rx_pos + n]);
            self.rx_pos += n;
            return n;
        }
        if self.stage != Stage::Configured {
            return 0;
        }
        unsafe {
            if BRDYSTS.read_volatile() & (1 << 1) == 0 {
                return 0; // no OUT data on PIPE1
            }
            CFIFOSEL.write_volatile(CF_RCNT | 1);
            while (CFIFOSEL.read_volatile() & 0xF) != 1 {}
            if CFIFOCTR.read_volatile() & CF_FRDY == 0 {
                return 0;
            }
            let dtln = (CFIFOCTR.read_volatile() & CF_DTLN_MASK) as usize;
            let n = dtln.min(self.rx.len());
            for b in self.rx.iter_mut().take(n) {
                *b = CFIFO8.read_volatile();
            }
            CFIFOCTR.write_volatile(CF_BCLR);
            BRDYSTS.write_volatile(!(1 << 1));
            PIPE1CTR.write_volatile(PID_BUF);
            self.rx_len = n;
            self.rx_pos = 0;
            let m = n.min(buf.len());
            buf[..m].copy_from_slice(&self.rx[..m]);
            self.rx_pos = m;
            m
        }
    }

    fn configured(&self) -> bool {
        self.stage == Stage::Configured
    }

    fn backend_id(&self) -> u32 {
        backend_id::RA_USBFS
    }
}

// silence unused-const warnings for the reference bits kept for clarity
const _: () = {
    let _ = (IS_VBSTS, CF_REW, CTR_BSTS);
};
