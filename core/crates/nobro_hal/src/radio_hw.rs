//! nRF52 2.4 GHz RADIO in Nordic 1 Mbps proprietary mode - a minimal board-to-board
//! packet primitive for multi-board data collection (no SoftDevice, no CC2530). HFXO
//! must be running before use (the radio derives its clock from it). One logical
//! address, 2-byte CRC, length-prefixed payload. Half-duplex: send() or recv(), not
//! both at once.

const RADIO_BASE: u32 = 0x4000_1000;

const TASKS_TXEN: u32 = 0x000;
const TASKS_RXEN: u32 = 0x004;
const TASKS_DISABLE: u32 = 0x010;
const EVENTS_END: u32 = 0x10C;
const EVENTS_DISABLED: u32 = 0x110;
const CRCSTATUS: u32 = 0x400;
const SHORTS: u32 = 0x200;
const PACKETPTR: u32 = 0x504;
const FREQUENCY: u32 = 0x508;
const TXPOWER: u32 = 0x50C;
const MODE: u32 = 0x510;
const PCNF0: u32 = 0x514;
const PCNF1: u32 = 0x518;
const BASE0: u32 = 0x51C;
const PREFIX0: u32 = 0x524;
const TXADDRESS: u32 = 0x52C;
const RXADDRESSES: u32 = 0x530;
const CRCCNF: u32 = 0x534;
const CRCPOLY: u32 = 0x538;
const CRCINIT: u32 = 0x53C;

const MODE_NRF_1MBIT: u32 = 0;
const ADDR_BASE0: u32 = 0xE7E7_E7E7;
const ADDR_PREFIX0: u32 = 0x0000_00C2;
const CHANNEL_FREQ: u32 = 40; // 2440 MHz - clear of common BLE advertising channels
const SHORT_READY_START: u32 = 1 << 0;
const SHORT_END_DISABLE: u32 = 1 << 1;

/// Max application payload per packet.
pub const RADIO_MAX_PAYLOAD: usize = 32;

fn reg(off: u32) -> *mut u32 {
    (RADIO_BASE + off) as *mut u32
}

/// nRF 2.4 GHz RADIO, proprietary 1 Mbps. Caller must have HFXO running.
pub struct Radio;

impl Radio {
    /// Configure the radio: 1 Mbps, channel 40, fixed address, 2-byte CRC, 1-byte
    /// length field. Idempotent.
    ///
    /// # Safety
    /// HFXO must be started; the radio peripheral must not be owned by a SoftDevice.
    pub unsafe fn init() {
        *reg(MODE) = MODE_NRF_1MBIT;
        *reg(FREQUENCY) = CHANNEL_FREQ;
        *reg(TXPOWER) = 0; // 0 dBm
                           // PCNF0: LFLEN = 8 (1-byte LENGTH field), S0LEN = 0, S1LEN = 0.
        *reg(PCNF0) = 8;
        // PCNF1: MAXLEN, STATLEN=0, BALEN=4 (4-byte base addr), little-endian, whiten.
        *reg(PCNF1) = (RADIO_MAX_PAYLOAD as u32) | (4 << 16) | (1 << 25);
        *reg(BASE0) = ADDR_BASE0;
        *reg(PREFIX0) = ADDR_PREFIX0;
        *reg(TXADDRESS) = 0;
        *reg(RXADDRESSES) = 1; // listen on logical address 0
        *reg(CRCCNF) = 2; // 2-byte CRC
        *reg(CRCPOLY) = 0x0001_1021;
        *reg(CRCINIT) = 0x0000_FFFF;
    }

    /// Send a payload (truncated to `RADIO_MAX_PAYLOAD`). Returns true if the radio
    /// completed the transmission (END/DISABLED), false on timeout.
    pub fn send(payload: &[u8]) -> bool {
        let n = payload.len().min(RADIO_MAX_PAYLOAD);
        let mut pkt = [0u8; RADIO_MAX_PAYLOAD + 1];
        pkt[0] = n as u8;
        pkt[1..1 + n].copy_from_slice(&payload[..n]);
        unsafe {
            *reg(EVENTS_END) = 0;
            *reg(EVENTS_DISABLED) = 0;
            *reg(PACKETPTR) = pkt.as_ptr() as u32;
            *reg(SHORTS) = SHORT_READY_START | SHORT_END_DISABLE;
            cortex_m::asm::dsb();
            *reg(TASKS_TXEN) = 1;
            let mut ok = false;
            for _ in 0..2_000_000u32 {
                if *reg(EVENTS_DISABLED) != 0 {
                    ok = true;
                    break;
                }
                cortex_m::asm::nop();
            }
            *reg(SHORTS) = 0;
            ok
        }
    }

    /// Receive one CRC-valid packet into `buf` within `timeout_spins`. Returns the
    /// payload length on success, or None on timeout / CRC error.
    pub fn recv(buf: &mut [u8], timeout_spins: u32) -> Option<usize> {
        let mut pkt = [0u8; RADIO_MAX_PAYLOAD + 1];
        unsafe {
            *reg(EVENTS_END) = 0;
            *reg(EVENTS_DISABLED) = 0;
            *reg(PACKETPTR) = pkt.as_mut_ptr() as u32;
            *reg(SHORTS) = SHORT_READY_START;
            cortex_m::asm::dsb();
            *reg(TASKS_RXEN) = 1;
            let mut got = false;
            for _ in 0..timeout_spins {
                if *reg(EVENTS_END) != 0 {
                    got = true;
                    break;
                }
                cortex_m::asm::nop();
            }
            *reg(SHORTS) = 0;
            *reg(EVENTS_DISABLED) = 0;
            *reg(TASKS_DISABLE) = 1;
            for _ in 0..200_000u32 {
                if *reg(EVENTS_DISABLED) != 0 {
                    break;
                }
                cortex_m::asm::nop();
            }
            if got && *reg(CRCSTATUS) == 1 {
                let n = (pkt[0] as usize).min(RADIO_MAX_PAYLOAD).min(buf.len());
                buf[..n].copy_from_slice(&pkt[1..1 + n]);
                Some(n)
            } else {
                None
            }
        }
    }
}
