//! Tri-radio node: one board drives three radios concurrently.
//!
//! Each 1-second cycle the node (a) broadcasts a BLE advertisement carrying its beat
//! (any phone/PC scanner receives it), (b) sends a proprietary-1Mbps packet over the
//! same nRF RADIO peripheral, and (c) services its CC2530 802.15.4 co-processor over
//! UART (PING/PONG + promiscuous frame counting). BLE and proprietary time-multiplex
//! the RADIO - each reconfigures it fully, restoring DATAWHITEIV when leaving BLE
//! (advertising seeds it with the channel index, which would corrupt proprietary
//! whitening). `NOBRO_TRI_RADIO_REPORT` carries all three counters; all_pass needs
//! every radio to have delivered at least once.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use nobro_hal::RadioSession;
use nobro_wireless::BleAdvBuilder;
use panic_halt as _;

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    ble_advs: u32,
    radio_tx: u32,
    cc_pongs: u32,
    cc_frames: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E54_5249; // "NTRI"

#[no_mangle]
#[used]
static mut NOBRO_TRI_RADIO_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    ble_advs: 0,
    radio_tx: 0,
    cc_pongs: 0,
    cc_frames: 0,
    checksum: 0,
};

// ---------------------------------------------------------------- RADIO (BLE mode)

const RADIO_BASE: usize = 0x4000_1000;
const TASKS_TXEN: *mut u32 = RADIO_BASE as *mut u32;
const EVENTS_DISABLED: *mut u32 = (RADIO_BASE + 0x110) as *mut u32;
const SHORTS: *mut u32 = (RADIO_BASE + 0x200) as *mut u32;
const PACKETPTR: *mut u32 = (RADIO_BASE + 0x504) as *mut u32;
const FREQUENCY: *mut u32 = (RADIO_BASE + 0x508) as *mut u32;
const TXPOWER: *mut u32 = (RADIO_BASE + 0x50C) as *mut u32;
const MODE: *mut u32 = (RADIO_BASE + 0x510) as *mut u32;
const PCNF0: *mut u32 = (RADIO_BASE + 0x514) as *mut u32;
const PCNF1: *mut u32 = (RADIO_BASE + 0x518) as *mut u32;
const BASE0: *mut u32 = (RADIO_BASE + 0x51C) as *mut u32;
const PREFIX0: *mut u32 = (RADIO_BASE + 0x524) as *mut u32;
const TXADDRESS: *mut u32 = (RADIO_BASE + 0x52C) as *mut u32;
const CRCCNF: *mut u32 = (RADIO_BASE + 0x534) as *mut u32;
const CRCPOLY: *mut u32 = (RADIO_BASE + 0x538) as *mut u32;
const CRCINIT: *mut u32 = (RADIO_BASE + 0x53C) as *mut u32;
const DATAWHITEIV: *mut u32 = (RADIO_BASE + 0x554) as *mut u32;

const ADV_CHANNELS: [(u32, u32); 3] = [(2, 37), (26, 38), (80, 39)];

fn ble_config() {
    unsafe {
        MODE.write_volatile(3); // Ble1Mbit
        TXPOWER.write_volatile(0);
        PCNF0.write_volatile(8 | (1 << 8));
        PCNF1.write_volatile(39 | (3 << 16) | (1 << 25));
        BASE0.write_volatile(0x89BE_D600);
        PREFIX0.write_volatile(0x8E);
        TXADDRESS.write_volatile(0);
        CRCCNF.write_volatile(3 | (1 << 8));
        CRCPOLY.write_volatile(0x0000_065B);
        CRCINIT.write_volatile(0x0055_5555);
        SHORTS.write_volatile((1 << 0) | (1 << 1)); // READY->START, END->DISABLE
    }
}

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

/// Hand the RADIO back to the proprietary link: full reconfig + reset whitening seed.
fn proprietary_config(radio: &RadioSession) {
    unsafe {
        radio
            .reconfigure()
            .unwrap_or_else(|_| defmt::panic!("radio session"));
        DATAWHITEIV.write_volatile(0x40); // hardware reset value; BLE left a channel iv
        SHORTS.write_volatile(0); // Radio::send drives tasks explicitly
    }
}

// ---------------------------------------------------------------- CC2530 UART link

const UART0: u32 = 0x4000_2000;
const U_STARTRX: *mut u32 = UART0 as *mut u32;
const U_STARTTX: *mut u32 = (UART0 + 0x008) as *mut u32;
const U_RXDRDY: *mut u32 = (UART0 + 0x108) as *mut u32;
const U_TXDRDY: *mut u32 = (UART0 + 0x11C) as *mut u32;
const U_ENABLE: *mut u32 = (UART0 + 0x500) as *mut u32;
const U_PSELTXD: *mut u32 = (UART0 + 0x50C) as *mut u32;
const U_PSELRXD: *mut u32 = (UART0 + 0x514) as *mut u32;
const U_RXD: *mut u32 = (UART0 + 0x518) as *mut u32;
const U_TXD: *mut u32 = (UART0 + 0x51C) as *mut u32;
const U_BAUD: *mut u32 = (UART0 + 0x524) as *mut u32;
const U_CONFIG: *mut u32 = (UART0 + 0x56C) as *mut u32;

