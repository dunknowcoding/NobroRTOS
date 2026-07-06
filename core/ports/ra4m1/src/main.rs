//! NobroRTOS portable core on the RA4M1 / Arduino UNO R4 WiFi (M86).
//!
//! Runs the shared cross-MCU conformance suite with **all drivers our own**, written
//! from the RA4M1 hardware manual: PRCR-unlocked clock setup (HOCO 48 MHz with the
//! official peripheral dividers), PFS pin muxing, native USB CDC on the board's
//! own connector, SCI1 on the board-internal WiFi-module UART, plus a header UART
//! and a PORT1 LED heartbeat (D13/P102). Report line:
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
const VBTBKR0: *mut u32 = 0x4001_E500 as *mut u32;
const VBTBKR1: *mut u8 = 0x4001_E501 as *mut u8;
const SCKDIVCR: *mut u32 = 0x4001_E020 as *mut u32;
const MSTPCRB: *mut u32 = 0x4004_7000 as *mut u32;
const USB_SYSCFG: *mut u16 = 0x4009_0000 as *mut u16;
const USB_DPRPU: u16 = 1 << 4;
const BOOT_DOUBLE_TAP_MAGIC: u32 = 0x0773_8135;

fn system_init() {
    unsafe {
        VTOR.write_volatile(0x4000);
        PRCR.write_volatile(0xA503); // unlock clock + low-power registers
                                     // Match the official board clock plan inherited from the bootloader: ICLK,
                                     // PCLKA/C/D = 48 MHz; PCLKB and FCLK = 24 MHz. USB uses HOCO directly.
        SCKDIVCR.write_volatile(0x1001_0100);
        PRCR.write_volatile(0xA500); // re-lock
                                     // wake SCI1 bridge UART, SCI2 header UART, and SCI9 user UART from module-stop
        MSTPCRB.write_volatile(MSTPCRB.read_volatile() & !((1 << 30) | (1 << 29) | (1 << 22)));
    }
}

// ---------------------------------------------------------------- pins (own driver)

const PWPR: *mut u8 = 0x4004_0D03 as *mut u8;
/// PmnPFS for P301 (D0/RXD2) and P302 (D1/TXD2): base + port*0x40 + pin*4.
const P301_PFS: *mut u32 = 0x4004_08C4 as *mut u32;
const P302_PFS: *mut u32 = 0x4004_08C8 as *mut u32;
/// P109/P110: user-facing SCI9 pair (odd SCI group, PSEL 5).
const P109_PFS: *mut u32 = 0x4004_0864 as *mut u32;
const P110_PFS: *mut u32 = 0x4004_0868 as *mut u32;
/// P501/P502: SCI1 pair wired to the board-internal WiFi-module UART.
const P501_PFS: *mut u32 = 0x4004_0944 as *mut u32;
const P502_PFS: *mut u32 = 0x4004_0948 as *mut u32;
/// Native USBFS pins used by the UNO R4 WiFi board wiring.
const P407_PFS: *mut u32 = 0x4004_091C as *mut u32;
const P500_PFS: *mut u32 = 0x4004_0940 as *mut u32;
const P914_PFS: *mut u32 = 0x4004_0A78 as *mut u32;
const P915_PFS: *mut u32 = 0x4004_0A7C as *mut u32;
/// P408 controls the board's USB data switch: high routes the connector to RA4M1.
const P408_PFS: *mut u32 = 0x4004_0920 as *mut u32;
/// PORT1 PCNTR1: low half = direction, high half = output data (LED = P102).
const PORT1_PCNTR1: *mut u32 = 0x4004_0020 as *mut u32;
const PORT4_PCNTR1: *mut u32 = 0x4004_0080 as *mut u32;
const LED_BIT: u32 = 1 << 2;
const USB_MUX_BIT: u32 = 1 << 8;
const NATIVE_USB_FALLBACK_TICKS: u32 = 180;

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
        let sci1 = (0x05 << 24) | (1 << 16); // PSEL = odd-SCI group (SCI1)
        P501_PFS.write_volatile(sci1);
        P502_PFS.write_volatile(sci1);
        let usb_fs = (0x13 << 24) | (1 << 16); // PSEL = USB_FS, PMR = peripheral
        P407_PFS.write_volatile(usb_fs | 0x400); // drive strength matches Arduino's generated pin data
        P500_PFS.write_volatile(usb_fs);
        P914_PFS.write_volatile(usb_fs);
        P915_PFS.write_volatile(usb_fs);
        P408_PFS.write_volatile(0); // GPIO output, not an alternate peripheral
        PWPR.write_volatile(0x00);
        PWPR.write_volatile(0x80); // re-lock (B0WI)
                                   // LED output, initially off
        PORT1_PCNTR1.write_volatile(LED_BIT);
    }
}

