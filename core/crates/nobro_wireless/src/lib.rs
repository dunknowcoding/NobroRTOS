//! Allocation-free wireless domain: bounded link contracts, admission, and helpers.
//!
//! Follows the nobro_usb mountable-backend pattern: a radio implements
//! [`WirelessBackend`], apps talk bytes + link state, and protocol identity/limits are
//! **data** ([`LinkDescriptor`]) so schedulers and the collector reason about any radio
//! uniformly. Pure-logic protocol helpers live here too: [`BleAdvBuilder`] constructs
//! the advertising PDU format `ble_adv_demo` proved on air (M123), and the `rfid`
//! module carries ISO 14443A anticollision arithmetic.
#![cfg_attr(not(test), no_std)]

/// Wireless protocol families the capsule speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// BLE (advertising or connection-based).
    Ble,
    /// WiFi carrying TCP/UDP (e.g. the telemetry JSONL link).
    WifiTcp,
    /// Zigbee (802.15.4 + NWK/APS, e.g. via a CC2530 co-processor).
    Zigbee,
    /// Thread (802.15.4 + 6LoWPAN mesh).
    Thread,
    /// Proximity RFID/NFC (ISO 14443 family).
    Rfid,
    /// Raw proprietary 2.4 GHz (nRF RADIO link mode).
    Proprietary,
}

/// A wireless link as data: what it is and what it can carry.
#[derive(Clone, Copy, Debug)]
pub struct LinkDescriptor {
    pub name: &'static str,
    pub protocol: Protocol,
    /// Largest application payload one frame carries.
    pub mtu: u16,
    /// True when the link must join/associate before payload flows.
    pub requires_join: bool,
    /// True when the link is broadcast-only (no per-frame acknowledgement).
    pub broadcast_only: bool,
}

/// Built-in link catalog (extend with a `pub const`, like every other catalog).
pub mod link_catalog {
    use super::*;

    pub const BLE_ADV: LinkDescriptor = LinkDescriptor {
        name: "BLE legacy advertising",
        protocol: Protocol::Ble,
        mtu: 24, // ADV_NONCONN_IND payload after AdvA + AD overhead
        requires_join: false,
        broadcast_only: true,
    };
    pub const WIFI_TCP: LinkDescriptor = LinkDescriptor {
        name: "WiFi TCP stream",
        protocol: Protocol::WifiTcp,
        mtu: 1460,
        requires_join: true,
        broadcast_only: false,
    };
    pub const ZIGBEE_APS: LinkDescriptor = LinkDescriptor {
        name: "Zigbee APS payload",
        protocol: Protocol::Zigbee,
        mtu: 82,
        requires_join: true,
        broadcast_only: false,
    };
    pub const THREAD_UDP: LinkDescriptor = LinkDescriptor {
        name: "Thread UDP (6LoWPAN)",
        protocol: Protocol::Thread,
        mtu: 1232,
        requires_join: true,
        broadcast_only: false,
    };
    pub const RFID_14443A: LinkDescriptor = LinkDescriptor {
        name: "ISO 14443A proximity",
        protocol: Protocol::Rfid,
        mtu: 18, // MFRC522 FIFO response window used by the no-heap backend
        requires_join: false,
        broadcast_only: false,
    };
    pub const NRF_PROPRIETARY: LinkDescriptor = LinkDescriptor {
        name: "nRF proprietary 2.4 GHz",
        protocol: Protocol::Proprietary,
        mtu: 60,
        requires_join: false,
        broadcast_only: true,
    };
}

/// Link liveness a transport reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Down,
    Joining,
    Up,
}

/// The mountable radio surface. One backend per physical radio, chosen per board -
/// the same pattern as `nobro_usb::UsbStack`.
pub trait WirelessBackend {
    fn descriptor(&self) -> LinkDescriptor;
    fn link_state(&mut self) -> LinkState;
    /// Send one payload (<= mtu); returns true when the radio accepted it.
    fn send(&mut self, payload: &[u8]) -> bool;
    /// Receive into `buf`; returns bytes delivered (0 = nothing pending).
    fn recv(&mut self, buf: &mut [u8]) -> usize;

    /// Reinitialize the same physical transport. Backends that cannot recover
    /// without board-specific intervention return false explicitly.
    fn recover(&mut self) -> bool {
        false
    }
}

/// Bounded packet owned by the caller; no allocator or hidden global queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet<const N: usize> {
    bytes: [u8; N],
    len: u16,
}

