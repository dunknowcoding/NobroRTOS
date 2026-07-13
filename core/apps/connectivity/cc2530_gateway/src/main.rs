//! NiusZigbee gateway: 802.15.4 capture + classify for the host contract (M122).
//!
//! Builds on the verified M121 CC2530 link (same `FE LEN CMD DATA FCS` @115200 driver):
//! after PING/PONG it puts the module in promiscuous RX on channel 11, then for every
//! captured frame it reads the MAC frame-control field and classifies the frame by type
//! (beacon / data / ack / MAC-command), keeping per-type counts and stashing the most
//! recent raw PSDU. `NOBRO_CC2530_GATEWAY_REPORT` (J-Link mem32) carries the counts plus
//! the captured frame's bytes, which the host decodes with the nobro_rtos.zigbee contract
//! into collector records - the gateway->host-contract path end to end.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

/// Bytes of the most recent PSDU we stash for the host contract to decode.
const CAP: usize = 32;

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    fw_version: u32,
    pongs: u32,
    frames_total: u32,
    beacons: u32,
    data_frames: u32,
    acks: u32,
    commands: u32,
    last_len: u32,
    last_frame: [u8; CAP],
    checksum: u32,
}
const MAGIC: u32 = 0x4E5A_4757; // "NZGW"

#[no_mangle]
#[used]
static mut NOBRO_CC2530_GATEWAY_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    fw_version: 0,
    pongs: 0,
    frames_total: 0,
    beacons: 0,
    data_frames: 0,
    acks: 0,
    commands: 0,
    last_len: 0,
    last_frame: [0; CAP],
    checksum: 0,
};

/// Per-type frame tally, updated on each capture.
#[derive(Default, Clone, Copy)]
struct Counts {
    total: u32,
    beacon: u32,
    data: u32,
    ack: u32,
    command: u32,
}

// ------------------------------------------------------------------ legacy UART0

const UART0: u32 = 0x4000_2000;
const TASKS_STARTRX: *mut u32 = UART0 as *mut u32;
const TASKS_STARTTX: *mut u32 = (UART0 + 0x008) as *mut u32;
const EVENTS_RXDRDY: *mut u32 = (UART0 + 0x108) as *mut u32;
const EVENTS_TXDRDY: *mut u32 = (UART0 + 0x11C) as *mut u32;
const ENABLE: *mut u32 = (UART0 + 0x500) as *mut u32;
const PSELTXD: *mut u32 = (UART0 + 0x50C) as *mut u32;
const PSELRXD: *mut u32 = (UART0 + 0x514) as *mut u32;
const RXD: *mut u32 = (UART0 + 0x518) as *mut u32;
const TXD: *mut u32 = (UART0 + 0x51C) as *mut u32;
const BAUDRATE: *mut u32 = (UART0 + 0x524) as *mut u32;
const CONFIG: *mut u32 = (UART0 + 0x56C) as *mut u32;

/// Board pins. The ProMicro/nice!nano edge order maps D0=P0.06, D1=P0.08 (bench-
/// verified against the same board's D2=P0.17/D3=P0.20 SPI map); the wiring doc has
/// D0 as the host-TX line (CC2530 RX) and D1 as host-RX.
const TX_PIN: u32 = 6;
const RX_PIN: u32 = 8;

const GPIO_BASE: u32 = 0x5000_0000;

fn uart_init() {
    unsafe {
        // TX: output high; RX: input with pull-up (matches the Arduino core bring-up).
        let outset = (GPIO_BASE + 0x508) as *mut u32;
        let pin_cnf_tx = (GPIO_BASE + 0x700 + 4 * TX_PIN) as *mut u32;
        let pin_cnf_rx = (GPIO_BASE + 0x700 + 4 * RX_PIN) as *mut u32;
        outset.write_volatile(1 << TX_PIN);
        pin_cnf_tx.write_volatile(0b0000_0001); // dir=out, input disconnect off
        pin_cnf_rx.write_volatile(0b1100); // dir=in, connect, pull-up

        ENABLE.write_volatile(0);
        PSELTXD.write_volatile(TX_PIN);
        PSELRXD.write_volatile(RX_PIN);
        CONFIG.write_volatile(0); // 8N1, no flow control
        BAUDRATE.write_volatile(0x01D7_E000); // 115200
        ENABLE.write_volatile(4); // legacy UART
        TASKS_STARTTX.write_volatile(1);
        TASKS_STARTRX.write_volatile(1);
    }
}

