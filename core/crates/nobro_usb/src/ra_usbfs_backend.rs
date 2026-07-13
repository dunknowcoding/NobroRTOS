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

use crate::{backend_id, CdcState, UsbBackendError, UsbConfig, UsbStack};

const BASE: usize = 0x4009_0000;
macro_rules! r16 {
    ($name:ident, $off:expr) => {
        const $name: *mut u16 = (BASE + $off) as *mut u16;
    };
}
r16!(SYSCFG, 0x00);
const CFIFO: *mut u16 = (BASE + 0x14) as *mut u16;
const CFIFO8: *mut u8 = (BASE + 0x14) as *mut u8;
const D0FIFO: *mut u16 = (BASE + 0x18) as *mut u16;
const D0FIFO8: *mut u8 = (BASE + 0x18) as *mut u8;
r16!(CFIFOSEL, 0x20);
r16!(CFIFOCTR, 0x22);
r16!(D0FIFOSEL, 0x28);
r16!(D0FIFOCTR, 0x2A);
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
const PIPE6CTR: *mut u16 = (BASE + 0x7A) as *mut u16;
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
const IE_VBSE: u16 = 1 << 15;
const IE_RSME: u16 = 1 << 14;
const IE_SOFE: u16 = 1 << 13;
// INTSTS0 bits
const IS_VBINT: u16 = 1 << 15;
const IS_RESM: u16 = 1 << 14;
const IS_SOFR: u16 = 1 << 13;
const IS_VBSTS: u16 = 1 << 7;
const IS_DVST: u16 = 1 << 12;
const IS_CTRT: u16 = 1 << 11;
const IS_VALID: u16 = 1 << 3;
const DVSQ_MASK: u16 = 0x70;
const DVSQ_DEFAULT: u16 = 0x10;
const DVSQ_ADDRESSED: u16 = 0x20;
const DVSQ_SUSPENDED_0: u16 = 0x40;
const DVSQ_SUSPENDED_1: u16 = 0x50;
const DVSQ_SUSPENDED_2: u16 = 0x60;
const DVSQ_SUSPENDED_3: u16 = 0x70;
// CFIFOSEL / CFIFOCTR
const CF_RCNT: u16 = 1 << 15;
const CF_REW: u16 = 1 << 14;
const CF_ISEL: u16 = 1 << 5;
const CF_MBW_16: u16 = 1 << 10;
const CF_MBW_MASK: u16 = 0b11 << 10;
const CF_BVAL: u16 = 1 << 15;
const CF_BCLR: u16 = 1 << 14;
const CF_FRDY: u16 = 1 << 13;
const CF_DTLN_MASK: u16 = 0xFFF;
// DCPCTR / PIPExCTR
const CTR_CCPL: u16 = 1 << 2;
const PID_BUF: u16 = 1;
const PID_NAK: u16 = 0;
const PID_STALL: u16 = 2;
const PID_MASK: u16 = 0x3;
const CTR_BSTS: u16 = 1 << 7;
const CTR_PBUSY: u16 = 1 << 5;
const CTR_SQCLR: u16 = 1 << 8;
const CTR_ACLRM: u16 = 1 << 9;
const PIPECFG_SHTNAK: u16 = 1 << 7;
const PIPE_MAX_PACKET_MASK: u16 = 0x07ff;
const PIPE_INTERVAL_MASK: u16 = 0x0007;

