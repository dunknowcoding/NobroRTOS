//! Data-collection source node: read the reference SPI MPU-9250 and broadcast its accel
//! magnitude over the radio for a peer to collect. Seals NOBRO_RADIO_REPORT (role=3)
//! with the live accel in `last_seq`, so one node verifies the whole sensor ->
//! radio pipeline (M23 + M26 combined).
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use embedded_hal::spi::SpiDevice as _;
use nobro_eh_spi::NobroSpiDevice;
use nobro_hal::{board, Radio};
use panic_halt as _;

#[path = "common.rs"]
mod common;
use common::{checksum, start_hfxo, RadioReport, MAGIC};

#[no_mangle]
#[used]
static mut NOBRO_RADIO_REPORT: RadioReport = RadioReport::zero();

fn rd(dev: &mut NobroSpiDevice, reg: u8) -> u8 {
    let mut rx = [0u8; 2];
    let _ = dev.transfer(&mut rx, &[0x80 | reg, 0]);
    rx[1]
}

fn wr(dev: &mut NobroSpiDevice, reg: u8, val: u8) {
    let _ = dev.write(&[reg & 0x7F, val]);
}

fn isqrt(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

#[entry]
fn main() -> ! {
    start_hfxo();
    let mut dev = unsafe {
        NobroSpiDevice::new(
            board::SPI_SCK_PIN,
            board::SPI_MOSI_PIN,
            board::SPI_MISO_PIN,
            board::SPI_CS_PIN,
        )
    };
    // MPU-9250 bring-up over SPI (reset, wake, SPI-only, +/-2 g).
    wr(&mut dev, 0x6B, 0x80);
    for _ in 0..400_000u32 {
        cortex_m::asm::nop();
    }
    wr(&mut dev, 0x6B, 0x01);
    wr(&mut dev, 0x6A, 0x10);
    wr(&mut dev, 0x6C, 0x00);
    wr(&mut dev, 0x1C, 0x00);
    for _ in 0..400_000u32 {
        cortex_m::asm::nop();
    }
    unsafe {
        Radio::init();
    }

    let mut seq: u32 = 0;
    let mut tx_sent: u32 = 0;
    loop {
        seq = seq.wrapping_add(1);
        let ax = i16::from_be_bytes([rd(&mut dev, 0x3B), rd(&mut dev, 0x3C)]);
        let ay = i16::from_be_bytes([rd(&mut dev, 0x3D), rd(&mut dev, 0x3E)]);
        let az = i16::from_be_bytes([rd(&mut dev, 0x3F), rd(&mut dev, 0x40)]);
        let sq = (i64::from(ax) * i64::from(ax)
            + i64::from(ay) * i64::from(ay)
            + i64::from(az) * i64::from(az)) as u64;
        let accel_mag_mg = (isqrt(sq) * 1000 / 16384) as u32;

        // Telemetry packet: marker + sequence + accel magnitude (milli-g).
        let mut pkt = [0u8; 9];
        pkt[0] = 0x5A;
        pkt[1..5].copy_from_slice(&seq.to_le_bytes());
        pkt[5..9].copy_from_slice(&accel_mag_mg.to_le_bytes());
        if Radio::send(&pkt) {
            tx_sent = tx_sent.wrapping_add(1);
        }

        let mut r = RadioReport {
            magic: MAGIC,
            version: 1,
            role: 3,
            tx_sent,
            rx_received: 0,
            last_seq: accel_mag_mg,
            all_pass: u32::from(tx_sent >= 10 && (800..1200).contains(&accel_mag_mg)),
            checksum: 0,
        };
        r.checksum = checksum(&r);
        unsafe {
            NOBRO_RADIO_REPORT = r;
        }

        for _ in 0..200_000u32 {
            cortex_m::asm::nop();
        }
    }
}
