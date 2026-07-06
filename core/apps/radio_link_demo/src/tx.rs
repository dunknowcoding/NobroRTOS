//! Radio TX node: stream a sequence counter over the nRF RADIO for a peer running
//! radio_rx to collect. Seals NOBRO_RADIO_REPORT (role=TX); on one node, a growing
//! tx_sent confirms the RADIO transmit path drives end-to-end.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use nobro_hal::Radio;
use panic_halt as _;

#[path = "common.rs"]
mod common;
use common::{checksum, start_hfxo, RadioReport, MAGIC};

#[no_mangle]
#[used]
static mut NOBRO_RADIO_REPORT: RadioReport = RadioReport::zero();

#[entry]
fn main() -> ! {
    start_hfxo();
    unsafe {
        Radio::init();
    }

    let mut seq: u32 = 0;
    let mut tx_sent: u32 = 0;
    loop {
        seq = seq.wrapping_add(1);
        // Payload: a marker + the little-endian sequence number.
        let mut pkt = [0u8; 5];
        pkt[0] = 0xA5;
        pkt[1..5].copy_from_slice(&seq.to_le_bytes());
        if Radio::send(&pkt) {
            tx_sent = tx_sent.wrapping_add(1);
        }

        let mut r = RadioReport {
            magic: MAGIC,
            version: 1,
            role: 1,
            tx_sent,
            rx_received: 0,
            last_seq: seq,
            all_pass: u32::from(tx_sent >= 10),
            checksum: 0,
        };
        r.checksum = checksum(&r);
        unsafe {
            NOBRO_RADIO_REPORT = r;
        }

        // ~few ms between packets so a polling receiver can keep up.
        for _ in 0..200_000u32 {
            cortex_m::asm::nop();
        }
    }
}