const FIFO_SELECT_SPINS: usize = 10_000;
const PIPE_IDLE_SPINS: usize = 10_000;
const CTRL_BUFFER_CAPACITY: usize = CFG_DESC.len();
const PIPE1_BULK_OUT_CFG: u16 = (0b01 << 14) | PIPECFG_SHTNAK | 1;
const PIPE2_BULK_IN_CFG: u16 = (0b01 << 14) | PIPECFG_SHTNAK | (1 << 4) | 2;
const NOTIFY_INTERRUPT_IN_CFG: u16 = (0b10 << 14) | (1 << 4) | 3;
// RUSB2 encodes the descriptor's full-speed bInterval=16 as log2(16)=4 in IITV.
const PIPE3_INTERVAL: u16 = 4;
const DEFAULT_LINE_CODING: [u8; 7] = [0x00, 0xC2, 0x01, 0x00, 0, 0, 8];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlState {
    Default,
    Addressed,
    Configured,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SetupPacket {
    request_type: u8,
    request: u8,
    value: u16,
    index: u16,
    length: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Descriptor {
    Device,
    Configuration,
    LanguageIds,
    Manufacturer,
    Product,
    Serial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetupAction {
    Descriptor(Descriptor, usize),
    DeviceStatus,
    InterfaceStatus,
    EndpointStatus(u16),
    GetConfiguration,
    GetInterface,
    GetLineCoding,
    SetAddress(u8),
    SetConfiguration(u8),
    ReceiveLineCoding,
    SetControlLineState,
    SendBreak,
    SetInterface,
    ClearEndpointHalt(u16),
    SetEndpointHalt(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BackendFault {
    latched: Option<UsbBackendError>,
}

impl BackendFault {
    const fn for_startup(controller_ready: bool) -> Self {
        Self {
            latched: if controller_ready {
                None
            } else {
                Some(UsbBackendError::StartupTimeout)
            },
        }
    }

    fn latch(&mut self, error: UsbBackendError) {
        if self.latched.is_none() {
            self.latched = Some(error);
        }
    }

    fn check(self) -> Result<(), UsbBackendError> {
        self.latched.map_or(Ok(()), Err)
    }
}

/// Progress markers so a silent board can signal where enumeration stalled.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    PoweredOff = 0,
    Attached = 1,   // pull-up asserted, waiting for reset
    Reset = 2,      // bus reset seen
    Addressed = 3,  // SET_ADDRESS handled
    Configured = 4, // SET_CONFIGURATION handled - CDC usable
    Suspended = 5,  // attached but host-suspended - CDC is not usable
}

// ---- USB descriptors (a minimal single-CDC-ACM device) ----
const VID: u16 = 0x1209;
const PID: u16 = 0x0004;

/// Fixed identity of the self-contained RA4M1 descriptor implementation.
///
/// Unlike the nRF backend, the allocation-free raw-register backend stores descriptors
/// in flash. Passing a different identity is rejected instead of silently advertising
/// strings that differ from [`UsbConfig`].
pub const RA4M1_USB_CONFIG: UsbConfig =
    UsbConfig::new(VID, PID, "NobroRTOS", "NobroRTOS RA4M1", "NOBRORA4");

#[rustfmt::skip]
const DEV_DESC: [u8; 18] = [
    18, 1, 0x00, 0x02, 0, 0, 0, 64, // USB2.0, interface-owned classes, EP0 = 64
    (VID & 0xFF) as u8, (VID >> 8) as u8,
    (PID & 0xFF) as u8, (PID >> 8) as u8,
    0x00, 0x01, 1, 2, 3, 1, // bcdDevice, iMfr, iProduct, iSerial, numConfigs
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

pub(crate) struct RaUsbfsCdc {
    // A failed SCKE bring-up is permanent for this mounted instance. In particular,
    // seeing VBUS later must not assert the pull-up on an unclocked controller.
    controller_ready: bool,
    fault: BackendFault,
    link_enabled: bool,
    stage: Stage,
    resume_stage: Stage,
    pending_addr: u8,
    rx: [u8; 64],
    rx_len: usize,
    rx_pos: usize,
    ctrl_data: [u8; CTRL_BUFFER_CAPACITY],
    ctrl_len: usize,
    ctrl_pos: usize,
    pending_line_coding: bool,
    line_coding: [u8; 7],
    tx_pending: bool,
}

impl RaUsbfsCdc {
    pub(crate) fn mount(_cfg: &UsbConfig) -> Self {
        // `try_mount` checks the exact fixed-descriptor policy before claiming the
        // singleton or entering this private constructor. Keep this path panic-free so
        // a configuration error is always returned as UsbMountError to the caller.
        let mut controller_ready = false;
        let mut vbus_present = false;
        unsafe {
            // MSTPCRB is protected by PRCR.PRC1. Writes made while it is locked are
            // silently ignored, leaving USBFS stopped and SCKE permanently clear.
            PRCR.write_volatile(0xA502);
            // RA4M1 exposes the full-speed controller at channel 0 / MSTPB11. Do not
            // wake the distinct high-speed channel assigned to MSTPB12 on larger MCUs.
            MSTPCRB.write_volatile(MSTPCRB.read_volatile() & !(1 << 11));
            PRCR.write_volatile(0xA500);
            // Force a disconnect edge first: the bootloader may have left its own CDC
            // enumerated, so drop the D+ pull-up and hold long enough for the host to
            // notice the unplug before we re-enumerate as our device. (This drop is also
            // the execution signal; if the host's port vanishes, our
            // code is definitely running on the RA4M1.)
            SYSCFG.write_volatile(SYSCFG_USBE); // USBE on, DPRPU off
                                                // This is an instruction-budgeted minimum disconnect hold, not a measured
                                                // deadline. The RA port's user-visible recovery deadline is clock-based.
            let mut spin = 0u32;
            while spin < 4_000_000 {
                core::hint::spin_loop();
                spin += 1;
            }
            // Device-controller bring-up (dcd_init, full-speed path). Match the UNO R4
            // variant post-init hook by setting USBMC.VDCEN while preserving VDDUSBE.
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
                // Data-pipe sources are enabled only after SET_CONFIGURATION opens the
                // corresponding pipes. PIPE0 remains live for enumeration.
                BEMPENB.write_volatile(1);
                BRDYENB.write_volatile(1);
                INTENB0.write_volatile(IE_VBSE | IE_RSME | IE_DVSE | IE_CTRE | IE_BEMPE | IE_BRDYE);
                DCPCTR.write_volatile(PID_BUF);
                vbus_present = INTSTS0.read_volatile() & IS_VBSTS != 0;
                if vbus_present {
                    // Assert D+ only while VBUS is live.
                    SYSCFG.write_volatile(SYSCFG.read_volatile() | SYSCFG_DPRPU);
                }
            }
        }
        RaUsbfsCdc {
            controller_ready,
            fault: BackendFault::for_startup(controller_ready),
            link_enabled: true,
            stage: if Self::may_attach(controller_ready, true, vbus_present) {
                Stage::Attached
            } else {
                Stage::PoweredOff
            },
            resume_stage: Stage::Attached,
            pending_addr: 0,
            rx: [0; 64],
            rx_len: 0,
            rx_pos: 0,
            ctrl_data: [0; CTRL_BUFFER_CAPACITY],
            ctrl_len: 0,
            ctrl_pos: 0,
            pending_line_coding: false,
            line_coding: DEFAULT_LINE_CODING,
            tx_pending: false,
        }
    }

    fn may_attach(controller_ready: bool, link_enabled: bool, vbus_present: bool) -> bool {
        controller_ready && link_enabled && vbus_present
    }

    fn validated_line_coding(bytes: &[u8]) -> Option<[u8; 7]> {
        bytes.try_into().ok()
    }

    fn control_state(&self) -> ControlState {
        match self.stage {
            Stage::Reset => ControlState::Default,
            Stage::Addressed => ControlState::Addressed,
            Stage::Configured => ControlState::Configured,
            Stage::PoweredOff | Stage::Attached | Stage::Suspended => ControlState::Unavailable,
        }
    }

    fn descriptor_for(value: u16, language: u16) -> Option<Descriptor> {
        match ((value >> 8) as u8, value as u8, language) {
            (1, 0, 0) => Some(Descriptor::Device),
            (2, 0, 0) => Some(Descriptor::Configuration),
            (3, 0, 0) => Some(Descriptor::LanguageIds),
            (3, 1, 0x0409) => Some(Descriptor::Manufacturer),
            (3, 2, 0x0409) => Some(Descriptor::Product),
            (3, 3, 0x0409) => Some(Descriptor::Serial),
            _ => None,
        }
    }

    fn descriptor_bytes(descriptor: Descriptor) -> &'static [u8] {
        match descriptor {
            Descriptor::Device => &DEV_DESC,
            Descriptor::Configuration => &CFG_DESC,
            Descriptor::LanguageIds => Self::string_desc(0).expect("language descriptor"),
            Descriptor::Manufacturer => Self::string_desc(1).expect("manufacturer descriptor"),
            Descriptor::Product => Self::string_desc(2).expect("product descriptor"),
            Descriptor::Serial => Self::string_desc(3).expect("serial descriptor"),
        }
    }

    fn interface_exists(index: u16) -> bool {
        matches!(index, 0 | 1)
    }

    fn validate_setup(packet: SetupPacket, state: ControlState) -> Option<SetupAction> {
        use ControlState::{Addressed, Configured, Default};

        let action = match (packet.request_type, packet.request) {
            (0x80, 0x06) if matches!(state, Default | Addressed | Configured) => {
                SetupAction::Descriptor(
                    Self::descriptor_for(packet.value, packet.index)?,
                    usize::from(packet.length),
                )
            }
            (0x80, 0x00)
                if matches!(state, Addressed | Configured)
                    && packet.value == 0
                    && packet.index == 0
                    && packet.length == 2 =>
            {
                SetupAction::DeviceStatus
            }
            (0x81, 0x00)
                if state == Configured
                    && packet.value == 0
                    && Self::interface_exists(packet.index)
                    && packet.length == 2 =>
            {
                SetupAction::InterfaceStatus
            }
            (0x82, 0x00)
                if packet.value == 0
                    && packet.length == 2
                    && ((state == Addressed && matches!(packet.index, 0x00 | 0x80))
                        || (state == Configured
                            && Self::endpoint_pipe(packet.index).is_some())) =>
            {
                SetupAction::EndpointStatus(packet.index)
            }
            (0x80, 0x08)
                if matches!(state, Addressed | Configured)
                    && packet.value == 0
                    && packet.index == 0
                    && packet.length == 1 =>
            {
                SetupAction::GetConfiguration
            }
            (0x81, 0x0a)
                if state == Configured
                    && packet.value == 0
                    && Self::interface_exists(packet.index)
                    && packet.length == 1 =>
            {
                SetupAction::GetInterface
            }
            (0xa1, 0x21)
                if state == Configured
                    && packet.value == 0
                    && packet.index == 0
                    && packet.length == 7 =>
            {
                SetupAction::GetLineCoding
            }
            (0x00, 0x05)
                if matches!(state, Default | Addressed)
                    && packet.value <= 127
                    && packet.index == 0
                    && packet.length == 0 =>
            {
                SetupAction::SetAddress(packet.value as u8)
            }
            (0x00, 0x09)
                if matches!(state, Addressed | Configured)
                    && packet.value <= 1
                    && packet.index == 0
                    && packet.length == 0 =>
            {
                SetupAction::SetConfiguration(packet.value as u8)
            }
            (0x21, 0x20)
                if state == Configured
                    && packet.value == 0
                    && packet.index == 0
                    && packet.length == 7 =>
            {
                SetupAction::ReceiveLineCoding
            }
            (0x21, 0x22)
                if state == Configured
                    && packet.value & !0x0003 == 0
                    && packet.index == 0
                    && packet.length == 0 =>
            {
                SetupAction::SetControlLineState
            }
            (0x21, 0x23) if state == Configured && packet.index == 0 && packet.length == 0 => {
                SetupAction::SendBreak
            }
            (0x01, 0x0b)
                if state == Configured
                    && packet.value == 0
                    && Self::interface_exists(packet.index)
                    && packet.length == 0 =>
            {
                SetupAction::SetInterface
            }
            (0x02, 0x01)
                if state == Configured
                    && packet.value == 0
                    && packet.length == 0
                    && Self::haltable_pipe(packet.index).is_some() =>
            {
                SetupAction::ClearEndpointHalt(packet.index)
            }
            (0x02, 0x03)
                if state == Configured
                    && packet.value == 0
                    && packet.length == 0
                    && Self::haltable_pipe(packet.index).is_some() =>
            {
                SetupAction::SetEndpointHalt(packet.index)
            }
            _ => return None,
        };
        Some(action)
    }

    fn resume_stage_for(dvsq: u16, current: Stage) -> Stage {
        match dvsq {
            DVSQ_SUSPENDED_0 => Stage::Attached,
            DVSQ_SUSPENDED_1 => Stage::Reset,
            DVSQ_SUSPENDED_2 => Stage::Addressed,
            DVSQ_SUSPENDED_3 => Stage::Configured,
            _ => current,
        }
    }

    /// How far enumeration has progressed - blink this on the LED when USB is silent.
    pub(crate) fn stage(&self) -> Stage {
        self.stage
    }

    // ---- control pipe (PIPE0) helpers via CFIFO ----

    fn ctrl_write_packet(&self, data: &[u8]) -> Result<(), UsbBackendError> {
        unsafe {
            CFIFOSEL.write_volatile(CF_ISEL | CF_MBW_16); // DCP, write direction, 16-bit FIFO
            let mut selected = false;
            for _ in 0..FIFO_SELECT_SPINS {
                let sel = CFIFOSEL.read_volatile();
                if sel & (0xF | CF_ISEL | CF_MBW_MASK) == (CF_ISEL | CF_MBW_16) {
                    selected = true;
                    break;
                }
            }
            if !selected {
                return Err(UsbBackendError::ControllerTimeout);
            }
            CFIFOCTR.write_volatile(CF_BCLR); // clear buffer
            let mut ready = false;
            for _ in 0..FIFO_SELECT_SPINS {
                if CFIFOCTR.read_volatile() & CF_FRDY != 0 {
                    ready = true;
                    break;
                }
            }
            if !ready {
                return Err(UsbBackendError::ControllerTimeout);
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
            Ok(())
        }
    }

    fn ctrl_start(&mut self, data: &[u8], len: usize) -> Result<(), UsbBackendError> {
        unsafe { BEMPSTS.write_volatile(!1) };
        self.ctrl_len = len.min(data.len()).min(self.ctrl_data.len());
        self.ctrl_data[..self.ctrl_len].copy_from_slice(&data[..self.ctrl_len]);
        self.ctrl_pos = 0;
        self.ctrl_continue()?;
        unsafe {
            DCPCTR.write_volatile(PID_BUF);
        }
        Ok(())
    }

    fn ctrl_continue(&mut self) -> Result<(), UsbBackendError> {
        if self.ctrl_pos >= self.ctrl_len {
            return Ok(());
        }
        let end = (self.ctrl_pos + 64).min(self.ctrl_len);
        self.ctrl_write_packet(&self.ctrl_data[self.ctrl_pos..end])?;
        self.ctrl_pos = end;
        Ok(())
    }

    /// Complete the control transfer status stage.
    fn ctrl_status_done(&self) {
        unsafe {
            DCPCTR.write_volatile(CTR_CCPL | PID_BUF);
        }
    }

    fn ctrl_stall(&mut self) -> Result<(), UsbBackendError> {
        self.ctrl_len = 0;
        self.ctrl_pos = 0;
        self.pending_line_coding = false;
        unsafe {
            // RUSB2 requires the legal PID transition through STALL2 when the pipe was
            // BUF. A single direct BUF -> STALL write is not a supported transition.
            let pid = DCPCTR.read_volatile() & 0x3;
            DCPCTR.write_volatile(pid | PID_STALL);
            DCPCTR.write_volatile(PID_STALL);
        }
        Self::wait_pipe_pid(DCPCTR, PID_STALL)
    }

    fn handle_setup(&mut self) -> Result<(), UsbBackendError> {
        let packet = unsafe {
            let req = USBREQ.read_volatile();
            SetupPacket {
                request_type: req as u8,
                request: (req >> 8) as u8,
                value: USBVAL.read_volatile(),
                index: USBINDX.read_volatile(),
                length: USBLENG.read_volatile(),
            }
        };
        unsafe {
            CFIFOCTR.write_volatile(CF_BCLR);
            INTSTS0.write_volatile(!IS_VALID);
        }

        let Some(action) = Self::validate_setup(packet, self.control_state()) else {
            return self.ctrl_stall();
        };
        match action {
            SetupAction::Descriptor(descriptor, requested) => {
                self.ctrl_start(Self::descriptor_bytes(descriptor), requested)?;
            }
            SetupAction::DeviceStatus | SetupAction::InterfaceStatus => {
                self.ctrl_start(&[0, 0], 2)?;
            }
            SetupAction::EndpointStatus(index) => {
                // Validation guarantees that this endpoint is present in this state.
                let halted = self
                    .endpoint_halted(index)
                    .ok_or(UsbBackendError::InvalidState)?;
                self.ctrl_start(&[u8::from(halted), 0], 2)?;
            }
            SetupAction::GetConfiguration => {
                self.ctrl_start(&[u8::from(self.stage == Stage::Configured)], 1)?;
            }
            SetupAction::GetInterface => self.ctrl_start(&[0], 1)?,
            SetupAction::GetLineCoding => {
                let current = self.line_coding;
                self.ctrl_start(&current, current.len())?;
            }
            SetupAction::SetAddress(address) => {
                self.pending_addr = address;
                self.stage = if address == 0 {
                    Stage::Reset
                } else {
                    Stage::Addressed
                };
                self.ctrl_status_done();
            }
            SetupAction::SetConfiguration(0) => {
                self.close_data_pipes()?;
                self.stage = Stage::Addressed;
                self.ctrl_status_done();
            }
            SetupAction::SetConfiguration(1) => {
                self.open_data_pipes()?;
                self.stage = Stage::Configured;
                self.ctrl_status_done();
            }
            SetupAction::SetConfiguration(_) => unreachable!("validated configuration"),
            SetupAction::ReceiveLineCoding => {
                self.pending_line_coding = true;
                unsafe { DCPCTR.write_volatile(PID_BUF) };
            }
            SetupAction::SetControlLineState
            | SetupAction::SendBreak
            | SetupAction::SetInterface => self.ctrl_status_done(),
            SetupAction::ClearEndpointHalt(index) => {
                if !self.clear_endpoint_halt(index)? {
                    return Err(UsbBackendError::InvalidState);
                }
                self.ctrl_status_done();
            }
            SetupAction::SetEndpointHalt(index) => {
                if !self.set_endpoint_halt(index)? {
                    return Err(UsbBackendError::InvalidState);
                }
                self.ctrl_status_done();
            }
        }
        Ok(())
    }

    fn string_desc(index: u8) -> Option<&'static [u8]> {
        // 0 = langid (en-US), then the exact identity in RA4M1_USB_CONFIG.
        const LANGID: [u8; 4] = [4, 3, 0x09, 0x04];
        const MFR: [u8; 20] = [
            20, 3, b'N', 0, b'o', 0, b'b', 0, b'r', 0, b'o', 0, b'R', 0, b'T', 0, b'O', 0, b'S', 0,
        ];
        const PROD: [u8; 32] = [
            32, 3, b'N', 0, b'o', 0, b'b', 0, b'r', 0, b'o', 0, b'R', 0, b'T', 0, b'O', 0, b'S', 0,
            b' ', 0, b'R', 0, b'A', 0, b'4', 0, b'M', 0, b'1', 0,
        ];
        const SERIAL: [u8; 18] = [
            18, 3, b'N', 0, b'O', 0, b'B', 0, b'R', 0, b'O', 0, b'R', 0, b'A', 0, b'4', 0,
        ];
        match index {
            0 => Some(&LANGID),
            1 => Some(&MFR),
            2 => Some(&PROD),
            3 => Some(&SERIAL),
            _ => None,
        }
    }

    fn receive_line_coding(&mut self) -> Result<(), UsbBackendError> {
        unsafe {
            CFIFOSEL.write_volatile(CF_RCNT); // DCP, OUT/read direction, 8-bit FIFO
            let mut selected = false;
            for _ in 0..FIFO_SELECT_SPINS {
                let select = CFIFOSEL.read_volatile();
                if select & (CF_ISEL | 0xF) == 0 {
                    selected = true;
                    break;
                }
            }
            if !selected {
                return Err(UsbBackendError::ControllerTimeout);
            }
            let mut ready = false;
            for _ in 0..FIFO_SELECT_SPINS {
                if CFIFOCTR.read_volatile() & CF_FRDY != 0 {
                    ready = true;
                    break;
                }
                core::hint::spin_loop();
            }
            if !ready {
                return Err(UsbBackendError::ControllerTimeout);
            }
            let count = (CFIFOCTR.read_volatile() & CF_DTLN_MASK) as usize;
            let mut candidate = [0u8; 7];
            for index in 0..count {
                let byte = CFIFO8.read_volatile();
                if let Some(slot) = candidate.get_mut(index) {
                    *slot = byte;
                }
            }
            CFIFOCTR.write_volatile(CF_BCLR);
            BRDYSTS.write_volatile(!1);
            self.pending_line_coding = false;
            let retained = (count == candidate.len())
                .then(|| Self::validated_line_coding(&candidate))
                .flatten();
            if let Some(line_coding) = retained {
                self.line_coding = line_coding;
                self.ctrl_status_done();
            } else {
                self.ctrl_stall()?;
            }
        }
        Ok(())
    }

    fn wait_pipe_idle(ctr: *mut u16) -> Result<(), UsbBackendError> {
        for _ in 0..PIPE_IDLE_SPINS {
            if unsafe { ctr.read_volatile() } & CTR_PBUSY == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(UsbBackendError::ControllerTimeout)
    }

    fn wait_pipe_pid(ctr: *mut u16, expected_pid: u16) -> Result<(), UsbBackendError> {
        for _ in 0..PIPE_IDLE_SPINS {
            if unsafe { ctr.read_volatile() } & PID_MASK == expected_pid {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(UsbBackendError::ControllerTimeout)
    }

    fn deselect_d0_fifo() -> Result<(), UsbBackendError> {
        unsafe {
            D0FIFOSEL.write_volatile(0);
            for _ in 0..FIFO_SELECT_SPINS {
                if D0FIFOSEL.read_volatile() & 0xF == 0 {
                    return Ok(());
                }
                core::hint::spin_loop();
            }
        }
        Err(UsbBackendError::ControllerTimeout)
    }

    fn select_d0_fifo(pipe: u16, mode: u16) -> Result<(), UsbBackendError> {
        unsafe {
            D0FIFOSEL.write_volatile(mode | pipe);
            for _ in 0..FIFO_SELECT_SPINS {
                let selected = D0FIFOSEL.read_volatile();
                if selected & (0xF | CF_MBW_MASK) == (pipe | (mode & CF_MBW_MASK)) {
                    return Ok(());
                }
                core::hint::spin_loop();
            }
            let _ = Self::deselect_d0_fifo();
        }
        Err(UsbBackendError::ControllerTimeout)
    }

    fn select_pipe(pipe: u16) -> Result<(), UsbBackendError> {
        unsafe {
            PIPESEL.write_volatile(pipe);
            for _ in 0..FIFO_SELECT_SPINS {
                if PIPESEL.read_volatile() & 0x0f == pipe {
                    return Ok(());
                }
                core::hint::spin_loop();
            }
        }
        Err(UsbBackendError::ControllerTimeout)
    }

    fn wait_pipe_config(
        expected_cfg: u16,
        expected_max_packet: Option<u16>,
        expected_interval: Option<u16>,
    ) -> Result<(), UsbBackendError> {
        for _ in 0..FIFO_SELECT_SPINS {
            let matches = unsafe {
                PIPECFG.read_volatile() == expected_cfg
                    && expected_max_packet.is_none_or(|expected| {
                        PIPEMAXP.read_volatile() & PIPE_MAX_PACKET_MASK == expected
                    })
                    && expected_interval.is_none_or(|expected| {
                        PIPEPERI.read_volatile() & PIPE_INTERVAL_MASK == expected
                    })
            };
            if matches {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(UsbBackendError::ControllerTimeout)
    }

    fn wait_d0_fifo_ready() -> Result<(), UsbBackendError> {
        for _ in 0..FIFO_SELECT_SPINS {
            if unsafe { D0FIFOCTR.read_volatile() } & CF_FRDY != 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(UsbBackendError::ControllerTimeout)
    }

    fn close_pipe(pipe: u16, ctr: *mut u16) -> Result<(), UsbBackendError> {
        unsafe { ctr.write_volatile(PID_NAK) };
        Self::wait_pipe_idle(ctr)?;
        unsafe {
            Self::select_pipe(pipe)?;
            PIPECFG.write_volatile(0);
            // Toggle ACLRM to clear any old FIFO allocation and force DATA0.
            ctr.write_volatile(CTR_ACLRM | CTR_SQCLR);
            ctr.write_volatile(0);
        }
        Self::wait_pipe_config(0, None, None)?;
        Ok(())
    }

    fn reset_software_data_state(&mut self) {
        self.rx_len = 0;
        self.rx_pos = 0;
        self.tx_pending = false;
    }

    fn close_data_pipes(&mut self) -> Result<(), UsbBackendError> {
        unsafe {
            BRDYENB.write_volatile(BRDYENB.read_volatile() & !(1 << 1));
            BEMPENB.write_volatile(BEMPENB.read_volatile() & !(1 << 2));
        }
        Self::deselect_d0_fifo()?;
        Self::close_pipe(1, PIPE1CTR)?;
        Self::close_pipe(2, PIPE2CTR)?;
        // RA4M1 pipes 6-9 are the interrupt-capable bank. USB endpoint number 3 is
        // independent of the hardware pipe chosen to carry it.
        Self::close_pipe(6, PIPE6CTR)?;
        Self::select_pipe(0)?;
        unsafe {
            BRDYSTS.write_volatile(!(1 << 1));
            BEMPSTS.write_volatile(!(1 << 2));
        }
        self.reset_software_data_state();
        Ok(())
    }

    /// Configure PIPE1 (bulk OUT, EP1), PIPE2 (bulk IN, EP2), and the advertised
    /// notification endpoint from a deterministic, empty DATA0 state.
    fn open_data_pipes(&mut self) -> Result<(), UsbBackendError> {
        self.close_data_pipes()?;
        Self::select_pipe(1)?;
        unsafe {
            PIPEMAXP.write_volatile(64);
            PIPE1CTR.write_volatile(CTR_ACLRM | CTR_SQCLR);
            PIPE1CTR.write_volatile(0);
            PIPECFG.write_volatile(PIPE1_BULK_OUT_CFG);
        }
        Self::wait_pipe_config(PIPE1_BULK_OUT_CFG, Some(64), None)?;
        Self::select_pipe(2)?;
        unsafe {
            PIPEMAXP.write_volatile(64);
            PIPE2CTR.write_volatile(CTR_ACLRM | CTR_SQCLR);
            PIPE2CTR.write_volatile(0);
            PIPECFG.write_volatile(PIPE2_BULK_IN_CFG);
        }
        Self::wait_pipe_config(PIPE2_BULK_IN_CFG, Some(64), None)?;
        Self::select_pipe(6)?;
        unsafe {
            PIPEMAXP.write_volatile(8);
            PIPEPERI.write_volatile(PIPE3_INTERVAL);
            PIPE6CTR.write_volatile(CTR_ACLRM | CTR_SQCLR);
            PIPE6CTR.write_volatile(0);
            PIPECFG.write_volatile(NOTIFY_INTERRUPT_IN_CFG);
        }
        Self::wait_pipe_config(NOTIFY_INTERRUPT_IN_CFG, Some(8), Some(PIPE3_INTERVAL))?;
        Self::select_pipe(0)?;
        unsafe {
            BRDYSTS.write_volatile(!(1 << 1));
            BEMPSTS.write_volatile(!(1 << 2));
            BRDYENB.write_volatile(BRDYENB.read_volatile() | (1 << 1));
            BEMPENB.write_volatile(BEMPENB.read_volatile() | (1 << 2));
            PIPE1CTR.write_volatile(PID_BUF);
            PIPE2CTR.write_volatile(PID_NAK);
            PIPE6CTR.write_volatile(PID_NAK);
        }
        Ok(())
    }

    fn endpoint_pipe(index: u16) -> Option<u8> {
        if index & 0xFF00 != 0 {
            return None;
        }
        match index as u8 {
            0x00 | 0x80 => Some(0),
            0x01 => Some(1),
            0x82 => Some(2),
            0x83 => Some(6),
            _ => None,
        }
    }

    fn pipe_ctr(pipe: u8) -> Option<*mut u16> {
        match pipe {
            0 => Some(DCPCTR),
            1 => Some(PIPE1CTR),
            2 => Some(PIPE2CTR),
            6 => Some(PIPE6CTR),
            _ => None,
        }
    }

    fn haltable_pipe(index: u16) -> Option<u8> {
        Self::endpoint_pipe(index).filter(|pipe| *pipe != 0)
    }

    fn pid_is_halted(pid: u16) -> bool {
        pid & PID_STALL != 0
    }

    fn stall_transition(pid: u16) -> (u16, u16) {
        ((pid & PID_MASK) | PID_STALL, PID_STALL)
    }

    fn normal_idle_pid(pipe: u8) -> Option<u16> {
        match pipe {
            1 => Some(PID_BUF),
            2 | 6 => Some(PID_NAK),
            _ => None,
        }
    }

    fn endpoint_status(index: u16, pid: u16) -> Option<bool> {
        Self::endpoint_pipe(index)?;
        Some(Self::pid_is_halted(pid & PID_MASK))
    }

    fn endpoint_halted(&self, index: u16) -> Option<bool> {
        let pipe = Self::endpoint_pipe(index)?;
        let ctr = Self::pipe_ctr(pipe)?;
        Self::endpoint_status(index, unsafe { ctr.read_volatile() })
    }

    fn set_endpoint_halt(&mut self, index: u16) -> Result<bool, UsbBackendError> {
        let Some(pipe) = Self::haltable_pipe(index) else {
            return Ok(false);
        };
        let Some(ctr) = Self::pipe_ctr(pipe) else {
            return Ok(false);
        };
        unsafe {
            let (transition, stalled) = Self::stall_transition(ctr.read_volatile());
            ctr.write_volatile(transition);
            ctr.write_volatile(stalled);
        }
        Self::wait_pipe_pid(ctr, PID_STALL)?;
        Ok(true)
    }

    fn clear_endpoint_halt(&mut self, index: u16) -> Result<bool, UsbBackendError> {
        let Some(pipe) = Self::haltable_pipe(index) else {
            return Ok(false);
        };
        let Some(ctr) = Self::pipe_ctr(pipe) else {
            return Ok(false);
        };
        unsafe { ctr.write_volatile(PID_NAK) };
        Self::wait_pipe_idle(ctr)?;
        unsafe {
            // Clear buffered bytes and the sequence toggle together, then restore the
            // endpoint's normal idle PID. IN endpoints stay NAK until data is queued.
            ctr.write_volatile(CTR_ACLRM | CTR_SQCLR);
            ctr.write_volatile(CTR_SQCLR);
            ctr.write_volatile(Self::normal_idle_pid(pipe).unwrap_or(PID_NAK));
        }
        Self::wait_pipe_pid(ctr, Self::normal_idle_pid(pipe).unwrap_or(PID_NAK))?;
        match pipe {
            1 => {
                self.rx_len = 0;
                self.rx_pos = 0;
            }
            2 => self.tx_pending = false,
            _ => {}
        }
        Ok(true)
    }

    fn latch_fault(&mut self, error: UsbBackendError) {
        self.fault.latch(error);
        // The readiness transaction already timed out, so fail shut using only direct
        // writes. Retrying another bounded selector/pipe wait here could wedge teardown.
        unsafe {
            SYSCFG.write_volatile(SYSCFG.read_volatile() & !SYSCFG_DPRPU);
            INTENB0.write_volatile(0);
            BRDYENB.write_volatile(0);
            BEMPENB.write_volatile(0);
            PIPE1CTR.write_volatile(PID_NAK);
            PIPE2CTR.write_volatile(PID_NAK);
            PIPE6CTR.write_volatile(PID_NAK);
        }
        self.stage = Stage::PoweredOff;
        self.pending_addr = 0;
        self.ctrl_len = 0;
        self.ctrl_pos = 0;
        self.pending_line_coding = false;
        self.reset_software_data_state();
    }

    fn latch_result(&mut self, result: Result<(), UsbBackendError>) -> bool {
        match result {
            Ok(()) => true,
            Err(error) => {
                self.latch_fault(error);
                false
            }
        }
    }

    fn attach(&mut self) -> Result<(), UsbBackendError> {
        if !Self::may_attach(self.controller_ready, self.link_enabled, unsafe {
            INTSTS0.read_volatile() & IS_VBSTS != 0
        }) {
            return Ok(());
        }
        self.close_data_pipes()?;
        self.pending_addr = 0;
        self.stage = Stage::Attached;
        self.resume_stage = Stage::Attached;
        unsafe { SYSCFG.write_volatile(SYSCFG.read_volatile() | SYSCFG_DPRPU) };
        Ok(())
    }

    fn detach(&mut self) -> Result<(), UsbBackendError> {
        unsafe {
            SYSCFG.write_volatile(SYSCFG.read_volatile() & !SYSCFG_DPRPU);
            INTENB0.write_volatile(INTENB0.read_volatile() & !IE_SOFE);
        }
        let close_result = self.close_data_pipes();
        self.pending_addr = 0;
        self.ctrl_len = 0;
        self.ctrl_pos = 0;
        self.pending_line_coding = false;
        self.line_coding = DEFAULT_LINE_CODING;
        self.stage = Stage::PoweredOff;
        self.resume_stage = Stage::Attached;
        close_result
    }

    fn suspend(&mut self, dvsq: u16) {
        self.resume_stage = Self::resume_stage_for(dvsq, self.stage);
        self.stage = Stage::Suspended;
        // RESM is not the only legal wake indication. Hosts can resume by sending SOF;
        // enable that otherwise-noisy source only while suspended.
        unsafe { INTENB0.write_volatile(INTENB0.read_volatile() | IE_SOFE) };
    }

    fn resume(&mut self) {
        if self.stage == Stage::Suspended {
            self.stage = self.resume_stage;
        }
        unsafe { INTENB0.write_volatile(INTENB0.read_volatile() & !IE_SOFE) };
    }

    pub(crate) fn disconnect(&mut self) {
        self.link_enabled = false;
        if self.controller_ready {
            let result = self.detach();
            self.latch_result(result);
        } else {
            self.stage = Stage::PoweredOff;
            self.reset_software_data_state();
        }
    }

    pub(crate) fn reconnect(&mut self) {
        self.link_enabled = true;
        if self.fault.check().is_ok()
            && self.controller_ready
            && unsafe { INTSTS0.read_volatile() & IS_VBSTS != 0 }
        {
            let result = self.attach();
            self.latch_result(result);
        }
    }
}

impl UsbStack for RaUsbfsCdc {
    fn poll(&mut self) -> CdcState {
        // Controller bring-up is a one-shot transaction. Do not reinterpret later VBUS
        // as recovery from an SCKE timeout, and do not auto-attach while the board mux
        // deliberately routes the connector away from RA4M1.
        if self.fault.check().is_err() || !self.controller_ready || !self.link_enabled {
            return CdcState::Disconnected;
        }
        unsafe {
            let is = INTSTS0.read_volatile();
            let vbus_present = is & IS_VBSTS != 0;
            if is & IS_VBINT != 0 {
                INTSTS0.write_volatile(!IS_VBINT);
            }
            // Sample the live status every poll as well as acknowledging VBINT. This
            // makes a missed edge fail closed instead of retaining Configured forever.
            if !vbus_present {
                if self.stage != Stage::PoweredOff {
                    let result = self.detach();
                    self.latch_result(result);
                }
                return CdcState::Disconnected;
            }
            if self.stage == Stage::PoweredOff {
                let result = self.attach();
                if !self.latch_result(result) {
                    return CdcState::Disconnected;
                }
            }

            let was_suspended = self.stage == Stage::Suspended;
            let resume_event = is & (IS_RESM | IS_SOFR) != 0;
            if is & IS_RESM != 0 {
                INTSTS0.write_volatile(!IS_RESM);
            }
            if is & IS_SOFR != 0 {
                INTSTS0.write_volatile(!IS_SOFR);
            }
            // Device-state transitions carry the authoritative reset/address state in
            // INTSTS0.DVSQ. DVSTCTR0.RHST is a host-controller field.
            if is & IS_DVST != 0 {
                INTSTS0.write_volatile(!IS_DVST);
                match is & DVSQ_MASK {
                    DVSQ_DEFAULT => {
                        let result = self.close_data_pipes();
                        if !self.latch_result(result) {
                            return CdcState::Disconnected;
                        }
                        self.pending_addr = 0;
                        self.stage = Stage::Reset;
                        self.resume_stage = Stage::Reset;
                        self.ctrl_len = 0;
                        self.ctrl_pos = 0;
                        self.pending_line_coding = false;
                        self.line_coding = DEFAULT_LINE_CODING;
                        DCPCTR.write_volatile(PID_BUF);
                    }
                    DVSQ_ADDRESSED => {
                        self.stage = Stage::Addressed;
                        self.resume_stage = Stage::Addressed;
                    }
                    DVSQ_SUSPENDED_0 | DVSQ_SUSPENDED_1 | DVSQ_SUSPENDED_2 | DVSQ_SUSPENDED_3 => {
                        self.suspend(is & DVSQ_MASK)
                    }
                    _ => {}
                }
            }
            // A SOFR bit that was already latched before the DVST transition must not
            // immediately undo a newly reported suspend in this same snapshot.
            if resume_event && was_suspended {
                self.resume();
            }
            // VALID marks a received setup packet. Do not filter by CTSQ here:
            // Windows may issue descriptor/status requests while the controller reports
            // a different control-transfer phase, and the request registers still hold
            // the authoritative setup packet.
            if (is & IS_CTRT != 0) && (is & IS_VALID != 0) {
                INTSTS0.write_volatile(!IS_CTRT);
                let result = self.handle_setup();
                if !self.latch_result(result) {
                    return CdcState::Disconnected;
                }
            }
            // EP0 can hold one 64-byte packet. Continue long descriptors when the
            // previous packet leaves the control FIFO.
            if BEMPSTS.read_volatile() & 1 != 0 {
                BEMPSTS.write_volatile(!1);
                let result = self.ctrl_continue();
                if !self.latch_result(result) {
                    return CdcState::Disconnected;
                }
            }
            if BEMPSTS.read_volatile() & (1 << 2) != 0 {
                BEMPSTS.write_volatile(!(1 << 2));
                PIPE2CTR.write_volatile(PID_NAK);
                self.tx_pending = false;
            }
            if self.pending_line_coding && BRDYSTS.read_volatile() & 1 != 0 {
                let result = self.receive_line_coding();
                if !self.latch_result(result) {
                    return CdcState::Disconnected;
                }
            }
        }
        match self.stage {
            Stage::Configured => CdcState::Configured,
            Stage::Addressed => CdcState::Addressed,
            Stage::Attached | Stage::Reset => CdcState::Default,
            Stage::PoweredOff => CdcState::Disconnected,
            Stage::Suspended => CdcState::Suspended,
        }
    }

    fn write(&mut self, data: &[u8]) -> usize {
        self.try_write(data).unwrap_or(0)
    }

    fn try_write(&mut self, data: &[u8]) -> Result<usize, UsbBackendError> {
        self.fault.check()?;
        if data.is_empty() || self.stage != Stage::Configured || self.tx_pending {
            return Ok(0);
        }
        unsafe {
            // Keep CFIFO exclusively on EP0. D0FIFO owns all data-pipe traffic, so a
            // bulk write cannot retarget the FIFO underneath a setup/data stage.
            if let Err(error) = Self::select_d0_fifo(2, CF_MBW_16) {
                self.latch_fault(error);
                return Err(error);
            }
            if let Err(error) = Self::wait_d0_fifo_ready() {
                let _ = Self::deselect_d0_fifo();
                self.latch_fault(error);
                return Err(error);
            }
            let n = data.len().min(64);
            let mut i = 0;
            while i + 1 < n {
                D0FIFO.write_volatile(u16::from(data[i]) | (u16::from(data[i + 1]) << 8));
                i += 2;
            }
            if i < n {
                D0FIFO8.write_volatile(data[i]);
            }
            if n < 64 {
                D0FIFOCTR.write_volatile(CF_BVAL);
            }
            if let Err(error) = Self::deselect_d0_fifo() {
                // The FIFO cannot safely be handed to another pipe. Clear this transfer
                // and report a persistent controller failure instead of backpressure.
                PIPE2CTR.write_volatile(PID_NAK);
                D0FIFOCTR.write_volatile(CF_BCLR);
                self.latch_fault(error);
                return Err(error);
            }
            PIPE2CTR.write_volatile(PID_BUF);
            self.tx_pending = n != 0;
            Ok(n)
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> usize {
        self.try_read(buf).unwrap_or(0)
    }

    fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, UsbBackendError> {
        self.fault.check()?;
        if buf.is_empty() {
            return Ok(0);
        }
        // deliver any buffered bytes first
        if self.rx_pos < self.rx_len {
            let n = (self.rx_len - self.rx_pos).min(buf.len());
            buf[..n].copy_from_slice(&self.rx[self.rx_pos..self.rx_pos + n]);
            self.rx_pos += n;
            return Ok(n);
        }
        if self.stage != Stage::Configured {
            return Ok(0);
        }
        unsafe {
            if BRDYSTS.read_volatile() & (1 << 1) == 0 {
                return Ok(0); // no OUT data on PIPE1
            }
            if let Err(error) = Self::select_d0_fifo(1, 0) {
                self.latch_fault(error);
                return Err(error);
            }
            if let Err(error) = Self::wait_d0_fifo_ready() {
                let _ = Self::deselect_d0_fifo();
                self.latch_fault(error);
                return Err(error);
            }
            let dtln = (D0FIFOCTR.read_volatile() & CF_DTLN_MASK) as usize;
            if dtln > self.rx.len() {
                D0FIFOCTR.write_volatile(CF_BCLR);
                let _ = Self::deselect_d0_fifo();
                let error = UsbBackendError::BufferOverflow;
                self.latch_fault(error);
                return Err(error);
            }
            let n = dtln;
            for b in self.rx.iter_mut().take(n) {
                *b = D0FIFO8.read_volatile();
            }
            D0FIFOCTR.write_volatile(CF_BCLR);
            if let Err(error) = Self::deselect_d0_fifo() {
                PIPE1CTR.write_volatile(PID_NAK);
                self.reset_software_data_state();
                self.latch_fault(error);
                return Err(error);
            }
            BRDYSTS.write_volatile(!(1 << 1));
            PIPE1CTR.write_volatile(PID_BUF);
            self.rx_len = n;
            self.rx_pos = 0;
            let m = n.min(buf.len());
            buf[..m].copy_from_slice(&self.rx[..m]);
            self.rx_pos = m;
            Ok(m)
        }
    }

    fn configured(&self) -> bool {
        self.stage == Stage::Configured
    }

    fn flush(&mut self) -> bool {
        self.try_flush().unwrap_or(false)
    }

    fn try_flush(&mut self) -> Result<bool, UsbBackendError> {
        self.fault.check()?;
        Ok(!self.tx_pending)
    }

    fn backend_fault(&self) -> Option<UsbBackendError> {
        self.fault.latched
    }

    fn backend_id(&self) -> u32 {
        backend_id::RA_USBFS
    }
}

// silence unused-const warnings for the reference bits kept for clarity
const _: () = {
    let _ = (CF_REW, CTR_BSTS);
};

#[cfg(test)]
mod tests {
    use super::{
        BackendFault, ControlState, RaUsbfsCdc, SetupAction, SetupPacket, Stage, UsbConfig,
        DVSQ_SUSPENDED_0, DVSQ_SUSPENDED_1, DVSQ_SUSPENDED_2, DVSQ_SUSPENDED_3, PIPE1_BULK_OUT_CFG,
        PIPE2_BULK_IN_CFG, PIPE3_INTERVAL, PIPECFG_SHTNAK, RA4M1_USB_CONFIG,
    };
    use crate::{UsbBackendError, UsbStack};

    #[test]
    fn fixed_descriptor_identity_is_enforced() {
        assert!(crate::config_supported(&RA4M1_USB_CONFIG));
        let wrong = UsbConfig::new(
            RA4M1_USB_CONFIG.vid,
            RA4M1_USB_CONFIG.pid,
            RA4M1_USB_CONFIG.manufacturer,
            "different product",
            RA4M1_USB_CONFIG.serial,
        );
        assert!(!crate::config_supported(&wrong));
    }

    #[test]
    fn every_advertised_string_index_exists_and_unknown_indices_stall() {
        for index in 0..=3 {
            let descriptor = RaUsbfsCdc::string_desc(index).expect("advertised string");
            assert_eq!(usize::from(descriptor[0]), descriptor.len());
            assert_eq!(descriptor[1], 3);
        }
        assert!(RaUsbfsCdc::string_desc(4).is_none());
        assert!(RaUsbfsCdc::string_desc(0xee).is_none());
    }

    #[test]
    fn suspend_stage_is_never_reported_as_configured() {
        assert_ne!(Stage::Suspended, Stage::Configured);
    }

    #[test]
    fn failed_controller_or_disabled_route_can_never_attach_from_vbus() {
        assert!(!RaUsbfsCdc::may_attach(false, true, true));
        assert!(!RaUsbfsCdc::may_attach(true, false, true));
        assert!(!RaUsbfsCdc::may_attach(true, true, false));
        assert!(RaUsbfsCdc::may_attach(true, true, true));
        assert_eq!(
            BackendFault::for_startup(false).check(),
            Err(UsbBackendError::StartupTimeout)
        );
        assert_eq!(BackendFault::for_startup(true).check(), Ok(()));
    }

    const fn setup(
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        length: u16,
    ) -> SetupPacket {
        SetupPacket {
            request_type,
            request,
            value,
            index,
            length,
        }
    }

    #[test]
    fn every_supported_setup_request_has_one_exact_legal_shape() {
        use ControlState::{Addressed, Configured, Default};
        use SetupAction::*;

        let cases = [
            (
                setup(0x80, 0x06, 0x0100, 0, 8),
                Default,
                Descriptor(super::Descriptor::Device, 8),
            ),
            (
                setup(0x80, 0x06, 0x0200, 0, 75),
                Addressed,
                Descriptor(super::Descriptor::Configuration, 75),
            ),
            (
                setup(0x80, 0x06, 0x0300, 0, 4),
                Default,
                Descriptor(super::Descriptor::LanguageIds, 4),
            ),
            (
                setup(0x80, 0x06, 0x0301, 0x0409, 255),
                Configured,
                Descriptor(super::Descriptor::Manufacturer, 255),
            ),
            (
                setup(0x80, 0x06, 0x0302, 0x0409, 32),
                Addressed,
                Descriptor(super::Descriptor::Product, 32),
            ),
            (
                setup(0x80, 0x06, 0x0303, 0x0409, 18),
                Configured,
                Descriptor(super::Descriptor::Serial, 18),
            ),
            (setup(0x80, 0x00, 0, 0, 2), Addressed, DeviceStatus),
            (setup(0x81, 0x00, 0, 1, 2), Configured, InterfaceStatus),
            (
                setup(0x82, 0x00, 0, 0x80, 2),
                Addressed,
                EndpointStatus(0x80),
            ),
            (
                setup(0x82, 0x00, 0, 0x01, 2),
                Configured,
                EndpointStatus(0x01),
            ),
            (setup(0x80, 0x08, 0, 0, 1), Addressed, GetConfiguration),
            (setup(0x81, 0x0a, 0, 1, 1), Configured, GetInterface),
            (setup(0xa1, 0x21, 0, 0, 7), Configured, GetLineCoding),
            (setup(0x00, 0x05, 127, 0, 0), Default, SetAddress(127)),
            (setup(0x00, 0x09, 1, 0, 0), Addressed, SetConfiguration(1)),
            (setup(0x21, 0x20, 0, 0, 7), Configured, ReceiveLineCoding),
            (setup(0x21, 0x22, 3, 0, 0), Configured, SetControlLineState),
            (setup(0x21, 0x23, 0xffff, 0, 0), Configured, SendBreak),
            (setup(0x01, 0x0b, 0, 1, 0), Configured, SetInterface),
            (
                setup(0x02, 0x01, 0, 0x82, 0),
                Configured,
                ClearEndpointHalt(0x82),
            ),
            (
                setup(0x02, 0x03, 0, 0x83, 0),
                Configured,
                SetEndpointHalt(0x83),
            ),
        ];

        for (packet, state, expected) in cases {
            assert_eq!(RaUsbfsCdc::validate_setup(packet, state), Some(expected));
        }
    }

    #[test]
    fn one_bit_mutations_and_wrong_states_are_rejected_before_hardware_actions() {
        use ControlState::{Addressed, Configured, Default, Unavailable};

        let cases = [
            (
                setup(0x80, 0x06, 0x0100, 0, 8),
                Default,
                Unavailable,
                true,
                false,
            ),
            (setup(0x80, 0x00, 0, 0, 2), Addressed, Default, true, true),
            (
                setup(0x81, 0x00, 0, 1, 2),
                Configured,
                Addressed,
                true,
                true,
            ),
            (
                setup(0x82, 0x00, 0, 0x01, 2),
                Configured,
                Default,
                true,
                true,
            ),
            (setup(0x80, 0x08, 0, 0, 1), Addressed, Default, true, true),
            (
                setup(0x81, 0x0a, 0, 1, 1),
                Configured,
                Addressed,
                true,
                true,
            ),
            (
                setup(0xa1, 0x21, 0, 0, 7),
                Configured,
                Addressed,
                true,
                true,
            ),
            (setup(0x00, 0x05, 1, 0, 0), Default, Configured, true, true),
            (setup(0x00, 0x09, 1, 0, 0), Addressed, Default, true, true),
            (
                setup(0x21, 0x20, 0, 0, 7),
                Configured,
                Addressed,
                true,
                true,
            ),
            (
                setup(0x21, 0x22, 3, 0, 0),
                Configured,
                Addressed,
                true,
                true,
            ),
            // SEND_BREAK deliberately accepts every 16-bit duration value.
            (
                setup(0x21, 0x23, 1, 0, 0),
                Configured,
                Addressed,
                false,
                true,
            ),
            (
                setup(0x01, 0x0b, 0, 1, 0),
                Configured,
                Addressed,
                true,
                true,
            ),
            (
                setup(0x02, 0x01, 0, 0x82, 0),
                Configured,
                Addressed,
                true,
                true,
            ),
            (
                setup(0x02, 0x03, 0, 0x83, 0),
                Configured,
                Addressed,
                true,
                true,
            ),
        ];

        for (packet, state, wrong_state, value_is_constrained, length_is_constrained) in cases {
            assert!(RaUsbfsCdc::validate_setup(packet, state).is_some());
            assert_eq!(RaUsbfsCdc::validate_setup(packet, wrong_state), None);

            let mut mutated = packet;
            mutated.request_type ^= 0x40;
            assert_eq!(RaUsbfsCdc::validate_setup(mutated, state), None);

            let mut mutated = packet;
            mutated.request ^= 0x04;
            assert_eq!(RaUsbfsCdc::validate_setup(mutated, state), None);

            if value_is_constrained {
                let mut mutated = packet;
                mutated.value ^= 0x8000;
                assert_eq!(RaUsbfsCdc::validate_setup(mutated, state), None);
            }

            let mut mutated = packet;
            mutated.index ^= 0x8000;
            assert_eq!(RaUsbfsCdc::validate_setup(mutated, state), None);

            if length_is_constrained {
                let mut mutated = packet;
                mutated.length ^= 0x8000;
                assert_eq!(RaUsbfsCdc::validate_setup(mutated, state), None);
            }
        }

        // Descriptor lengths are caller-selected, but descriptor indices and language
        // IDs are exact. These are common one-bit malformed-enumeration cases.
        assert_eq!(
            RaUsbfsCdc::validate_setup(setup(0x80, 0x06, 0x0101, 0, 18), Default),
            None
        );
        assert_eq!(
            RaUsbfsCdc::validate_setup(setup(0x80, 0x06, 0x0301, 0x0408, 20), Default),
            None
        );
    }

    #[test]
    fn injected_controller_fault_is_first_failure_wins_and_gates_every_io_surface() {
        let mut fault = BackendFault::for_startup(true);
        assert_eq!(fault.check(), Ok(()));
        fault.latch(UsbBackendError::ControllerTimeout);
        assert_eq!(fault.check(), Err(UsbBackendError::ControllerTimeout));
        // Teardown or a later parse failure must not erase the root controller fault.
        fault.latch(UsbBackendError::BufferOverflow);
        assert_eq!(fault.latched, Some(UsbBackendError::ControllerTimeout));

        // This fixture is safe on the host because every error-aware entry point checks
        // the injected latch before reading any memory-mapped register.
        let mut backend = RaUsbfsCdc {
            controller_ready: true,
            fault,
            link_enabled: true,
            stage: Stage::Configured,
            resume_stage: Stage::Configured,
            pending_addr: 1,
            rx: [0; 64],
            rx_len: 0,
            rx_pos: 0,
            ctrl_data: [0; super::CTRL_BUFFER_CAPACITY],
            ctrl_len: 0,
            ctrl_pos: 0,
            pending_line_coding: false,
            line_coding: super::DEFAULT_LINE_CODING,
            tx_pending: false,
        };
        assert_eq!(
            backend.try_write(b"x"),
            Err(UsbBackendError::ControllerTimeout)
        );
        assert_eq!(
            backend.try_read(&mut [0; 1]),
            Err(UsbBackendError::ControllerTimeout)
        );
        assert_eq!(backend.try_flush(), Err(UsbBackendError::ControllerTimeout));
        assert_eq!(
            backend.backend_fault(),
            Some(UsbBackendError::ControllerTimeout)
        );
    }

    #[test]
    fn suspended_device_resumes_to_its_prior_usb_state() {
        assert_eq!(
            RaUsbfsCdc::resume_stage_for(DVSQ_SUSPENDED_0, Stage::Configured),
            Stage::Attached
        );
        assert_eq!(
            RaUsbfsCdc::resume_stage_for(DVSQ_SUSPENDED_1, Stage::Attached),
            Stage::Reset
        );
        assert_eq!(
            RaUsbfsCdc::resume_stage_for(DVSQ_SUSPENDED_2, Stage::Attached),
            Stage::Addressed
        );
        assert_eq!(
            RaUsbfsCdc::resume_stage_for(DVSQ_SUSPENDED_3, Stage::Attached),
            Stage::Configured
        );
    }

    #[test]
    fn line_coding_is_retained_only_as_one_complete_cdc_payload() {
        let changed = [0x80, 0x25, 0, 0, 2, 1, 7];
        assert_eq!(RaUsbfsCdc::validated_line_coding(&changed), Some(changed));
        assert_eq!(RaUsbfsCdc::validated_line_coding(&changed[..6]), None);
        assert_eq!(RaUsbfsCdc::validated_line_coding(&[0; 8]), None);
    }

    #[test]
    fn endpoint_lookup_rejects_unadvertised_or_malformed_addresses() {
        assert_eq!(RaUsbfsCdc::endpoint_pipe(0x00), Some(0));
        assert_eq!(RaUsbfsCdc::endpoint_pipe(0x80), Some(0));
        assert_eq!(RaUsbfsCdc::endpoint_pipe(0x01), Some(1));
        assert_eq!(RaUsbfsCdc::endpoint_pipe(0x82), Some(2));
        assert_eq!(RaUsbfsCdc::endpoint_pipe(0x83), Some(6));
        assert_eq!(RaUsbfsCdc::endpoint_pipe(0x81), None);
        assert_eq!(RaUsbfsCdc::endpoint_pipe(0x02), None);
        assert_eq!(RaUsbfsCdc::endpoint_pipe(0x03), None);
        assert_eq!(RaUsbfsCdc::endpoint_pipe(0x84), None);
        assert_eq!(RaUsbfsCdc::endpoint_pipe(0x0182), None);
    }

    #[test]
    fn endpoint_halt_contract_is_advertised_directional_and_stateful() {
        // Endpoint zero is advertised for status, but ENDPOINT_HALT is not a legal
        // feature transition for the default control pipe.
        assert_eq!(RaUsbfsCdc::endpoint_status(0x00, 0), Some(false));
        assert_eq!(RaUsbfsCdc::endpoint_status(0x80, 2), Some(true));
        assert_eq!(RaUsbfsCdc::haltable_pipe(0x00), None);
        assert_eq!(RaUsbfsCdc::haltable_pipe(0x80), None);

        for (address, pipe) in [(0x01, 1), (0x82, 2), (0x83, 6)] {
            assert_eq!(RaUsbfsCdc::haltable_pipe(address), Some(pipe));
            assert_eq!(RaUsbfsCdc::endpoint_status(address, 0), Some(false));
            assert_eq!(RaUsbfsCdc::endpoint_status(address, 1), Some(false));
            assert_eq!(RaUsbfsCdc::endpoint_status(address, 2), Some(true));
            assert_eq!(RaUsbfsCdc::endpoint_status(address, 3), Some(true));
        }

        for address in [0x02, 0x03, 0x81, 0x84, 0x0182] {
            assert_eq!(RaUsbfsCdc::endpoint_status(address, 2), None);
            assert_eq!(RaUsbfsCdc::haltable_pipe(address), None);
        }

        // TinyUSB/RUSB2's legal set sequence preserves BUF as STALL2 for the first
        // write, then settles every prior PID at STALL.
        assert_eq!(RaUsbfsCdc::stall_transition(0), (2, 2));
        assert_eq!(RaUsbfsCdc::stall_transition(1), (3, 2));
        assert_eq!(RaUsbfsCdc::stall_transition(2), (2, 2));
        assert_eq!(RaUsbfsCdc::stall_transition(3), (3, 2));
        assert_eq!(RaUsbfsCdc::normal_idle_pid(1), Some(1));
        assert_eq!(RaUsbfsCdc::normal_idle_pid(2), Some(0));
        assert_eq!(RaUsbfsCdc::normal_idle_pid(6), Some(0));
        assert_eq!(RaUsbfsCdc::normal_idle_pid(0), None);
    }

    #[test]
    fn data_pipe_contract_uses_short_packet_nak_and_encoded_interval() {
        assert_ne!(PIPE1_BULK_OUT_CFG & PIPECFG_SHTNAK, 0);
        assert_ne!(PIPE2_BULK_IN_CFG & PIPECFG_SHTNAK, 0);
        assert_eq!(PIPE3_INTERVAL, 4);
    }
}
