//! NobroRTOS portable core on the RA4M1 / Arduino UNO R4 WiFi (M86).
//!
//! Runs the shared cross-MCU conformance suite with **all drivers our own**, written
//! from the RA4M1 hardware manual: PRCR-unlocked clock setup (HOCO 48 MHz with the
//! official peripheral dividers), PFS pin muxing, an SCI1 UART on P501/P502 - the pins wired to the board's
//! ESP32-S3 USB bridge, so the report arrives on the board's own USB port at 9600 with no
//! extra hardware - plus a header UART and a PORT1 LED heartbeat (D13/P102). Report line:
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
/// P501/P502: SCI1 pair wired to the ESP32-S3 USB bridge (odd SCI group, PSEL 5).
const P501_PFS: *mut u32 = 0x4004_0944 as *mut u32;
const P502_PFS: *mut u32 = 0x4004_0948 as *mut u32;
/// P408 controls the board's USB data switch: high routes the connector to RA4M1.
const P408_PFS: *mut u32 = 0x4004_0920 as *mut u32;
/// PORT1 PCNTR1: low half = direction, high half = output data (LED = P102).
const PORT1_PCNTR1: *mut u32 = 0x4004_0020 as *mut u32;
const PORT4_PCNTR1: *mut u32 = 0x4004_0080 as *mut u32;
const LED_BIT: u32 = 1 << 2;
const USB_MUX_BIT: u32 = 1 << 8;

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

impl Sci {
    /// 9600 8N1 from PCLKB 24 MHz: BRR = 24e6 / (32 * 9600) - 1 = 77 (0.16% error).
    fn init(&self) {
        unsafe {
            let b = self.0;
            ((b + 2) as *mut u8).write_volatile(0); // SCR: all off while configuring
            (b as *mut u8).write_volatile(0); // SMR: async 8N1, PCLK/1
            ((b + 6) as *mut u8).write_volatile(0xF2); // SCMR: not smart-card mode
            ((b + 7) as *mut u8).write_volatile(0); // SEMR
            ((b + 1) as *mut u8).write_volatile(77); // BRR
            for _ in 0..2_000u32 {
                cortex_m::asm::nop(); // one-bit settle before enabling
            }
            ((b + 2) as *mut u8).write_volatile(0x30); // SCR: TE + RE
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
    fn read(&self) -> Option<u8> {
        unsafe {
            let b = self.0;
            if ((b + 4) as *const u8).read_volatile() & 0x40 == 0 {
                return None;
            }
            Some(((b + 5) as *const u8).read_volatile())
        }
    }
}

struct BootCommand {
    matched: usize,
}

impl BootCommand {
    const SEQUENCE: &'static [u8] = b"BOOT!";

    fn new() -> Self {
        Self { matched: 0 }
    }

    fn push(&mut self, byte: u8) -> bool {
        if byte == Self::SEQUENCE[self.matched] {
            self.matched += 1;
            if self.matched == Self::SEQUENCE.len() {
                self.matched = 0;
                return true;
            }
        } else {
            self.matched = usize::from(byte == Self::SEQUENCE[0]);
        }
        false
    }
}

/// SCI2 = the D0/D1 header pins; SCI1 = the ESP32-S3 USB bridge (this is COM-visible).
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

    // Keep the bridge route long enough to emit a boot witness, then hand the USB
    // connector to RA4M1. If native enumeration fails, the loop restores the bridge.
    SCI1.print(line);
    cortex_m::asm::delay(200_000);
    route_usb_to_ra4(true);

    // Mount our own RA4M1 USBFS CDC backend (M86) - the app reports over native USB.
    let cfg = UsbConfig::new(0x1209, 0x0004, "NiusRobotLab", "NobroRTOS RA4M1", "NBROR4");
    let mut usb = RaUsbfsCdc::mount(&cfg); // concrete type: exposes stage() for LED debug

    let mut ticks: u32 = 0;
    let mut native_route = true;
    let mut boot_command = BootCommand::new();
    loop {
        // service USB frequently so enumeration/control transfers complete
        for _ in 0..2000 {
            let _ = usb.poll();
            if let Some(byte) = SCI1.read() {
                if boot_command.push(byte) {
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
        // A failed native stack must not strand a probe-less board. Restore the USB
        // bridge after roughly three seconds so the normal upload path becomes visible.
        if native_route && !usb.configured() && ticks >= 180 {
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
