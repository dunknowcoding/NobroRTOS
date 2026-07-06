//! ESP-free RA4M1 (Arduino UNO R4) USBFS device backend for the mountable USB stack
//! (`--features backend-ra-usbfs`). Self-contained raw-register CDC-ACM, no vendored C:
//! the register sequences are ported from the RA hardware manual and cross-checked
//! against TinyUSB's proven `dcd_rusb2` driver. Like every nobro_usb backend it presents
//! the common [`UsbStack`] trait, so the RA4M1 port reports over its own native USB
//! consistently with the nRF USBD and ESP32 USB-Serial-JTAG backends.
//!
//! The peripheral is pipe-based: PIPE0 is the default control pipe (enumeration), and we
//! use PIPE1 (bulk OUT) + PIPE2 (bulk IN) for the CDC data endpoints. Enumeration is a
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
const CFIFO: *mut u16 = (BASE + 0x14) as *mut u16;
const CFIFO8: *mut u8 = (BASE + 0x14) as *mut u8;
r16!(CFIFOSEL, 0x20);
r16!(CFIFOCTR, 0x22);
r16!(INTENB0, 0x30);
r16!(BRDYENB, 0x36);
r16!(BEMPENB, 0x3A);
r16!(INTSTS0, 0x40);
r16!(BRDYSTS, 0x46);
r16!(BEMPSTS, 0x4A);
r16!(USBREQ, 0x54);
r16!(USBVAL, 0x56);
r16!(USBINDX, 0x58);
r16!(USBLENG, 0x5A);
r16!(DCPMAXP, 0x5E);
r16!(DCPCTR, 0x60);
r16!(PIPESEL, 0x64);
r16!(PIPECFG, 0x68);
r16!(PIPEMAXP, 0x6C);
r16!(PIPEPERI, 0x6E);
const PIPE1CTR: *mut u16 = (BASE + 0x70) as *mut u16;
const PIPE2CTR: *mut u16 = (BASE + 0x72) as *mut u16;
const PIPE3CTR: *mut u16 = (BASE + 0x74) as *mut u16;
const USBMC: *mut u16 = (BASE + 0xCC) as *mut u16;
const PHYSLEW: *mut u32 = (BASE + 0xF0) as *mut u32;
const DPUSR0R_FS: *mut u32 = (BASE + 0x400) as *mut u32;

const MSTPCRB: *mut u32 = 0x4004_7000 as *mut u32;
const PRCR: *mut u16 = 0x4001_E3FE as *mut u16;

// SYSCFG bits
const SYSCFG_SCKE: u16 = 1 << 10;
const SYSCFG_DCFM: u16 = 1 << 6;
const SYSCFG_DPRPU: u16 = 1 << 4;
const SYSCFG_USBE: u16 = 1 << 0;
const SYSCFG_DRPD: u16 = 1 << 5;
// INTENB0 bits
const IE_DVSE: u16 = 1 << 12;
const IE_CTRE: u16 = 1 << 11;
const IE_BEMPE: u16 = 1 << 10;
const IE_BRDYE: u16 = 1 << 8;
// INTSTS0 bits
const IS_VBSTS: u16 = 1 << 7;
const IS_DVST: u16 = 1 << 12;
const IS_CTRT: u16 = 1 << 11;
const IS_VALID: u16 = 1 << 3;
const DVSQ_MASK: u16 = 0x70;
const DVSQ_DEFAULT: u16 = 0x10;
const DVSQ_ADDRESSED: u16 = 0x20;
// CFIFOSEL / CFIFOCTR
const CF_RCNT: u16 = 1 << 15;
const CF_REW: u16 = 1 << 14;
const CF_ISEL: u16 = 1 << 5;
const CF_MBW_16: u16 = 1 << 10;
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
const VID: u16 = 0x1209;
const PID: u16 = 0x0004;

#[rustfmt::skip]
const DEV_DESC: [u8; 18] = [
    18, 1, 0x00, 0x02, 0, 0, 0, 64, // USB2.0, interface-owned classes, EP0 = 64
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
    ctrl_data: &'static [u8],
    ctrl_len: usize,
    ctrl_pos: usize,
}

