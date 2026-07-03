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

impl<'a> BleAdvBuilder<'a> {
    /// Assemble the PDU into `out` with `payload` as the manufacturer data body.
    /// Returns the total PDU length, or None when it would exceed the legacy budget.
    pub fn build(&self, payload: &[u8], out: &mut [u8]) -> Option<usize> {
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
        out[0] = 0x42; // ADV_NONCONN_IND, TxAdd = random
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
    fn adv_builder_enforces_the_31_byte_budget() {
        let addr = [0u8; 6];
        let b = BleAdvBuilder { adv_addr: &addr, name: b"WAY-TOO-LONG-DEVICE-NAME", company_id: 0 };
        let mut pdu = [0u8; 39];
        assert!(b.build(&[0u8; 8], &mut pdu).is_none());
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
