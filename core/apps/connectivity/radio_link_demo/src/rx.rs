//! Radio RX node: collect sequence packets from a peer running radio_tx and seal
//! NOBRO_RADIO_REPORT (role=RX). A growing rx_received with the latest last_seq
//! confirms over-the-air delivery between two NobroRTOS boards.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use nobro_hal::RadioSession;
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
    let radio =
        unsafe { RadioSession::acquire(5) }.unwrap_or_else(|_| defmt::panic!("radio session"));

    let mut rx_received: u32 = 0;
    let mut last_seq: u32 = 0;
    loop {
        let mut buf = [0u8; 32];
        if let Ok(Some(n)) = radio.recv(&mut buf, 1_000_000) {
            // Payload: 0xA5 marker + 4-byte little-endian sequence.
            if n >= 5 && buf[0] == 0xA5 {
                rx_received = rx_received.wrapping_add(1);
                last_seq = u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]);
            }
        }

        let mut r = RadioReport {
            magic: MAGIC,
            version: 1,
            role: 2,
            tx_sent: 0,
            rx_received,
            last_seq,
            all_pass: u32::from(rx_received >= 10),
            checksum: 0,
        };
        r.checksum = checksum(&r);
        unsafe {
            NOBRO_RADIO_REPORT = r;
        }
    }
}