impl RaUsbfsCdc {
    pub fn mount(cfg: &UsbConfig) -> Self {
        let mut controller_ready = false;
        unsafe {
            // MSTPCRB is protected by PRCR.PRC1. Writes made while it is locked are
            // silently ignored, leaving USBFS stopped and SCKE permanently clear.
            PRCR.write_volatile(0xA502);
            MSTPCRB.write_volatile(MSTPCRB.read_volatile() & !((1 << 11) | (1 << 12)));
            PRCR.write_volatile(0xA500);
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
            // USBMC bit 1 is reserved but must be written as one. Match Arduino's
            // post-init hook by setting VDCEN while preserving the register image.
            USBMC.write_volatile(USBMC.read_volatile() | (1 << 7));
            for _ in 0..50_000 {
                core::hint::spin_loop();
            }
            SYSCFG.write_volatile(SYSCFG_SCKE);
            for _ in 0..100_000 {
                if SYSCFG.read_volatile() & SYSCFG_SCKE != 0 {
                    controller_ready = true;
                    break;
                }
            }
            if controller_ready {
                SYSCFG.write_volatile(
                    (SYSCFG.read_volatile() & !SYSCFG_DRPD & !SYSCFG_DCFM) | SYSCFG_USBE,
                );
                DCPMAXP.write_volatile(64); // EP0 max packet
                PHYSLEW.write_volatile(0x5);
                DPUSR0R_FS.write_volatile(DPUSR0R_FS.read_volatile() & !(1 << 4));
                INTSTS0.write_volatile(0);
                BRDYSTS.write_volatile(0);
                BEMPSTS.write_volatile(0);
                BEMPENB.write_volatile(1); // PIPE0 empty status drives long descriptors
                BRDYENB.write_volatile(1); // PIPE0 ready status drives setup/data transitions
                INTENB0.write_volatile(IE_DVSE | IE_CTRE | IE_BEMPE | IE_BRDYE);
                DCPCTR.write_volatile(PID_BUF);
                // Assert the D+ pull-up so the host begins enumeration.
                SYSCFG.write_volatile(SYSCFG.read_volatile() | SYSCFG_DPRPU);
            }
        }
        RaUsbfsCdc {
            _cfg: *cfg,
            stage: if controller_ready {
                Stage::Attached
            } else {
                Stage::PoweredOff
            },
            pending_addr: 0,
            rx: [0; 64],
            rx_len: 0,
            rx_pos: 0,
            ctrl_data: &[],
            ctrl_len: 0,
            ctrl_pos: 0,
        }
    }

    /// How far enumeration has progressed - blink this on the LED when USB is silent.
    pub fn stage(&self) -> Stage {
        self.stage
    }

    // ---- control pipe (PIPE0) helpers via CFIFO ----

    fn ctrl_write_packet(&self, data: &[u8]) -> bool {
        unsafe {
            CFIFOSEL.write_volatile(CF_ISEL | CF_MBW_16); // DCP, write direction, 16-bit FIFO
            let mut selected = false;
            for _ in 0..10_000 {
                let sel = CFIFOSEL.read_volatile();
                if (sel & CF_ISEL != 0) && (sel & CF_MBW_16 != 0) {
                    selected = true;
                    break;
                }
            }
            if !selected {
                return false;
            }
            CFIFOCTR.write_volatile(CF_BCLR); // clear buffer
            let mut ready = false;
            for _ in 0..10_000 {
                if CFIFOCTR.read_volatile() & CF_FRDY != 0 {
                    ready = true;
                    break;
                }
            }
            if !ready {
                return false;
            }
            let mut i = 0;
            while i + 1 < data.len() {
                CFIFO.write_volatile(u16::from(data[i]) | (u16::from(data[i + 1]) << 8));
                i += 2;
            }
            if i < data.len() {
                CFIFO8.write_volatile(data[i]);
            }
            if data.len() < 64 {
                CFIFOCTR.write_volatile(CF_BVAL); // short packet terminates the control data stage
            }
            true
        }
    }

    fn ctrl_start(&mut self, data: &'static [u8], len: usize) {
        unsafe { BEMPSTS.write_volatile(!1) };
        self.ctrl_data = data;
        self.ctrl_len = len.min(data.len());
        self.ctrl_pos = 0;
        self.ctrl_continue();
        unsafe {
            DCPCTR.write_volatile(PID_BUF);
        }
    }