/// D0=P0.06 (host TX -> CC2530 RX), D1=P0.08 (host RX).
const TX_PIN: u32 = 6;
const RX_PIN: u32 = 8;
const GPIO_BASE: u32 = 0x5000_0000;

fn uart_init() {
    unsafe {
        ((GPIO_BASE + 0x508) as *mut u32).write_volatile(1 << TX_PIN);
        ((GPIO_BASE + 0x700 + 4 * TX_PIN) as *mut u32).write_volatile(0b0000_0001);
        ((GPIO_BASE + 0x700 + 4 * RX_PIN) as *mut u32).write_volatile(0b1100);
        U_ENABLE.write_volatile(0);
        U_PSELTXD.write_volatile(TX_PIN);
        U_PSELRXD.write_volatile(RX_PIN);
        U_CONFIG.write_volatile(0);
        U_BAUD.write_volatile(0x01D7_E000); // 115200
        U_ENABLE.write_volatile(4);
        U_STARTTX.write_volatile(1);
        U_STARTRX.write_volatile(1);
    }
}

fn uart_tx(b: u8) {
    unsafe {
        U_TXDRDY.write_volatile(0);
        U_TXD.write_volatile(u32::from(b));
        let mut spins = 0u32;
        while U_TXDRDY.read_volatile() == 0 && spins < 1_000_000 {
            spins += 1;
        }
    }
}

fn uart_rx() -> Option<u8> {
    unsafe {
        if U_RXDRDY.read_volatile() != 0 {
            U_RXDRDY.write_volatile(0);
            Some(U_RXD.read_volatile() as u8)
        } else {
            None
        }
    }
}

fn cc_send(cmd: u8, data: &[u8]) {
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

fn start_hfxo() {
    unsafe {
        core::ptr::write_volatile(0x4000_0000 as *mut u32, 1);
        while core::ptr::read_volatile(0x4000_0100 as *const u32) == 0 {}
    }
}

#[entry]
fn main() -> ! {
    start_hfxo();
    let radio =
        unsafe { RadioSession::acquire(5) }.unwrap_or_else(|_| defmt::panic!("radio session"));
    uart_init();
    cortex_m::asm::delay(3_200_000);
    for _ in 0..140 {
        uart_tx(0x00); // flush the CC2530 firmware's frame parser
    }

    let adv_addr = [0x4E, 0x42, 0x52, 0x4F, 0x02, 0xC3];
    let builder = BleAdvBuilder {
        adv_addr: &adv_addr,
        name: b"NOBRO",
        company_id: 0xFFFF,
    };

    let mut dec = Decoder::new();
    let mut beat: u32 = 0;
    let mut ble_advs = 0u32;
    let mut radio_tx = 0u32;
    let mut cc_pongs = 0u32;
    let mut cc_frames = 0u32;
    let mut cc_configured = false;

    loop {
        beat = beat.wrapping_add(1);

        // (a) BLE advertisement with the beat in manufacturer data
        ble_config();
        let mut payload = [0u8; 6];
        payload[0..4].copy_from_slice(&beat.to_le_bytes());
        payload[4] = 3; // radios on this node
        let mut pdu = [0u8; 39];
        if let Some(len) = builder.build(&payload, &mut pdu) {
            for (freq, iv) in ADV_CHANNELS {
                ble_send(&pdu[..len], freq, iv);
            }
            ble_advs += 1;
        }

        // (b) proprietary packet over the same RADIO, reconfigured
        proprietary_config(&radio);
        let mut pkt = [0u8; 5];
        pkt[0] = 0xA5;
        pkt[1..5].copy_from_slice(&beat.to_le_bytes());
        if radio.send(&pkt).is_ok() {
            radio_tx += 1;
        }

        // (c) CC2530: ping once a cycle, drain for ~1 s
        cc_send(0x01, &[]);
        for _ in 0..2_000_000u32 {
            if let Some(b) = uart_rx() {
                if let Some(cmd) = dec.feed(b) {
                    match cmd {
                        0x81 => {
                            cc_pongs += 1;
                            if !cc_configured {
                                cc_configured = true;
                                cc_send(0x02, &[11]);
                                cc_send(0x04, &[0]);
                            }
                        }
                        0x84 => cc_frames += 1,
                        _ => {}
                    }
                }
            } else {
                cortex_m::asm::nop();
            }
        }

        let ap = u32::from(ble_advs > 0 && radio_tx > 0 && cc_pongs > 0);
        let cs = MAGIC ^ 1 ^ 1 ^ ap ^ ble_advs ^ radio_tx ^ cc_pongs ^ cc_frames;
        unsafe {
            NOBRO_TRI_RADIO_REPORT = Report {
                magic: MAGIC,
                version: 1,
                completed: 1,
                all_pass: ap,
                ble_advs,
                radio_tx,
                cc_pongs,
                cc_frames,
                checksum: cs,
            };
        }
    }
}
