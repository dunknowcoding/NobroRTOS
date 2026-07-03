//! BLE peripheral telemetry without a SoftDevice (M123).
//!
//! Drives the nRF52840 RADIO directly in Ble1Mbit mode and broadcasts legacy
//! ADV_NONCONN_IND packets on the three advertising channels. The advertisement
//! carries the device name "NOBRO" plus manufacturer-specific data with live
//! telemetry (a beat counter + status), so any standard BLE scanner - a phone app or
//! `tools/ble_scan.py` on a PC - receives NobroRTOS telemetry with no pairing, no
//! connection, and no BLE stack on either side of the firmware.
//!
//! Advertising PHY facts used below (Bluetooth Core Spec, LE 1M uncoded):
//!   access address 0x8E89BED6, CRC-24 poly 0x65B init 0x555555 (skips the address),
//!   data whitening seeded with the channel index, channels 37/38/39 = 2402/2426/2480 MHz.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

const RADIO_BASE: usize = 0x4000_1000;

macro_rules! radio_reg {
    ($name:ident, $offset:expr) => {
        const $name: *mut u32 = (RADIO_BASE + $offset) as *mut u32;
    };
}

radio_reg!(TASKS_TXEN, 0x000);
radio_reg!(TASKS_DISABLE, 0x010);
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

#[entry]
fn main() -> ! {
    start_hfxo();
    ble_radio_init();

    // ADV_NONCONN_IND with a random static address (top two bits set) and two AD
    // structures: complete local name "NOBRO" + manufacturer data (0xFFFF test id)
    // carrying [beat u32 LE, status u8].
    let mut pdu = [0u8; 39];
    pdu[0] = 0x42; // header: ADV_NONCONN_IND, TxAdd = random
    pdu[1] = 6 + 7 + 10; // AdvA + name AD + manufacturer AD
    pdu[2..8].copy_from_slice(&[0x4E, 0x42, 0x52, 0x4F, 0x01, 0xC3]); // AdvA (LE; MSB 0xC3 = static)
    pdu[8] = 6; // name AD length
    pdu[9] = 0x09; // Complete Local Name
    pdu[10..15].copy_from_slice(b"NOBRO");
    pdu[15] = 9; // manufacturer AD length
    pdu[16] = 0xFF; // Manufacturer Specific Data
    pdu[17] = 0xFF; // company id 0xFFFF (test/prototyping)
    pdu[18] = 0xFF;
    // pdu[19..23] = beat, pdu[23] = status, pdu[24] = 0 spare

    let mut beat: u32 = 0;
    loop {
        beat = beat.wrapping_add(1);
        pdu[19..23].copy_from_slice(&beat.to_le_bytes());
        pdu[23] = 1; // status: alive/all_pass

        for (freq, iv) in ADV_CHANNELS {
            ble_send(&pdu[..25], freq, iv);
        }

        // ~100 ms advertising interval
        for _ in 0..1_600_000u32 {
            cortex_m::asm::nop();
        }
    }
}