    fn ctrl_continue(&mut self) {
        if self.ctrl_pos >= self.ctrl_len {
            return;
        }
        let end = (self.ctrl_pos + 64).min(self.ctrl_len);
        if self.ctrl_write_packet(&self.ctrl_data[self.ctrl_pos..end]) {
            self.ctrl_pos = end;
        }
    }

    /// Complete the control transfer status stage.
    fn ctrl_status_done(&self) {
        unsafe {
            DCPCTR.write_volatile(CTR_CCPL | PID_BUF);
        }
    }

    fn handle_setup(&mut self) {
        let (req, val, _idx, len) = unsafe {
            (
                USBREQ.read_volatile(),
                USBVAL.read_volatile(),
                USBINDX.read_volatile(),
                USBLENG.read_volatile(),
            )
        };
        let bm = (req & 0xFF) as u8;
        let brequest = (req >> 8) as u8;
        unsafe { CFIFOCTR.write_volatile(CF_BCLR) };
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
                    self.ctrl_start(resp, len as usize);
                }
            }
            // Standard device status/configuration queries used during enumeration.
            (0x80, 0x00) => self.ctrl_start(&[0, 0], len as usize),
            (0x80, 0x08) => {
                let configured: &'static [u8] = if self.stage == Stage::Configured {
                    &[1]
                } else {
                    &[0]
                };
                self.ctrl_start(configured, len as usize);
            }
            (0x81, 0x0A) => self.ctrl_start(&[0], len as usize),
            // CDC GET_LINE_CODING: 115200, 8 data bits, no parity, one stop bit.
            (0xA1, 0x21) => self.ctrl_start(&[0x00, 0xC2, 0x01, 0x00, 0, 0, 8], len as usize),
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
            // PIPE3 = interrupt IN, EP3 (CDC notifications). It remains NAK until a
            // notification is queued, but must exist because the descriptor advertises it.
            PIPESEL.write_volatile(3);
            PIPECFG.write_volatile((0b10 << 14) | (1 << 4) | 3);
            PIPEMAXP.write_volatile(8);
            PIPEPERI.write_volatile(16);
            PIPE3CTR.write_volatile(PID_NAK);
            PIPESEL.write_volatile(0);
        }
    }
}

impl UsbStack for RaUsbfsCdc {
    fn poll(&mut self) -> CdcState {
        unsafe {
            let is = INTSTS0.read_volatile();
            // Device-state transitions carry the authoritative reset/address state in
            // INTSTS0.DVSQ. DVSTCTR0.RHST is a host-controller field.
            if is & IS_DVST != 0 {
                INTSTS0.write_volatile(!IS_DVST);
                match is & DVSQ_MASK {
                    DVSQ_DEFAULT => {
                        self.stage = Stage::Reset;
                        self.ctrl_data = &[];
                        self.ctrl_len = 0;
                        self.ctrl_pos = 0;
                        DCPCTR.write_volatile(PID_BUF);
                    }
                    DVSQ_ADDRESSED => self.stage = Stage::Addressed,
                    _ => {}
                }
            }
            // VALID marks a received setup packet. Do not filter by CTSQ here:
            // Windows may issue descriptor/status requests while the controller reports
            // a different control-transfer phase, and the request registers still hold
            // the authoritative setup packet.
            if (is & IS_CTRT != 0) && (is & IS_VALID != 0) {
                INTSTS0.write_volatile(!IS_CTRT);
                self.handle_setup();
            }
            // EP0 can hold one 64-byte packet. Continue long descriptors when the
            // previous packet leaves the control FIFO.
            if BEMPSTS.read_volatile() & 1 != 0 {
                BEMPSTS.write_volatile(!1);
                self.ctrl_continue();
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
            let mut selected = false;
            for _ in 0..10_000 {
                if (CFIFOSEL.read_volatile() & 0xF) == 2 {
                    selected = true;
                    break;
                }
            }
            if !selected {
                return 0;
            }
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
            let mut selected = false;
            for _ in 0..10_000 {
                if (CFIFOSEL.read_volatile() & 0xF) == 1 {
                    selected = true;
                    break;
                }
            }
            if !selected {
                return 0;
            }
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
