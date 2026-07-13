//! BLE peripheral telemetry without a SoftDevice (M123).
//!
//! Drives the nRF52840 RADIO directly in Ble1Mbit mode and broadcasts legacy
//! ADV_NONCONN_IND packets on the three advertising channels. The advertisement
//! carries the device name "NOBRO" plus manufacturer-specific data with live
//! telemetry (a beat counter + status), so any standard BLE scanner - a phone app or
//! a standard BLE scanner - receives NobroRTOS telemetry with no pairing, no
//! connection, and no BLE stack on either side of the firmware.
//!
//! Advertising PHY facts used below (Bluetooth Core Spec, LE 1M uncoded):
//!   access address 0x8E89BED6, CRC-24 poly 0x65B init 0x555555 (skips the address),
//!   data whitening seeded with the channel index, channels 37/38/39 = 2402/2426/2480 MHz.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use nobro_iot::{link_catalog, BleAdvBuilder, IotLinkState, IotTransport, LinkDescriptor};
use panic_halt as _;

const RADIO_BASE: usize = 0x4000_1000;

macro_rules! radio_reg {
    ($name:ident, $offset:expr) => {
        const $name: *mut u32 = (RADIO_BASE + $offset) as *mut u32;
    };
}

radio_reg!(TASKS_TXEN, 0x000);
radio_reg!(EVENTS_DISABLED, 0x110);
radio_reg!(SHORTS, 0x200);
radio_reg!(PACKETPTR, 0x504);
radio_reg!(FREQUENCY, 0x508);
radio_reg!(TXPOWER, 0x50C);
radio_reg!(MODE, 0x510);
radio_reg!(PCNF0, 0x514);
radio_reg!(PCNF1, 0x518);
radio_reg!(BASE0, 0x51C);
radio_reg!(PREFIX0, 0x524);
radio_reg!(TXADDRESS, 0x52C);
radio_reg!(CRCCNF, 0x534);
radio_reg!(CRCPOLY, 0x538);
radio_reg!(CRCINIT, 0x53C);
radio_reg!(DATAWHITEIV, 0x554);

/// (frequency register value, whitening IV) per advertising channel.
const ADV_CHANNELS: [(u32, u32); 3] = [(2, 37), (26, 38), (80, 39)];

/// Start the external 32 MHz crystal (HFXO) - required by the radio.
fn start_hfxo() {
    unsafe {
        core::ptr::write_volatile(0x4000_0000 as *mut u32, 1); // CLOCK.TASKS_HFCLKSTART
        while core::ptr::read_volatile(0x4000_0100 as *const u32) == 0 {}
    }
}

/// Configure the RADIO for LE 1M advertising once; per-packet state is set in `send`.
fn ble_radio_init() {
    unsafe {
        MODE.write_volatile(3); // Ble1Mbit
        TXPOWER.write_volatile(0); // 0 dBm
                                   // On-air PDU framing: S0 = 1 byte (header), LENGTH = 8 bits, S1 = none.
        PCNF0.write_volatile(8 | (1 << 8));
        // Max 39-byte PDU, 3-byte base address, little-endian, whitening on.
        PCNF1.write_volatile(39 | (3 << 16) | (1 << 25));
        BASE0.write_volatile(0x89BE_D600);
        PREFIX0.write_volatile(0x8E);
        TXADDRESS.write_volatile(0);
        CRCCNF.write_volatile(3 | (1 << 8)); // 3-byte CRC, skip the access address
        CRCPOLY.write_volatile(0x0000_065B);
        CRCINIT.write_volatile(0x0055_5555);
        // READY starts TX immediately; END disables - one task kick per packet.
        SHORTS.write_volatile((1 << 0) | (1 << 1));
    }
}

/// Transmit one already-formed PDU on one advertising channel (blocking, poll-driven).
fn ble_send(pdu: &[u8], freq: u32, white_iv: u32) {
    unsafe {
        FREQUENCY.write_volatile(freq);
        DATAWHITEIV.write_volatile(white_iv);
        PACKETPTR.write_volatile(pdu.as_ptr() as u32);
        EVENTS_DISABLED.write_volatile(0);
        TASKS_TXEN.write_volatile(1);
        while EVENTS_DISABLED.read_volatile() == 0 {}
    }
}

/// The nRF52840 BLE-advertising backend of `nobro_iot::IotTransport` (M219): mounting
/// it owns the RADIO; `send` broadcasts one manufacturer-data payload on all three
/// advertising channels through the crate's PDU builder. Broadcast-only, so `recv`
/// never delivers.
struct BleAdvRadio {
    adv_addr: [u8; 6],
}

impl BleAdvRadio {
    fn mount(adv_addr: [u8; 6]) -> Self {
        start_hfxo();
        ble_radio_init();
        BleAdvRadio { adv_addr }
    }
}

impl IotTransport for BleAdvRadio {
    fn descriptor(&self) -> LinkDescriptor {
        link_catalog::BLE_ADV
    }
    fn link_state(&mut self) -> IotLinkState {
        IotLinkState::Up // advertising needs no join
    }
    fn send(&mut self, payload: &[u8]) -> bool {
        let builder = BleAdvBuilder {
            adv_addr: &self.adv_addr,
            name: b"NOBRO",
            company_id: 0xFFFF,
        };
        let mut pdu = [0u8; 39];
        let Some(len) = builder.build(payload, &mut pdu) else {
            return false;
        };
        for (freq, iv) in ADV_CHANNELS {
            ble_send(&pdu[..len], freq, iv);
        }
        true
    }
    fn recv(&mut self, _buf: &mut [u8]) -> usize {
        0
    }
}

/// The app side is radio-agnostic: it talks to any `IotTransport`, so swapping BLE
/// advertising for WiFi TCP or the proprietary radio is a different `mount`, not a
/// different app.
fn telemetry_loop(radio: &mut impl IotTransport) -> ! {
    let mut beat: u32 = 0;
    loop {
        beat = beat.wrapping_add(1);
        let mut payload = [0u8; 6];
        payload[0..4].copy_from_slice(&beat.to_le_bytes());
        payload[4] = 1; // status: alive/all_pass
        if radio.link_state() == IotLinkState::Up {
            if !radio.send(&payload) {
                defmt::warn!("advertising send failed");
            }
        }

        // ~100 ms advertising interval
        for _ in 0..1_600_000u32 {
            cortex_m::asm::nop();
        }
    }
}

#[entry]
fn main() -> ! {
    let mut radio = BleAdvRadio::mount([0x4E, 0x42, 0x52, 0x4F, 0x01, 0xC3]);
    telemetry_loop(&mut radio)
}
