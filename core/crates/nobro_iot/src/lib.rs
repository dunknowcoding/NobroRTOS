//! IoT capsule: wireless-communication apps and APIs behind one surface.
//!
//! Follows the nobro_usb mountable-backend pattern: a radio implements
//! [`IotTransport`], apps talk bytes + link state, and protocol identity/limits are
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
        mtu: 16, // classic sector-fragment granularity
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
pub enum IotLinkState {
    Down,
    Joining,
    Up,
}

/// The mountable radio surface. One backend per physical radio, chosen per board -
/// the same pattern as `nobro_usb::UsbStack`.
pub trait IotTransport {
    fn descriptor(&self) -> LinkDescriptor;
    fn link_state(&mut self) -> IotLinkState;
    /// Send one payload (<= mtu); returns true when the radio accepted it.
    fn send(&mut self, payload: &[u8]) -> bool;
    /// Receive into `buf`; returns bytes delivered (0 = nothing pending).
    fn recv(&mut self, buf: &mut [u8]) -> usize;
}

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
/// and presents the common [`IotTransport`] surface - so 802.15.4 is a mountable radio
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
        Cc2530Decoder { state: 0, len: 0, idx: 0, fcs: 0, buf: [0; 160] }
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
                self.state = if b == 0 || b as usize > self.buf.len() { 0 } else { 2 };
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
        Cc2530 { io, joined: false, dec: Cc2530Decoder::new() }
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

impl<U: ByteIo> IotTransport for Cc2530<U> {
    fn descriptor(&self) -> LinkDescriptor {
        link_catalog::ZIGBEE_APS
    }
    fn link_state(&mut self) -> IotLinkState {
        if self.joined {
            IotLinkState::Up
        } else {
            IotLinkState::Down
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
    }

    #[test]
    fn adv_builder_reproduces_the_on_air_format() {
        // Same identity ble_adv_demo used on air (M123).
        let addr = [0x4E, 0x42, 0x52, 0x4F, 0x01, 0xC3];
        let b = BleAdvBuilder { adv_addr: &addr, name: b"NOBRO", company_id: 0xFFFF };
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
        let b = BleAdvBuilder { adv_addr: &addr, name: b"NOBRO", company_id: 0xFFFF };
        let mut pdu = [0u8; 39];
        let payload = [1u8, 0, 0, 0, 1];
        // non-connectable (default) keeps the M123-verified 0x42 header
        assert_eq!(b.build(&payload, &mut pdu).unwrap(), 24);
        assert_eq!(pdu[0], 0x42);
        // connectable ADV_IND
        b.build_as(AdvKind::Connectable, &payload, &mut pdu).unwrap();
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
        let b = BleAdvBuilder { adv_addr: &addr, name: b"N", company_id: 0 };
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
        let b = BleAdvBuilder { adv_addr: &addr, name: b"WAY-TOO-LONG-DEVICE-NAME", company_id: 0 };
        let mut pdu = [0u8; 39];
        assert!(b.build(&[0u8; 8], &mut pdu).is_none());
    }

    #[test]
    fn mac_frame_type_matches_the_host_contract() {
        assert_eq!(mac_frame_type(&[0x02, 0x00]), Some(MacFrameType::Ack));
        assert_eq!(mac_frame_type(&[0x03, 0x08]), Some(MacFrameType::MacCommand));
        assert_eq!(mac_frame_type(&[0x61, 0x88]), Some(MacFrameType::Data));
        assert_eq!(mac_frame_type(&[0x00, 0x80]), Some(MacFrameType::Beacon));
        assert_eq!(mac_frame_type(&[]), None);
    }

    // Scripted UART: replays firmware bytes and captures what the backend transmits.
    struct FakeUart {
        rx: heapless_vec,
        rx_pos: usize,
        tx: heapless_vec,
    }
    // A tiny fixed-capacity byte vec so the test stays no_std/no-heap like the crate.
    struct heapless_vec {
        data: [u8; 256],
        len: usize,
    }
    impl Default for heapless_vec {
        fn default() -> Self {
            heapless_vec { data: [0; 256], len: 0 }
        }
    }
    impl heapless_vec {
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
        let mut rx = heapless_vec::default();
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

        let uart = FakeUart { rx, rx_pos: 0, tx: heapless_vec::default() };
        let mut radio = Cc2530::new(uart);
        assert!(radio.join(11, 10_000));
        assert_eq!(radio.link_state(), IotLinkState::Up);
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
        assert!(!rfid::validate_anticollision(&[0xDE, 0xAD, 0xBE, 0xEF, bcc ^ 1]));
        assert!(rfid::has_next_cascade(&[rfid::CASCADE_TAG, 1, 2, 3, 0x88 ^ 1 ^ 2 ^ 3]));
    }

    // A tiny in-memory transport proves the trait surface composes.
    struct LoopbackRadio {
        buf: [u8; 64],
        len: usize,
    }
    impl IotTransport for LoopbackRadio {
        fn descriptor(&self) -> LinkDescriptor {
            link_catalog::NRF_PROPRIETARY
        }
        fn link_state(&mut self) -> IotLinkState {
            IotLinkState::Up
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
        let mut radio = LoopbackRadio { buf: [0; 64], len: 0 };
        assert_eq!(radio.link_state(), IotLinkState::Up);
        assert!(radio.send(b"nobro-iot"));
        assert!(!radio.send(&[0u8; 61])); // over the proprietary MTU
        let mut rx = [0u8; 64];
        assert_eq!(radio.recv(&mut rx), 9);
        assert_eq!(&rx[..9], b"nobro-iot");
        assert_eq!(radio.recv(&mut rx), 0); // drained
    }
}
