//! CC2530 802.15.4 co-processor raw link (M121).
//!
//! Speaks the NiusZigbee SDCC transceiver firmware's UART protocol from NobroRTOS:
//! `FE LEN CMD DATA.. FCS` at 115200 (LEN counts CMD+DATA, FCS = XOR of LEN..DATA).
//! The app PINGs the module (PONG carries the firmware version), switches it to
//! promiscuous RX on channel 11, and counts real 802.15.4 frames heard off the air.
//! `NOBRO_CC2530_REPORT` (J-Link mem32) carries pongs/version/rx_frames; all_pass
//! requires a valid PONG - frames heard depends on what else is transmitting.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    fw_version: u32,
    pings: u32,
    pongs: u32,
    rx_frames: u32,
    rx_bytes: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E43_3235; // "NC25"

#[no_mangle]
#[used]
static mut NOBRO_CC2530_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    fw_version: 0,
    pings: 0,
    pongs: 0,
    rx_frames: 0,
    rx_bytes: 0,
    checksum: 0,
};

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

// ------------------------------------------------------------------ protocol

fn send_frame(cmd: u8, data: &[u8]) {
    let len = (data.len() + 1) as u8;
    let mut fcs = len ^ cmd;
    uart_tx(0xFE);
    uart_tx(len);
    uart_tx(cmd);
    for &b in data {
        uart_tx(b);
        fcs ^= b;
    }
    uart_tx(fcs);
}

/// Streaming decoder mirroring the C++ host driver's state machine.
struct Decoder {
    state: u8,
    len: u8,
    idx: u8,
    fcs: u8,
    buf: [u8; 160],
}

impl Decoder {
    const fn new() -> Self {
        Decoder {
            state: 0,
            len: 0,
            idx: 0,
            fcs: 0,
            buf: [0; 160],
        }
    }
    /// Feed one byte; returns Some(cmd) when a checksum-valid frame completes.
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
                    return Some(self.buf[0]);
                }
            }
        }
        None
    }
}

fn seal(fw: u32, pings: u32, pongs: u32, frames: u32, rx_bytes: u32) {
    let ap = u32::from(pongs > 0);
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ fw ^ pings ^ pongs ^ frames ^ rx_bytes;
    unsafe {
        NOBRO_CC2530_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            fw_version: fw,
            pings,
            pongs,
            rx_frames: frames,
            rx_bytes,
            checksum: cs,
        };
    }
}

#[entry]
fn main() -> ! {
    uart_init();

    // The CC2530 keeps running while this host reboots, so its frame parser may be
    // stuck mid-packet. Zero-fill flushes any partial frame (mirrors the C++ driver).
    cortex_m::asm::delay(3_200_000); // ~50 ms boot settle
    for _ in 0..140 {
        uart_tx(0x00);
    }
    cortex_m::asm::delay(320_000);

    let mut dec = Decoder::new();
    let mut pings = 0u32;
    let mut pongs = 0u32;
    let mut frames = 0u32;
    let mut fw = 0u32;
    let mut rx_bytes = 0u32;
    let mut configured = false;

    loop {
        pings += 1;
        send_frame(0x01, &[]); // PING

        // ~1 s window: drain RX, count frames, catch the PONG
        for _ in 0..2_000_000u32 {
            if let Some(b) = uart_rx() {
                rx_bytes += 1;
                if let Some(cmd) = dec.feed(b) {
                    match cmd {
                        0x81 => {
                            // PONG [ver_hi ver_lo]
                            fw = (u32::from(dec.buf[1]) << 8) | u32::from(dec.buf[2]);
                            pongs += 1;
                            if !configured {
                                configured = true;
                                send_frame(0x02, &[11]); // SET_CHANNEL 11
                                send_frame(0x04, &[0]); // SET_PROMISC (filter off)
                            }
                        }
                        0x84 => frames += 1, // RX_FRAME
                        _ => {}
                    }
                }
            } else {
                cortex_m::asm::nop();
            }
        }
        seal(fw, pings, pongs, frames, rx_bytes);
    }
}