fn route_usb_to_ra4(enabled: bool) {
    unsafe {
        if enabled {
            // Match the UNO R4 WiFi variant hook before driving the external USB switch.
            PRCR.write_volatile(0xA502);
            VBTBKR1.write_volatile(40);
            PRCR.write_volatile(0xA500);
        }
        let mut value = PORT4_PCNTR1.read_volatile() | USB_MUX_BIT;
        if enabled {
            value |= USB_MUX_BIT << 16;
        } else {
            value &= !(USB_MUX_BIT << 16);
        }
        PORT4_PCNTR1.write_volatile(value);
    }
}

fn enter_bootloader() -> ! {
    unsafe {
        PRCR.write_volatile(0xA502);
        VBTBKR0.write_volatile(BOOT_DOUBLE_TAP_MAGIC);
        PRCR.write_volatile(0xA500);
        USB_SYSCFG.write_volatile(USB_SYSCFG.read_volatile() & !USB_DPRPU);
    }
    cortex_m::peripheral::SCB::sys_reset();
}

fn led_toggle() {
    unsafe {
        PORT1_PCNTR1.write_volatile(PORT1_PCNTR1.read_volatile() ^ (LED_BIT << 16));
    }
}

// ---------------------------------------------------------------- SCI UARTs (own driver)

/// SCIn register block (0x20 apart): SMR+0, BRR+1, SCR+2, TDR+3, SSR+4, SCMR+6, SEMR+7.
struct Sci(u32);

const SCR_TE_RE: u8 = 0x30;
const SSR_TDRE: u8 = 0x80;
const SSR_RDRF: u8 = 0x40;
const SSR_ERROR_MASK: u8 = 0x38; // ORER | FER | PER
const SSR_CLEAR_ERROR: u8 = 0xC7; // keep TDRE/RDRF/TEND/MPB/MPBT, clear ORER/FER/PER

impl Sci {
    /// 115200 8N1 from PCLKB 24 MHz with BGDM=1: BRR = 24e6 / (16 * 115200) - 1 = 12.
    fn init(&self) {
        unsafe {
            let b = self.0;
            ((b + 2) as *mut u8).write_volatile(0); // SCR: all off while configuring
            (b as *mut u8).write_volatile(0); // SMR: async 8N1, PCLK/1
            ((b + 6) as *mut u8).write_volatile(0xF2); // SCMR: not smart-card mode
            ((b + 7) as *mut u8).write_volatile(0x40); // SEMR.BGDM=1, Arduino/FSP-compatible mode
            ((b + 1) as *mut u8).write_volatile(12); // BRR
            for _ in 0..2_000u32 {
                cortex_m::asm::nop(); // one-bit settle before enabling
            }
            ((b + 4) as *mut u8).write_volatile(SSR_CLEAR_ERROR);
            ((b + 2) as *mut u8).write_volatile(SCR_TE_RE); // SCR: TE + RE
        }
    }
    fn tx(&self, byte: u8) {
        unsafe {
            let b = self.0;
            while ((b + 4) as *const u8).read_volatile() & SSR_TDRE == 0 {} // SSR.TDRE
            ((b + 3) as *mut u8).write_volatile(byte);
        }
    }
    fn print(&self, s: &str) {
        for &byte in s.as_bytes() {
            self.tx(byte);
        }
    }
    fn read(&self) -> Option<u8> {
        unsafe {
            let b = self.0;
            let ssr = ((b + 4) as *const u8).read_volatile();
            if ssr & SSR_ERROR_MASK != 0 {
                ((b + 2) as *mut u8).write_volatile(0);
                let _ = ((b + 5) as *const u8).read_volatile();
                ((b + 4) as *mut u8).write_volatile(SSR_CLEAR_ERROR);
                ((b + 2) as *mut u8).write_volatile(SCR_TE_RE);
                return None;
            }
            if ssr & SSR_RDRF == 0 {
                return None;
            }
            Some(((b + 5) as *const u8).read_volatile())
        }
    }
}

struct BridgeCommand {
    matched: usize,
    dfu_matched: usize,
}

impl BridgeCommand {
    const BOOT: &'static [u8] = b"BOOT!";
    const DFU: &'static [u8] = b"DFU\n";

    fn new() -> Self {
        Self {
            matched: 0,
            dfu_matched: 0,
        }
    }

    fn push_pattern(matched: &mut usize, pattern: &[u8], byte: u8) -> bool {
        if byte == pattern[*matched] {
            *matched += 1;
            if *matched == pattern.len() {
                *matched = 0;
                return true;
            }
        } else if byte == b'\r' {
            // Accept CRLF for line-oriented host tools without treating CR as a mismatch.
        } else {
            *matched = usize::from(byte == pattern[0]);
        }
        false
    }

