//! NobroRTOS portable core on the RA4M1 / Arduino UNO R4 WiFi (M86).
//!
//! Runs the shared cross-MCU conformance suite with **all drivers our own**, written
//! from the RA4M1 hardware manual: PRCR-unlocked clock setup (MOCO 8 MHz, all dividers
//! /1), PFS pin muxing, an SCI9 UART on P109/P110 - the pins wired to the board's
//! ESP32-S3 USB bridge, so the report arrives on the板's own USB port at 9600 with no
//! extra hardware - and a PORT1 LED heartbeat (D13/P102). Report line:
//!   `NOBRO-RA4M1 arch=thumbv7em subsystems=7 all_pass=1`
//!
//! The image links at 0x4000 behind the stock DFU bootloader; SCB->VTOR is pointed at
//! our vector table first thing (the hard-won nRF lesson - the bootloader leaves VTOR
//! at its own table). Double-tap RESET always returns the board to DFU.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

use nobro_conformance::run_all;

// ---------------------------------------------------------------- system (own driver)

const VTOR: *mut u32 = 0xE000_ED08 as *mut u32;
const PRCR: *mut u16 = 0x4001_E3FE as *mut u16;
const SCKDIVCR: *mut u32 = 0x4001_E020 as *mut u32;
const MSTPCRB: *mut u32 = 0x4004_7000 as *mut u32;

fn system_init() {
    unsafe {
        VTOR.write_volatile(0x4000);
        PRCR.write_volatile(0xA503); // unlock clock + low-power registers
        SCKDIVCR.write_volatile(0); // every clock = MOCO 8 MHz / 1
        PRCR.write_volatile(0xA500); // re-lock
        // wake SCI2 (bit 29) + SCI9 (bit 22) from module-stop
        MSTPCRB.write_volatile(MSTPCRB.read_volatile() & !((1 << 29) | (1 << 22)));
    }
}

// ---------------------------------------------------------------- pins (own driver)

const PWPR: *mut u8 = 0x4004_0D03 as *mut u8;
/// PmnPFS for P301 (D0/RXD2) and P302 (D1/TXD2): base + port*0x40 + pin*4.
const P301_PFS: *mut u32 = 0x4004_08C4 as *mut u32;
const P302_PFS: *mut u32 = 0x4004_08C8 as *mut u32;
/// P109/P110: the SCI9 pair wired to the ESP32-S3 USB bridge (odd SCI group, PSEL 5).
const P109_PFS: *mut u32 = 0x4004_0864 as *mut u32;
const P110_PFS: *mut u32 = 0x4004_0868 as *mut u32;
/// PORT1 PCNTR1: low half = direction, high half = output data (LED = P102).
const PORT1_PCNTR1: *mut u32 = 0x4004_0020 as *mut u32;
const LED_BIT: u32 = 1 << 2;

fn pins_init() {
    unsafe {
        PWPR.write_volatile(0x00); // clear B0WI
        PWPR.write_volatile(0x40); // set PFSWE: PFS writes enabled
        let sci2 = (0x04 << 24) | (1 << 16); // PSEL = even-SCI group, PMR = peripheral
        P301_PFS.write_volatile(sci2);
        P302_PFS.write_volatile(sci2);
        let sci9 = (0x05 << 24) | (1 << 16); // PSEL = odd-SCI group (SCI9)
        P109_PFS.write_volatile(sci9);
        P110_PFS.write_volatile(sci9);
        PWPR.write_volatile(0x00);
        PWPR.write_volatile(0x80); // re-lock (B0WI)
        // LED output, initially off
        PORT1_PCNTR1.write_volatile(LED_BIT);
    }
}

fn led_toggle() {
    unsafe {
        PORT1_PCNTR1.write_volatile(PORT1_PCNTR1.read_volatile() ^ (LED_BIT << 16));
    }
}

// ---------------------------------------------------------------- SCI2 UART (own driver)

/// SCIn register block (0x20 apart): SMR+0, BRR+1, SCR+2, TDR+3, SSR+4, SCMR+6, SEMR+7.
struct Sci(u32);

impl Sci {
    /// 9600 8N1 from MOCO 8 MHz: BRR = 8e6 / (32 * 9600) - 1 = 25 (0.16% error).
    fn init(&self) {
        unsafe {
            let b = self.0;
            ((b + 2) as *mut u8).write_volatile(0); // SCR: all off while configuring
            (b as *mut u8).write_volatile(0); // SMR: async 8N1, PCLK/1
            ((b + 6) as *mut u8).write_volatile(0xF2); // SCMR: not smart-card mode
            ((b + 7) as *mut u8).write_volatile(0); // SEMR
            ((b + 1) as *mut u8).write_volatile(25); // BRR
            for _ in 0..2_000u32 {
                cortex_m::asm::nop(); // one-bit settle before enabling
            }
            ((b + 2) as *mut u8).write_volatile(0x20); // SCR: TE
        }
    }
    fn tx(&self, byte: u8) {
        unsafe {
            let b = self.0;
            while ((b + 4) as *const u8).read_volatile() & 0x80 == 0 {} // SSR.TDRE
            ((b + 3) as *mut u8).write_volatile(byte);
        }
    }
    fn print(&self, s: &str) {
        for &byte in s.as_bytes() {
            self.tx(byte);
        }
    }
}

/// SCI2 = the D0/D1 header pins; SCI9 = the ESP32-S3 USB bridge (this is COM-visible).
const SCI2: Sci = Sci(0x4007_0040);
const SCI9: Sci = Sci(0x4007_0120);

/// Drive the three loopback output pins (D6=P111, D5=P107, D3=P105) to a fixed
/// pattern HIGH/LOW/HIGH. The user wired these to A4/A0/A5, so the pattern is
/// externally checkable once a readout channel exists; here it also proves our
/// PORT1 GPIO writes take effect (part of the own-driver deliverable).
fn drive_loopback() {
    unsafe {
        // PORT1 PCNTR1: low half PDR (1=output), high half PODR (output level).
        // outputs: P111, P107, P105 ; levels: P111=1, P107=0, P105=1
        let pdr = (1 << 11) | (1 << 7) | (1 << 5) | LED_BIT;
        let podr = ((1u32 << 11) | (1 << 5)) << 16; // P111 & P105 high, P107 low
        PORT1_PCNTR1.write_volatile(pdr | podr);
    }
}

#[entry]
fn main() -> ! {
    system_init();
    pins_init();
    SCI2.init();
    SCI9.init();
    drive_loopback();

    let results = run_all(); // the shared cross-MCU conformance suite (M92)
    let all = results.iter().all(|&r| r);
    let line = if all {
        "NOBRO-RA4M1 arch=thumbv7em subsystems=7 all_pass=1\r\n"
    } else {
        "NOBRO-RA4M1 arch=thumbv7em subsystems=7 all_pass=0\r\n"
    };

    loop {
        SCI9.print(line); // out the ESP32-S3 bridge candidate
        SCI2.print(line); // and the D0/D1 header
        led_toggle(); // ~1 Hz heartbeat on the built-in LED = eyes-on evidence
        cortex_m::asm::delay(8_000_000); // ~1 s at 8 MHz
    }
}