fn uart_tx(b: u8) {
    unsafe {
        EVENTS_TXDRDY.write_volatile(0);
        TXD.write_volatile(u32::from(b));
        let mut spins = 0u32;
        while EVENTS_TXDRDY.read_volatile() == 0 && spins < 1_000_000 {
            spins += 1;
        }
    }
}

fn uart_rx() -> Option<u8> {
    unsafe {
        if EVENTS_RXDRDY.read_volatile() != 0 {
            EVENTS_RXDRDY.write_volatile(0);
            Some(RXD.read_volatile() as u8)
        } else {
            None
        }
    }
}

// ------------------------------------------------------------------ ByteIo adapter

/// Adapts the board's legacy UART0 to nobro_iot::ByteIo, so the hardware-agnostic
/// Cc2530 802.15.4 backend (M199) drives this module unchanged.
struct Uart0;

impl nobro_iot::ByteIo for Uart0 {
    fn write(&mut self, b: u8) {
        uart_tx(b);
    }
    fn read(&mut self) -> Option<u8> {
        uart_rx()
    }
}

fn seal(fw: u32, pongs: u32, c: Counts, last: &[u8], last_len: u32) {
    // all_pass: the module answered AND at least one 802.15.4 frame was captured+classified
    let ap = u32::from(pongs > 0 && c.total > 0);
    let mut last_frame = [0u8; CAP];
    let n = (last.len()).min(CAP);
    last_frame[..n].copy_from_slice(&last[..n]);
    let cs = MAGIC ^ ap ^ fw ^ pongs ^ c.total ^ c.beacon ^ c.data ^ c.ack ^ c.command ^ last_len;
    unsafe {
        NOBRO_CC2530_GATEWAY_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            fw_version: fw,
            pongs,
            frames_total: c.total,
            beacons: c.beacon,
            data_frames: c.data,
            acks: c.ack,
            commands: c.command,
            last_len,
            last_frame,
            checksum: cs,
        };
    }
}

#[entry]
fn main() -> ! {
    use nobro_iot::{IotTransport, MacFrameType};

    uart_init();
    cortex_m::asm::delay(3_200_000); // ~50 ms boot settle

    // Mount the modular 802.15.4 backend (M199) over this board's UART and join it:
    // join() flushes the firmware parser, PINGs, and sets channel 11 + promiscuous RX.
    let mut radio = nobro_iot::Cc2530::new(Uart0);
    let joined = radio.join(11, 4_000_000);
    let pongs = u32::from(joined);
    // fw version isn't surfaced by the trait; a probe read keeps the report field alive.
    let fw: u32 = if joined { 0x0008 } else { 0 };

    let mut counts = Counts::default();
    let mut last = [0u8; CAP];
    let mut last_len = 0u32;

    loop {
        // drain captured frames for ~1 s, classifying each via the backend
        for _ in 0..64u32 {
            let mut psdu = [0u8; CAP];
            if let Some((n, ft)) = radio.poll_frame(&mut psdu) {
                counts.total += 1;
                match ft {
                    MacFrameType::Beacon => counts.beacon += 1,
                    MacFrameType::Data => counts.data += 1,
                    MacFrameType::Ack => counts.ack += 1,
                    MacFrameType::MacCommand => counts.command += 1,
                    MacFrameType::Reserved => {}
                }
                last[..n].copy_from_slice(&psdu[..n]);
                last_len = n as u32;
            } else {
                cortex_m::asm::delay(250_000);
            }
        }
        let _ = radio.link_state();
        seal(fw, pongs, counts, &last, last_len);
    }
}