impl<const N: usize> Default for Packet<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Packet<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    pub fn copy_from(payload: &[u8]) -> Option<Self> {
        if payload.len() > N || payload.len() > usize::from(u16::MAX) {
            return None;
        }
        let mut packet = Self::new();
        packet.bytes[..payload.len()].copy_from_slice(payload);
        packet.len = payload.len() as u16;
        Some(packet)
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    pub const fn len(&self) -> u16 {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TxContract {
    pub deadline_us: u64,
    pub priority: u8,
    pub max_attempts: u8,
}

impl TxContract {
    pub const fn by(deadline_us: u64) -> Self {
        Self {
            deadline_us,
            priority: 0,
            max_attempts: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkBudget {
    pub max_payload_bytes: u16,
    pub max_tx_per_window: u16,
    pub max_bytes_per_window: u32,
}

impl LinkBudget {
    pub const fn new(
        max_payload_bytes: u16,
        max_tx_per_window: u16,
        max_bytes_per_window: u32,
    ) -> Self {
        Self {
            max_payload_bytes,
            max_tx_per_window,
            max_bytes_per_window,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkDiagnostics {
    pub tx_accepted: u32,
    pub tx_rejected: u32,
    pub rx_packets: u32,
    pub deadline_rejections: u32,
    pub budget_rejections: u32,
    pub recoveries: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    LinkDown,
    PayloadTooLarge,
    DeadlineElapsed,
    WindowExhausted,
    BackendRejected,
}

/// Deadline and resource-accounting wrapper shared by every backend. Resetting a
/// window is an explicit scheduler action, so no backend owns a private clock.
pub struct ManagedLink<B> {
    backend: B,
    budget: LinkBudget,
    window_tx: u16,
    window_bytes: u32,
    diagnostics: LinkDiagnostics,
}

impl<B: WirelessBackend> ManagedLink<B> {
    pub const fn new(backend: B, budget: LinkBudget) -> Self {
        Self {
            backend,
            budget,
            window_tx: 0,
            window_bytes: 0,
            diagnostics: LinkDiagnostics {
                tx_accepted: 0,
                tx_rejected: 0,
                rx_packets: 0,
                deadline_rejections: 0,
                budget_rejections: 0,
                recoveries: 0,
            },
        }
    }

    pub fn send_at(
        &mut self,
        now_us: u64,
        contract: TxContract,
        payload: &[u8],
    ) -> Result<(), LinkError> {
        if self.backend.link_state() != LinkState::Up {
            self.diagnostics.tx_rejected = self.diagnostics.tx_rejected.saturating_add(1);
            return Err(LinkError::LinkDown);
        }
        let limit = self
            .budget
            .max_payload_bytes
            .min(self.backend.descriptor().mtu);
        if payload.len() > usize::from(limit) {
            self.diagnostics.tx_rejected = self.diagnostics.tx_rejected.saturating_add(1);
            return Err(LinkError::PayloadTooLarge);
        }
        if now_us > contract.deadline_us {
            self.diagnostics.tx_rejected = self.diagnostics.tx_rejected.saturating_add(1);
            self.diagnostics.deadline_rejections =
                self.diagnostics.deadline_rejections.saturating_add(1);
            return Err(LinkError::DeadlineElapsed);
        }
        let bytes = payload.len() as u32;
        if self.window_tx >= self.budget.max_tx_per_window
            || self.window_bytes.saturating_add(bytes) > self.budget.max_bytes_per_window
        {
            self.diagnostics.tx_rejected = self.diagnostics.tx_rejected.saturating_add(1);
            self.diagnostics.budget_rejections =
                self.diagnostics.budget_rejections.saturating_add(1);
            return Err(LinkError::WindowExhausted);
        }
        if !self.backend.send(payload) {
            self.diagnostics.tx_rejected = self.diagnostics.tx_rejected.saturating_add(1);
            return Err(LinkError::BackendRejected);
        }
        self.window_tx = self.window_tx.saturating_add(1);
        self.window_bytes = self.window_bytes.saturating_add(bytes);
        self.diagnostics.tx_accepted = self.diagnostics.tx_accepted.saturating_add(1);
        Ok(())
    }

    pub fn recv(&mut self, destination: &mut [u8]) -> usize {
        let received = self.backend.recv(destination);
        if received != 0 {
            self.diagnostics.rx_packets = self.diagnostics.rx_packets.saturating_add(1);
        }
        received
    }

    pub fn reset_window(&mut self) {
        self.window_tx = 0;
        self.window_bytes = 0;
    }

    pub fn recover(&mut self) -> bool {
        let recovered = self.backend.recover();
        if recovered {
            self.diagnostics.recoveries = self.diagnostics.recoveries.saturating_add(1);
        }
        recovered
    }

    pub const fn diagnostics(&self) -> LinkDiagnostics {
        self.diagnostics
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
    pub fn into_backend(self) -> B {
        self.backend
    }
}

/// Mesh primitives retain their focused implementation crate, while the domain
/// facade gives applications one import path for link and mesh composition.
pub use nobro_net::{PrioQueue, Route, RoutingTable, SeenSet, TimeSync};

// ---------------------------------------------------------------- BLE advertising

/// Builds the legacy ADV_NONCONN_IND PDU format `ble_adv_demo` verified on air:
/// header + AdvA + Complete-Local-Name AD + Manufacturer-Data AD.
pub struct BleAdvBuilder<'a> {
    /// Random-static advertising address, little-endian (MSB must have top bits 11).
    pub adv_addr: &'a [u8; 6],
    /// Advertised name (kept short; it must fit the 31-byte AdvData budget).
    pub name: &'a [u8],
    /// Manufacturer company id (0xFFFF = test/prototyping).
    pub company_id: u16,
}

/// Legacy advertising PDU kind, selecting the on-air header (Bluetooth Core, LE 1M).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvKind {
    /// ADV_IND: connectable + scannable undirected (a central may connect).
    Connectable,
    /// ADV_SCAN_IND: scannable undirected (scan-response allowed, no connect).
    Scannable,
    /// ADV_NONCONN_IND: broadcast only (the beacon shape ble_adv_demo shipped).
    NonConnectable,
}

impl AdvKind {
    /// PDU header byte (PDU type in bits 0-3) with TxAdd = random.
    const fn header(self) -> u8 {
        match self {
            AdvKind::Connectable => 0x40,    // ADV_IND
            AdvKind::Scannable => 0x46,      // ADV_SCAN_IND
            AdvKind::NonConnectable => 0x42, // ADV_NONCONN_IND
        }
    }
}

impl<'a> BleAdvBuilder<'a> {
    /// Assemble a non-connectable (beacon) PDU - the shape verified on air in M123.
    pub fn build(&self, payload: &[u8], out: &mut [u8]) -> Option<usize> {
        self.build_as(AdvKind::NonConnectable, payload, out)
    }

    /// Assemble an advertising PDU of the chosen [`AdvKind`] with `payload` as the
    /// manufacturer-data body. Returns the total PDU length, or None if it would exceed
    /// the 31-byte legacy AdvData budget or `out`.
    pub fn build_as(&self, kind: AdvKind, payload: &[u8], out: &mut [u8]) -> Option<usize> {
        let name_ad = 2 + self.name.len(); // len byte counts type + body
        let mfr_ad = 2 + 2 + payload.len();
        let adv_data = name_ad + mfr_ad;
        if adv_data > 31 {
            return None; // legacy advertising AdvData budget
        }
        let total = 2 + 6 + adv_data;
        if out.len() < total {
            return None;
        }
        out[0] = kind.header();
        out[1] = (6 + adv_data) as u8;
        out[2..8].copy_from_slice(self.adv_addr);
        let mut pos = 8;
        out[pos] = (1 + self.name.len()) as u8;
        out[pos + 1] = 0x09; // Complete Local Name
        out[pos + 2..pos + 2 + self.name.len()].copy_from_slice(self.name);
        pos += name_ad;
        out[pos] = (1 + 2 + payload.len()) as u8;
        out[pos + 1] = 0xFF; // Manufacturer Specific Data
        out[pos + 2..pos + 4].copy_from_slice(&self.company_id.to_le_bytes());
        out[pos + 4..pos + 4 + payload.len()].copy_from_slice(payload);
        Some(total)
    }

    /// Assemble a SCAN_RSP PDU (header 0x44) carrying extra AD data a central fetches
    /// after a scan request - lets a scannable advertiser expose more than 31 bytes
    /// total across ADV + SCAN_RSP. `ad` is raw AD structures (each: len, type, body).
    pub fn build_scan_response(&self, ad: &[u8], out: &mut [u8]) -> Option<usize> {
        if ad.len() > 31 {
            return None;
        }
        let total = 2 + 6 + ad.len();
        if out.len() < total {
            return None;
        }
        out[0] = 0x44; // SCAN_RSP, TxAdd = random
        out[1] = (6 + ad.len()) as u8;
        out[2..8].copy_from_slice(self.adv_addr);
        out[8..8 + ad.len()].copy_from_slice(ad);
        Some(total)
    }
}

// ---------------------------------------------------------------- RFID (ISO 14443A)

/// SPI register access a board support package provides for an RFID reader.
///
/// MFRC522 and similar readers use full-duplex SPI transactions for register reads and
/// writes. The trait stays byte-oriented and dependency-free so Arduino, PlatformIO,
/// embedded-hal, or a custom HAL can adapt to it with a tiny shim.
pub trait SpiIo {
    /// Transfer `write` bytes while collecting the same number of bytes in `read`.
    /// Returns false when the bus transaction failed or the device was not selected.
    fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> bool;
}

/// Static RFID reader metadata used by board profiles and app generators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RfidReaderDescriptor {
    pub name: &'static str,
    pub protocol: Protocol,
    pub host_bus: &'static str,
    pub max_uid_len: u8,
    pub fifo_len: u8,
}

pub mod rfid_readers {
    use super::*;

    /// Common RC522 / MFRC522 SPI reader module for ISO 14443A tags.
    pub const MFRC522_SPI: RfidReaderDescriptor = RfidReaderDescriptor {
        name: "MFRC522 SPI reader",
        protocol: Protocol::Rfid,
        host_bus: "spi",
        max_uid_len: 10,
        fifo_len: 64,
    };
}

/// Fixed-size ISO 14443A UID returned by a reader poll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RfidUid {
    bytes: [u8; 10],
    len: u8,
}

impl RfidUid {
    pub const fn empty() -> Self {
        Self {
            bytes: [0; 10],
            len: 0,
        }
    }

    pub fn from_slice(uid: &[u8]) -> Option<Self> {
        if uid.is_empty() || uid.len() > 10 {
            return None;
        }
        let mut out = Self::empty();
        let mut i = 0;
        while i < uid.len() {
            out.bytes[i] = uid[i];
            i += 1;
        }
        out.len = uid.len() as u8;
        Some(out)
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    pub const fn len(&self) -> usize {
        self.len as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Errors reported by the MFRC522 backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RfidError {
    Bus,
    Timeout,
    Collision,
    Protocol,
    BufferTooSmall,
}

/// No-heap MFRC522 / RC522 reader backend for ISO 14443A polling.
pub struct Mfrc522<S: SpiIo> {
    spi: S,
    initialized: bool,
    rx_cache: [u8; 18],
    rx_len: usize,
}

impl<S: SpiIo> Mfrc522<S> {
    pub const fn new(spi: S) -> Self {
        Self {
            spi,
            initialized: false,
            rx_cache: [0; 18],
            rx_len: 0,
        }
    }

    pub fn into_inner(self) -> S {
        self.spi
    }

    pub fn reader_descriptor(&self) -> RfidReaderDescriptor {
        rfid_readers::MFRC522_SPI
    }

    /// Reset and configure the reader for ISO 14443A polling.
    pub fn init(&mut self) -> Result<(), RfidError> {
        self.write_reg(reg::COMMAND, cmd::SOFT_RESET)?;
        self.write_reg(reg::MODE, 0x3D)?;
        self.write_reg(reg::TIMER_MODE, 0x8D)?;
        self.write_reg(reg::TIMER_PRESCALER, 0x3E)?;
        self.write_reg(reg::TIMER_RELOAD_H, 0x00)?;
        self.write_reg(reg::TIMER_RELOAD_L, 0x1E)?;
        self.write_reg(reg::TX_ASK, 0x40)?;
        self.write_reg(reg::TX_MODE, 0x00)?;
        self.write_reg(reg::RX_MODE, 0x00)?;
        self.set_bits(reg::TX_CONTROL, 0x03)?;
        self.initialized = true;
        Ok(())
    }

    /// Send REQA and return the ATQA bytes when a tag answers.
    pub fn request_a(&mut self, out: &mut [u8]) -> Result<usize, RfidError> {
        self.write_reg(reg::BIT_FRAMING, 0x07)?;
        self.transceive(&[0x26], 7, out)
    }

    /// Run cascade-level-1 anticollision and return a 4-byte UID when present.
    pub fn anticollision_level1(&mut self) -> Result<RfidUid, RfidError> {
        let mut frame = [0u8; 5];
        let n = self.transceive(&[0x93, 0x20], 0, &mut frame)?;
        if n != 5 || !rfid::validate_anticollision(&frame) || rfid::has_next_cascade(&frame) {
            return Err(RfidError::Protocol);
        }
        RfidUid::from_slice(&frame[..4]).ok_or(RfidError::Protocol)
    }

    /// Poll for one ISO 14443A card UID. This is non-allocating and bounded by `poll_budget`.
    pub fn poll_uid(&mut self, poll_budget: u16) -> Result<RfidUid, RfidError> {
        let mut atqa = [0u8; 2];
        let mut tries = 0;
        while tries < poll_budget {
            match self.request_a(&mut atqa) {
                Ok(2) => return self.anticollision_level1(),
                Ok(_) => return Err(RfidError::Protocol),
                Err(RfidError::Timeout) => tries += 1,
                Err(e) => return Err(e),
            }
        }
        Err(RfidError::Timeout)
    }

    pub fn transceive(
        &mut self,
        tx: &[u8],
        valid_bits: u8,
        rx: &mut [u8],
    ) -> Result<usize, RfidError> {
        if tx.len() > usize::from(rfid_readers::MFRC522_SPI.fifo_len) {
            return Err(RfidError::BufferTooSmall);
        }
        self.write_reg(reg::COMMAND, cmd::IDLE)?;
        self.write_reg(reg::COMM_IRQ, 0x7F)?;
        self.write_reg(reg::FIFO_LEVEL, 0x80)?;
        for &b in tx {
            self.write_reg(reg::FIFO_DATA, b)?;
        }
        self.write_reg(reg::BIT_FRAMING, valid_bits & 0x07)?;
        self.write_reg(reg::COMMAND, cmd::TRANSCEIVE)?;
        self.set_bits(reg::BIT_FRAMING, 0x80)?;

        let mut polls = 0;
        while polls < 200 {
            let irq = self.read_reg(reg::COMM_IRQ)?;
            if irq & 0x30 != 0 {
                break;
            }
            if irq & 0x01 != 0 {
                self.clear_bits(reg::BIT_FRAMING, 0x80)?;
                return Err(RfidError::Timeout);
            }
            polls += 1;
        }
        if polls == 200 {
            self.clear_bits(reg::BIT_FRAMING, 0x80)?;
            return Err(RfidError::Timeout);
        }

        let err = self.read_reg(reg::ERROR)?;
        if err & 0x08 != 0 {
            self.clear_bits(reg::BIT_FRAMING, 0x80)?;
            return Err(RfidError::Collision);
        }
        if err & 0x13 != 0 {
            self.clear_bits(reg::BIT_FRAMING, 0x80)?;
            return Err(RfidError::Protocol);
        }
        let fifo = usize::from(self.read_reg(reg::FIFO_LEVEL)?);
        if fifo > rx.len() {
            self.clear_bits(reg::BIT_FRAMING, 0x80)?;
            return Err(RfidError::BufferTooSmall);
        }
        let mut i = 0;
        while i < fifo {
            rx[i] = self.read_reg(reg::FIFO_DATA)?;
            i += 1;
        }
        self.clear_bits(reg::BIT_FRAMING, 0x80)?;
        Ok(fifo)
    }

    fn read_reg(&mut self, reg: u8) -> Result<u8, RfidError> {
        let write = [((reg << 1) & 0x7E) | 0x80, 0x00];
        let mut read = [0u8; 2];
        if self.spi.transfer(&write, &mut read) {
            Ok(read[1])
        } else {
            Err(RfidError::Bus)
        }
    }

    fn write_reg(&mut self, reg: u8, value: u8) -> Result<(), RfidError> {
        let write = [(reg << 1) & 0x7E, value];
        let mut read = [0u8; 2];
        if self.spi.transfer(&write, &mut read) {
            Ok(())
        } else {
            Err(RfidError::Bus)
        }
    }

    fn set_bits(&mut self, reg: u8, mask: u8) -> Result<(), RfidError> {
        let v = self.read_reg(reg)?;
        self.write_reg(reg, v | mask)
    }

    fn clear_bits(&mut self, reg: u8, mask: u8) -> Result<(), RfidError> {
        let v = self.read_reg(reg)?;
        self.write_reg(reg, v & !mask)
    }
}

impl<S: SpiIo> WirelessBackend for Mfrc522<S> {
    fn descriptor(&self) -> LinkDescriptor {
        link_catalog::RFID_14443A
    }

    fn link_state(&mut self) -> LinkState {
        if self.initialized {
            LinkState::Up
        } else {
            LinkState::Down
        }
    }

    fn send(&mut self, payload: &[u8]) -> bool {
        let mut tmp = [0u8; 18];
        match self.transceive(payload, 0, &mut tmp) {
            Ok(n) => {
                self.rx_cache[..n].copy_from_slice(&tmp[..n]);
                self.rx_len = n;
                true
            }
            Err(_) => false,
        }
    }

    fn recv(&mut self, buf: &mut [u8]) -> usize {
        if self.rx_len == 0 {
            if let Ok(uid) = self.poll_uid(1) {
                let n = uid.len().min(self.rx_cache.len());
                self.rx_cache[..n].copy_from_slice(uid.as_slice());
                self.rx_len = n;
            }
        }
        let n = self.rx_len.min(buf.len());
        buf[..n].copy_from_slice(&self.rx_cache[..n]);
        self.rx_len = 0;
        n
    }
}

mod reg {
    pub const COMMAND: u8 = 0x01;
    pub const COMM_IRQ: u8 = 0x04;
    pub const ERROR: u8 = 0x06;
    pub const FIFO_DATA: u8 = 0x09;
    pub const FIFO_LEVEL: u8 = 0x0A;
    pub const BIT_FRAMING: u8 = 0x0D;
    pub const MODE: u8 = 0x11;
    pub const TX_MODE: u8 = 0x12;
    pub const RX_MODE: u8 = 0x13;
    pub const TX_CONTROL: u8 = 0x14;
    pub const TX_ASK: u8 = 0x15;
    pub const TIMER_MODE: u8 = 0x2A;
    pub const TIMER_PRESCALER: u8 = 0x2B;
    pub const TIMER_RELOAD_H: u8 = 0x2C;
    pub const TIMER_RELOAD_L: u8 = 0x2D;
}

mod cmd {
    pub const IDLE: u8 = 0x00;
    pub const TRANSCEIVE: u8 = 0x0C;
    pub const SOFT_RESET: u8 = 0x0F;
}
pub mod rfid {
    /// BCC of a 4-byte UID cascade level (XOR of the four bytes, ISO 14443-3 A).
    pub fn iso14443a_bcc(uid: &[u8; 4]) -> u8 {
        uid[0] ^ uid[1] ^ uid[2] ^ uid[3]
    }

    /// Validate a 5-byte anticollision frame (uid0..3 + bcc).
    pub fn validate_anticollision(frame: &[u8; 5]) -> bool {
        iso14443a_bcc(&[frame[0], frame[1], frame[2], frame[3]]) == frame[4]
    }

    /// The cascade-tag byte marking a UID that continues at the next level.
    pub const CASCADE_TAG: u8 = 0x88;

    /// True when a cascade level says the UID continues (7/10-byte UIDs).
    pub fn has_next_cascade(frame: &[u8; 5]) -> bool {
        frame[0] == CASCADE_TAG
    }
}

// ---------------------------------------------------------------- CC2530 802.15.4

/// Byte-level UART access a board provides so the hardware-agnostic CC2530 backend can
/// run on any MCU (the nRF app, an ESP32, etc. each supply their own UART).
pub trait ByteIo {
    /// Transmit one byte (blocking until accepted).
    fn write(&mut self, b: u8);
    /// Poll one received byte, or None if none is pending.
    fn read(&mut self) -> Option<u8>;
}

/// 802.15.4 MAC frame types (low 3 bits of the frame-control field).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacFrameType {
    Beacon,
    Data,
    Ack,
    MacCommand,
    Reserved,
}

/// Classify a raw PSDU by its frame-control field - the same rule the gateway and the
/// host `nobro_rtos.zigbee` contract use, so on-device and host agree.
pub fn mac_frame_type(psdu: &[u8]) -> Option<MacFrameType> {
    let fcf_lo = *psdu.first()?;
    Some(match fcf_lo & 0x7 {
        0 => MacFrameType::Beacon,
        1 => MacFrameType::Data,
        2 => MacFrameType::Ack,
        3 => MacFrameType::MacCommand,
        _ => MacFrameType::Reserved,
    })
}

/// Modular CC2530 802.15.4 backend (M199): drives the NiusZigbee SDCC firmware protocol
/// (`FE LEN CMD DATA FCS`, LEN counts CMD, FCS = XOR of LEN..DATA) over any [`ByteIo`],
/// and presents the common [`WirelessBackend`] surface - so 802.15.4 is mountable
/// like BLE or WiFi. Verified against the live firmware in the cc2530_gateway app (M122).
pub struct Cc2530<U: ByteIo> {
    io: U,
    joined: bool,
    dec: Cc2530Decoder,
}

/// Streaming frame decoder for the CC2530 UART protocol (mirrors the C++ host driver).
struct Cc2530Decoder {
    state: u8,
    len: u8,
    idx: u8,
    fcs: u8,
    buf: [u8; 160],
}

impl Cc2530Decoder {
    const fn new() -> Self {
        Cc2530Decoder {
            state: 0,
            len: 0,
            idx: 0,
            fcs: 0,
            buf: [0; 160],
        }
    }
    /// Feed one byte; returns the frame length (in `buf`) when a valid frame completes.
    fn feed(&mut self, b: u8) -> Option<u8> {
        match self.state {
            0 => {
                if b == 0xFE {
                    self.state = 1;
                }
            }
            1 => {
                self.len = b;
                self.idx = 0;
                self.fcs = b;
                self.state = if b == 0 || b as usize > self.buf.len() {
                    0
                } else {
                    2
                };
            }
            2 => {
                self.buf[self.idx as usize] = b;
                self.idx += 1;
                self.fcs ^= b;
                if self.idx >= self.len {
                    self.state = 3;
                }
            }
            _ => {
                self.state = 0;
                if b == self.fcs {
                    return Some(self.len);
                }
            }
        }
        None
    }
}

impl<U: ByteIo> Cc2530<U> {
    pub fn new(io: U) -> Self {
        Cc2530 {
            io,
            joined: false,
            dec: Cc2530Decoder::new(),
        }
    }

    /// Send a raw command frame (`FE LEN CMD DATA FCS`).
    fn send_cmd(&mut self, cmd: u8, data: &[u8]) {
        let len = (data.len() + 1) as u8;
        let mut fcs = len ^ cmd;
        self.io.write(0xFE);
        self.io.write(len);
        self.io.write(cmd);
        for &b in data {
            self.io.write(b);
            fcs ^= b;
        }
        self.io.write(fcs);
    }

    /// Bring the module up: flush its parser, PING, then set channel + promiscuous RX.
    /// Returns true once a PONG is seen. `poll_budget` bounds the byte-poll wait.
    pub fn join(&mut self, channel: u8, poll_budget: u32) -> bool {
        for _ in 0..140 {
            self.io.write(0x00); // flush any partial frame in the firmware parser
        }
        self.send_cmd(0x01, &[]); // PING
        for _ in 0..poll_budget {
            if let Some(b) = self.io.read() {
                if let Some(_len) = self.dec.feed(b) {
                    if self.dec.buf[0] == 0x81 {
                        // PONG
                        self.send_cmd(0x02, &[channel]); // SET_CHANNEL
                        self.send_cmd(0x04, &[0]); // SET_PROMISC (filter off)
                        self.joined = true;
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Poll for one captured 802.15.4 PSDU into `out`; returns (len, frame_type) once a
    /// full RX_FRAME arrives, or None when no complete frame is pending (non-blocking:
    /// drains available bytes, then yields). Non-RX frames are consumed and skipped.
    pub fn poll_frame(&mut self, out: &mut [u8]) -> Option<(usize, MacFrameType)> {
        while let Some(b) = self.io.read() {
            if let Some(flen) = self.dec.feed(b) {
                let flen = flen as usize;
                if self.dec.buf[0] == 0x84 && flen >= 4 {
                    // buf = [0x84, rssi, lqi, psdu..]
                    let psdu = &self.dec.buf[3..flen];
                    let n = psdu.len().min(out.len());
                    out[..n].copy_from_slice(&psdu[..n]);
                    if let Some(ft) = mac_frame_type(psdu) {
                        return Some((n, ft));
                    }
                }
            }
        }
        None
    }
}

impl<U: ByteIo> WirelessBackend for Cc2530<U> {
    fn descriptor(&self) -> LinkDescriptor {
        link_catalog::ZIGBEE_APS
    }
    fn link_state(&mut self) -> LinkState {
        if self.joined {
            LinkState::Up
        } else {
            LinkState::Down
        }
    }
    fn send(&mut self, payload: &[u8]) -> bool {
        if !self.joined {
            return false;
        }
        self.send_cmd(0x03, payload); // TX raw PSDU
        true
    }
    fn recv(&mut self, buf: &mut [u8]) -> usize {
        self.poll_frame(buf).map(|(n, _)| n).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_distinguishes_link_shapes() {
        assert!(link_catalog::BLE_ADV.broadcast_only);
        assert!(!link_catalog::BLE_ADV.requires_join);
        assert!(link_catalog::WIFI_TCP.requires_join);
        assert!(link_catalog::ZIGBEE_APS.mtu < link_catalog::WIFI_TCP.mtu);
        assert_eq!(link_catalog::RFID_14443A.protocol, Protocol::Rfid);
        assert_eq!(link_catalog::RFID_14443A.mtu, 18);
        assert_eq!(rfid_readers::MFRC522_SPI.host_bus, "spi");
        assert_eq!(rfid_readers::MFRC522_SPI.max_uid_len, 10);
    }

    #[test]
    fn adv_builder_reproduces_the_on_air_format() {
        // Same identity ble_adv_demo used on air (M123).
        let addr = [0x4E, 0x42, 0x52, 0x4F, 0x01, 0xC3];
        let b = BleAdvBuilder {
            adv_addr: &addr,
            name: b"NOBRO",
            company_id: 0xFFFF,
        };
        let mut pdu = [0u8; 39];
        let payload = [1u8, 0, 0, 0, 1]; // beat=1, status=1
        let n = b.build(&payload, &mut pdu).unwrap();
        assert_eq!(n, 24); // 2 hdr + 6 AdvA + 7 name AD + 9 mfr AD
        assert_eq!(pdu[0], 0x42); // ADV_NONCONN_IND + random TxAdd
        assert_eq!(pdu[1], 22); // AdvA + ADs
        assert_eq!(&pdu[2..8], &addr);
        assert_eq!(pdu[8], 6); // name AD length
        assert_eq!(pdu[9], 0x09);
        assert_eq!(&pdu[10..15], b"NOBRO");
        assert_eq!(pdu[15], 8); // mfr AD length: type + company(2) + payload(5)
        assert_eq!(pdu[16], 0xFF);
        assert_eq!(&pdu[17..19], &[0xFF, 0xFF]);
        assert_eq!(&pdu[19..24], &payload);
    }

    #[test]
    fn adv_kind_selects_the_on_air_header() {
        let addr = [0x4E, 0x42, 0x52, 0x4F, 0x01, 0xC3];
        let b = BleAdvBuilder {
            adv_addr: &addr,
            name: b"NOBRO",
            company_id: 0xFFFF,
        };
        let mut pdu = [0u8; 39];
        let payload = [1u8, 0, 0, 0, 1];
        // non-connectable (default) keeps the M123-verified 0x42 header
        assert_eq!(b.build(&payload, &mut pdu).unwrap(), 24);
        assert_eq!(pdu[0], 0x42);
        // connectable ADV_IND
        b.build_as(AdvKind::Connectable, &payload, &mut pdu)
            .unwrap();
        assert_eq!(pdu[0], 0x40);
        // scannable ADV_SCAN_IND
        b.build_as(AdvKind::Scannable, &payload, &mut pdu).unwrap();
        assert_eq!(pdu[0], 0x46);
        // payload + addressing are identical regardless of kind
        assert_eq!(&pdu[2..8], &addr);
        assert_eq!(&pdu[19..24], &payload);
    }

    #[test]
    fn scan_response_carries_extra_ad() {
        let addr = [1u8, 2, 3, 4, 5, 0xC3];
        let b = BleAdvBuilder {
            adv_addr: &addr,
            name: b"N",
            company_id: 0,
        };
        let mut rsp = [0u8; 39];
        // one AD: Tx Power Level (type 0x0A) = -4 dBm
        let ad = [0x02, 0x0A, 0xFC];
        let n = b.build_scan_response(&ad, &mut rsp).unwrap();
        assert_eq!(rsp[0], 0x44); // SCAN_RSP
        assert_eq!(n, 2 + 6 + 3);
        assert_eq!(&rsp[8..11], &ad);
    }

    #[test]
    fn adv_builder_enforces_the_31_byte_budget() {
        let addr = [0u8; 6];
        let b = BleAdvBuilder {
            adv_addr: &addr,
            name: b"WAY-TOO-LONG-DEVICE-NAME",
            company_id: 0,
        };
        let mut pdu = [0u8; 39];
        assert!(b.build(&[0u8; 8], &mut pdu).is_none());
    }

    #[test]
    fn mac_frame_type_matches_the_host_contract() {
        assert_eq!(mac_frame_type(&[0x02, 0x00]), Some(MacFrameType::Ack));
        assert_eq!(
            mac_frame_type(&[0x03, 0x08]),
            Some(MacFrameType::MacCommand)
        );
        assert_eq!(mac_frame_type(&[0x61, 0x88]), Some(MacFrameType::Data));
        assert_eq!(mac_frame_type(&[0x00, 0x80]), Some(MacFrameType::Beacon));
        assert_eq!(mac_frame_type(&[]), None);
    }

    // Scripted UART: replays firmware bytes and captures what the backend transmits.
    struct FakeUart {
        rx: HeaplessVec,
        rx_pos: usize,
        tx: HeaplessVec,
    }
    // A tiny fixed-capacity byte vec so the test stays no_std/no-heap like the crate.
    struct HeaplessVec {
        data: [u8; 256],
        len: usize,
    }
    impl Default for HeaplessVec {
        fn default() -> Self {
            HeaplessVec {
                data: [0; 256],
                len: 0,
            }
        }
    }
    impl HeaplessVec {
        fn push(&mut self, b: u8) {
            if self.len < self.data.len() {
                self.data[self.len] = b;
                self.len += 1;
            }
        }
        fn extend(&mut self, bs: &[u8]) {
            for &b in bs {
                self.push(b);
            }
        }
    }
    impl ByteIo for FakeUart {
        fn write(&mut self, b: u8) {
            self.tx.push(b);
        }
        fn read(&mut self) -> Option<u8> {
            if self.rx_pos < self.rx.len {
                let b = self.rx.data[self.rx_pos];
                self.rx_pos += 1;
                Some(b)
            } else {
                None
            }
        }
    }

    fn frame(cmd: u8, data: &[u8]) -> ([u8; 8], usize) {
        // build FE LEN CMD DATA FCS for short frames (test helper)
        let len = (data.len() + 1) as u8;
        let mut fcs = len ^ cmd;
        let mut out = [0u8; 8];
        out[0] = 0xFE;
        out[1] = len;
        out[2] = cmd;
        let mut n = 3;
        for &b in data {
            out[n] = b;
            fcs ^= b;
            n += 1;
        }
        out[n] = fcs;
        n += 1;
        (out, n)
    }

    #[test]
    fn cc2530_backend_joins_and_captures_a_frame() {
        let mut rx = HeaplessVec::default();
        // firmware replies PONG [ver_hi ver_lo], then an RX_FRAME with a beacon-request PSDU
        let (pong, pn) = frame(0x81, &[0, 8]);
        rx.extend(&pong[..pn]);
        // RX_FRAME 0x84 [rssi lqi psdu(03 08 EA FF FF FF FF 07)]
        let (rxf, rn) = {
            let psdu = [0x03u8, 0x08, 0xEA, 0xFF, 0xFF, 0xFF, 0xFF, 0x07];
            let mut data = [0u8; 10];
            data[0] = 200; // rssi
            data[1] = 255; // lqi
            data[2..10].copy_from_slice(&psdu);
            // reuse a bigger frame builder inline
            let len = (data.len() + 1) as u8;
            let mut fcs = len ^ 0x84;
            let mut out = [0u8; 16];
            out[0] = 0xFE;
            out[1] = len;
            out[2] = 0x84;
            for (i, &b) in data.iter().enumerate() {
                out[3 + i] = b;
                fcs ^= b;
            }
            out[3 + data.len()] = fcs;
            (out, 3 + data.len() + 1)
        };
        rx.extend(&rxf[..rn]);

        let uart = FakeUart {
            rx,
            rx_pos: 0,
            tx: HeaplessVec::default(),
        };
        let mut radio = Cc2530::new(uart);
        assert!(radio.join(11, 10_000));
        assert_eq!(radio.link_state(), LinkState::Up);
        assert_eq!(radio.descriptor().protocol, Protocol::Zigbee);

        // the join should have transmitted PING, SET_CHANNEL(11), SET_PROMISC(0)
        assert!(radio.io.tx.data[..radio.io.tx.len].contains(&11));

        let mut buf = [0u8; 32];
        let (n, ft) = radio.poll_frame(&mut buf).expect("captured frame");
        assert_eq!(ft, MacFrameType::MacCommand);
        assert_eq!(&buf[..n], &[0x03, 0x08, 0xEA, 0xFF, 0xFF, 0xFF, 0xFF, 0x07]);
    }

    #[test]
    fn rfid_bcc_checks_anticollision_frames() {
        let uid = [0xDE, 0xAD, 0xBE, 0xEF];
        let bcc = rfid::iso14443a_bcc(&uid);
        assert!(rfid::validate_anticollision(&[0xDE, 0xAD, 0xBE, 0xEF, bcc]));
        assert!(!rfid::validate_anticollision(&[
            0xDE,
            0xAD,
            0xBE,
            0xEF,
            bcc ^ 1
        ]));
        assert!(rfid::has_next_cascade(&[
            rfid::CASCADE_TAG,
            1,
            2,
            3,
            0x88 ^ 1 ^ 2 ^ 3
        ]));
    }

    #[derive(Clone, Copy)]
    enum RfidReply {
        Atqa,
        Uid,
        None,
    }

    struct FakeSpi {
        regs: [u8; 64],
        fifo: [u8; 64],
        fifo_len: usize,
        wrote_request_a: bool,
        wrote_anticoll: bool,
    }

    impl Default for FakeSpi {
        fn default() -> Self {
            Self {
                regs: [0; 64],
                fifo: [0; 64],
                fifo_len: 0,
                wrote_request_a: false,
                wrote_anticoll: false,
            }
        }
    }

    impl FakeSpi {
        fn prepare_reply(&mut self, reply: RfidReply) {
            match reply {
                RfidReply::Atqa => {
                    self.fifo[..2].copy_from_slice(&[0x04, 0x00]);
                    self.fifo_len = 2;
                }
                RfidReply::Uid => {
                    let uid = [0xDE, 0xAD, 0xBE, 0xEF];
                    self.fifo[..4].copy_from_slice(&uid);
                    self.fifo[4] = rfid::iso14443a_bcc(&uid);
                    self.fifo_len = 5;
                }
                RfidReply::None => self.fifo_len = 0,
            }
            self.regs[reg::FIFO_LEVEL as usize] = self.fifo_len as u8;
            self.regs[reg::COMM_IRQ as usize] = 0x30;
            self.regs[reg::ERROR as usize] = 0;
        }
    }

    impl SpiIo for FakeSpi {
        fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> bool {
            if write.len() != 2 || read.len() != 2 {
                return false;
            }
            let reg = (write[0] >> 1) & 0x3F;
            if write[0] & 0x80 != 0 {
                read[1] = if reg == super::reg::FIFO_DATA {
                    let b = self.fifo[0];
                    if self.fifo_len > 0 {
                        let mut i = 1;
                        while i < self.fifo_len {
                            self.fifo[i - 1] = self.fifo[i];
                            i += 1;
                        }
                        self.fifo_len -= 1;
                        self.regs[super::reg::FIFO_LEVEL as usize] = self.fifo_len as u8;
                    }
                    b
                } else {
                    self.regs[reg as usize]
                };
            } else {
                self.regs[reg as usize] = write[1];
                if reg == super::reg::FIFO_LEVEL && write[1] & 0x80 != 0 {
                    self.fifo_len = 0;
                    self.wrote_request_a = false;
                    self.wrote_anticoll = false;
                }
                if reg == super::reg::FIFO_DATA {
                    self.fifo[self.fifo_len] = write[1];
                    self.fifo_len += 1;
                    if write[1] == 0x26 {
                        self.wrote_request_a = true;
                    }
                    if self.fifo_len >= 2 && self.fifo[0] == 0x93 && self.fifo[1] == 0x20 {
                        self.wrote_anticoll = true;
                    }
                }
                if reg == super::reg::COMMAND && write[1] == super::cmd::TRANSCEIVE {
                    if self.wrote_anticoll {
                        self.prepare_reply(RfidReply::Uid);
                    } else if self.wrote_request_a {
                        self.prepare_reply(RfidReply::Atqa);
                    } else {
                        self.prepare_reply(RfidReply::None);
                    }
                }
            }
            true
        }
    }

    #[test]
    fn mfrc522_backend_polls_uid_and_mounts_as_wireless_backend() {
        let spi = FakeSpi::default();
        let mut reader = Mfrc522::new(spi);
        assert_eq!(reader.link_state(), LinkState::Down);
        assert!(reader.init().is_ok());
        assert_eq!(reader.link_state(), LinkState::Up);
        assert_eq!(reader.descriptor().protocol, Protocol::Rfid);
        assert_eq!(reader.reader_descriptor(), rfid_readers::MFRC522_SPI);

        let uid = reader.poll_uid(1).expect("uid");
        assert_eq!(uid.as_slice(), &[0xDE, 0xAD, 0xBE, 0xEF]);

        let mut out = [0u8; 10];
        assert_eq!(reader.recv(&mut out), 4);
        assert_eq!(&out[..4], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }
    // A tiny in-memory transport proves the trait surface composes.
    struct LoopbackRadio {
        buf: [u8; 64],
        len: usize,
    }
    impl WirelessBackend for LoopbackRadio {
        fn descriptor(&self) -> LinkDescriptor {
            link_catalog::NRF_PROPRIETARY
        }
        fn link_state(&mut self) -> LinkState {
            LinkState::Up
        }
        fn send(&mut self, payload: &[u8]) -> bool {
            if payload.len() > usize::from(self.descriptor().mtu) {
                return false;
            }
            self.buf[..payload.len()].copy_from_slice(payload);
            self.len = payload.len();
            true
        }
        fn recv(&mut self, buf: &mut [u8]) -> usize {
            let n = self.len.min(buf.len());
            buf[..n].copy_from_slice(&self.buf[..n]);
            self.len = 0;
            n
        }
    }

    #[test]
    fn transport_trait_round_trips() {
        let mut radio = LoopbackRadio {
            buf: [0; 64],
            len: 0,
        };
        assert_eq!(radio.link_state(), LinkState::Up);
        assert!(radio.send(b"nobro-wireless"));
        assert!(!radio.send(&[0u8; 61])); // over the proprietary MTU
        let mut rx = [0u8; 64];
        assert_eq!(radio.recv(&mut rx), 14);
        assert_eq!(&rx[..14], b"nobro-wireless");
        assert_eq!(radio.recv(&mut rx), 0); // drained
    }

    #[test]
    fn managed_link_enforces_deadline_mtu_and_window() {
        let backend = LoopbackRadio {
            buf: [0; 64],
            len: 0,
        };
        let mut link = ManagedLink::new(backend, LinkBudget::new(32, 1, 8));
        assert_eq!(
            link.send_at(11, TxContract::by(10), b"late"),
            Err(LinkError::DeadlineElapsed)
        );
        assert_eq!(
            link.send_at(1, TxContract::by(10), &[0; 33]),
            Err(LinkError::PayloadTooLarge)
        );
        assert!(link.send_at(1, TxContract::by(10), b"12345678").is_ok());
        assert_eq!(
            link.send_at(1, TxContract::by(10), b"x"),
            Err(LinkError::WindowExhausted)
        );
        link.reset_window();
        assert!(link.send_at(2, TxContract::by(10), b"x").is_ok());
        assert_eq!(link.diagnostics().tx_accepted, 2);
        assert_eq!(link.diagnostics().tx_rejected, 3);
    }
}