    fn push(&mut self, byte: u8) -> bool {
        if Self::push_pattern(&mut self.matched, Self::BOOT, byte) {
            return true;
        }
        if Self::push_pattern(&mut self.dfu_matched, Self::DFU, byte) {
            return true;
        }
        false
    }
}

struct HostCommand {
    matched: usize,
}

impl HostCommand {
    const ENTER_BOOTLOADER: &'static [u8] = b"NOBRO_BOOT";

    fn new() -> Self {
        Self { matched: 0 }
    }

    fn push(&mut self, byte: u8) -> bool {
        if byte == Self::ENTER_BOOTLOADER[self.matched] {
            self.matched += 1;
            if self.matched == Self::ENTER_BOOTLOADER.len() {
                self.matched = 0;
                return true;
            }
        } else if byte == b'\r' || byte == b'\n' {
            self.matched = 0;
        } else {
            self.matched = usize::from(byte == Self::ENTER_BOOTLOADER[0]);
        }
        false
    }
}

/// SCI2 = the D0/D1 header pins; SCI1 = the board-internal WiFi-module UART.
const SCI2: Sci = Sci(0x4007_0040);
const SCI1: Sci = Sci(0x4007_0020);

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
    use nobro_usb::{RaUsbfsCdc, UsbConfig, UsbStack};

    system_init();
    pins_init();
    SCI2.init();
    SCI1.init();
    drive_loopback();

    let results = run_all(); // the shared cross-MCU conformance suite (M92)
    let all = results.iter().all(|&r| r);
    let line = if all {
        "NOBRO-RA4M1 arch=thumbv7em subsystems=7 all_pass=1\r\n"
    } else {
        "NOBRO-RA4M1 arch=thumbv7em subsystems=7 all_pass=0\r\n"
    };

    // Emit a short witness on the board-internal WiFi UART, then hand the USB
    // connector to the RA4M1 native USBFS device. If native enumeration fails,
    // the loop restores the stock upload-visible route so the board is not stranded.
    SCI1.print(line);
    cortex_m::asm::delay(200_000);
    route_usb_to_ra4(true);

    // Mount our own RA4M1 USBFS CDC backend (M86) - the app reports over native USB.
    let cfg = UsbConfig::new(0x1209, 0x0004, "NiusRobotLab", "NobroRTOS RA4M1", "NBROR4");
    let mut usb = RaUsbfsCdc::mount(&cfg); // concrete type: exposes stage() for LED debug

    let mut ticks: u32 = 0;
    let mut native_route = true;
    let mut bridge_command = BridgeCommand::new();
    let mut host_command = HostCommand::new();
    loop {
        // service USB frequently so enumeration/control transfers complete
        for _ in 0..2000 {
            let _ = usb.poll();
            if let Some(byte) = SCI1.read() {
                if bridge_command.push(byte) {
                    SCI1.print("NOBRO-RA4M1 boot=bridge\r\n");
                    enter_bootloader();
                }
            }
            let mut usb_bytes = [0u8; 32];
            let count = usb.read(&mut usb_bytes);
            for &byte in &usb_bytes[..count] {
                if host_command.push(byte) {
                    let _ = usb.write(b"NOBRO-RA4M1 boot=native\r\n");
                    enter_bootloader();
                }
            }
            cortex_m::asm::delay(400);
        }
        ticks += 1;
        if usb.configured() {
            let _ = usb.write(line.as_bytes()); // report over native USB
        }
        // Keep the header UART alive and use a non-blocking LED cadence. Blocking
        // blink delays can starve EP0 while the host is enumerating.
        SCI2.print(line);
        let blink_divisor = match usb.stage() {
            nobro_usb::Stage::Configured => 64,
            nobro_usb::Stage::Addressed => 32,
            nobro_usb::Stage::Reset => 16,
            _ => 8,
        };
        if ticks % blink_divisor == 0 {
            led_toggle();
        }
        // A failed native stack must not strand a probe-less board. Keep the
        // native route long enough for Windows to enumerate a fresh CDC device,
        // then restore the upload-visible route if enumeration still failed.
        if native_route && !usb.configured() && ticks >= NATIVE_USB_FALLBACK_TICKS {
            route_usb_to_ra4(false);
            native_route = false;
        }
        if !native_route && ticks % 64 == 0 {
            SCI1.print(line);
            SCI1.print("usb_stage=");
            SCI1.tx(b'0' + usb.stage() as u8);
            SCI1.print("\r\n");
        }
    }
}
