//! NobroRTOS portable core on the RA4M1 / Arduino UNO R4 WiFi (M86).
//!
//! Runs the shared cross-MCU conformance suite with **all drivers our own**, written
//! from the RA4M1 hardware manual: PRCR-unlocked clock setup (HOCO 48 MHz with the
//! official peripheral dividers), PFS pin muxing, native USB CDC on the board's
//! own connector, SCI9 through the board USB bridge, SCI1 on the WiFi/AT coprocessor
//! UART, plus a header UART and a PORT1 LED heartbeat (D13/P102). Report line:
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
const HOCOCR: *mut u8 = 0x4001_E036 as *mut u8;
const OSCSF: *const u8 = 0x4001_E03C as *const u8;
const USBCKCR: *mut u8 = 0x4001_E0D0 as *mut u8;
const MSTPCRB: *mut u32 = 0x4004_7000 as *mut u32;
const USB_SYSCFG: *mut u16 = 0x4009_0000 as *mut u16;
const USB_DPRPU: u16 = 1 << 4;
const BOOT_DOUBLE_TAP_MAGIC: u32 = 0x0773_8135;
const OSCSF_HOCOSF: u8 = 1 << 0;
const USBCKCR_HOCO: u8 = 1 << 0;

fn system_init() {
    unsafe {
        VTOR.write_volatile(0x4000);
        PRCR.write_volatile(0xA503); // unlock clock + low-power registers
                                     // Match the board's high-speed boot clock domain and explicitly feed USBFS
                                     // from HOCO. The SCI baud divisor below is calibrated against the resulting
                                     // 48 MHz peripheral clock observed on the UNO R4 WiFi bridge path.
        HOCOCR.write_volatile(HOCOCR.read_volatile() & !1);
        for _ in 0..100_000u32 {
            if OSCSF.read_volatile() & OSCSF_HOCOSF != 0 {
                break;
            }
            cortex_m::asm::nop();
        }
        SCKDIVCR.write_volatile(0x1001_0100);
        USBCKCR.write_volatile(USBCKCR_HOCO);
        PRCR.write_volatile(0xA500); // re-lock
                                     // wake SCI1 WiFi UART, SCI2 header UART, and SCI9 USB-bridge UART from module-stop
        MSTPCRB.write_volatile(MSTPCRB.read_volatile() & !((1 << 30) | (1 << 29) | (1 << 22)));
    }
}

// ---------------------------------------------------------------- pins (own driver)

const PWPR: *mut u8 = 0x4004_0D03 as *mut u8;
/// PmnPFS for P301 (D0/RXD2) and P302 (D1/TXD2): base + port*0x40 + pin*4.
const P301_PFS: *mut u32 = 0x4004_08C4 as *mut u32;
const P302_PFS: *mut u32 = 0x4004_08C8 as *mut u32;
/// P109/P110: SCI9 pair wired to the ESP32-S3 USB bridge user tunnel.
const P109_PFS: *mut u32 = 0x4004_0864 as *mut u32;
const P110_PFS: *mut u32 = 0x4004_0868 as *mut u32;
/// P501/P502: SCI1 pair wired to the board-internal WiFi/AT UART.
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
    /// 115200 8N1 from the SCI peripheral clock at 48 MHz with BGDM=1:
    /// BRR = 48e6 / (16 * 115200) - 1 = 25.
    fn init(&self) {
        unsafe {
            let b = self.0;
            ((b + 2) as *mut u8).write_volatile(0); // SCR: all off while configuring
            (b as *mut u8).write_volatile(0); // SMR: async 8N1, PCLK/1
            ((b + 6) as *mut u8).write_volatile(0xF2); // SCMR: not smart-card mode
            ((b + 7) as *mut u8).write_volatile(0x40); // SEMR.BGDM=1, Arduino/FSP-compatible mode
            ((b + 1) as *mut u8).write_volatile(25); // BRR
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

enum LinkCommand {
    Bootloader,
    NativeUsb,
}

struct BridgeCommand {
    matched: usize,
    dfu_matched: usize,
    native_matched: usize,
}

impl BridgeCommand {
    const BOOT: &'static [u8] = b"BOOT!";
    const DFU: &'static [u8] = b"DFU\n";
    const NATIVE_USB: &'static [u8] = b"NOBRO_NATIVE";

    fn new() -> Self {
        Self {
            matched: 0,
            dfu_matched: 0,
            native_matched: 0,
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

    fn push(&mut self, byte: u8) -> Option<LinkCommand> {
        if Self::push_pattern(&mut self.matched, Self::BOOT, byte) {
            return Some(LinkCommand::Bootloader);
        }
        if Self::push_pattern(&mut self.dfu_matched, Self::DFU, byte) {
            return Some(LinkCommand::Bootloader);
        }
        if Self::push_pattern(&mut self.native_matched, Self::NATIVE_USB, byte) {
            return Some(LinkCommand::NativeUsb);
        }
        None
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

/// SCI9 = the upload-visible USB bridge; SCI2 = D0/D1; SCI1 = WiFi/AT.
const SCI2: Sci = Sci(0x4007_0040);
const SCI1: Sci = Sci(0x4007_0020);
const SCI9: Sci = Sci(0x4007_0120);

/// Drive the loopback output pin (D5=P107) high. The current UNO R4 WiFi bench
/// fixture routes D5 to A0, so the level is externally checkable once a readout
/// channel exists; here it also proves our PORT1 GPIO writes take effect.
fn drive_loopback() {
    unsafe {
        // PORT1 PCNTR1: low half PDR (1=output), high half PODR (output level).
        // outputs: P107 ; levels: P107=1
        let pdr = (1 << 7) | LED_BIT;
        let podr = (1u32 << 7) << 16;
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
    SCI9.init();
    drive_loopback();

    let results = run_all(); // the shared cross-MCU conformance suite (M92)
    let all = results.iter().all(|&r| r);
    let line = if all {
        "NOBRO-RA4M1 arch=thumbv7em subsystems=7 all_pass=1\r\n"
    } else {
        "NOBRO-RA4M1 arch=thumbv7em subsystems=7 all_pass=0\r\n"
    };

    // Keep the UNO R4 WiFi connector on the stock ESP32-S3 bridge. The bridge
    // firmware tunnels COM traffic to SCI9/P109-P110 at 115200 by default and
    // changes the RA-side baud when the host changes CDC line coding.
    route_usb_to_ra4(false);
    SCI9.print(line);
    SCI1.print("NOBRO-RA4M1 wifi_uart=ready\r\n");

    // Native RA4M1 USBFS is available on request; it is not the boot default on
    // UNO R4 WiFi because the board's normal upload/debug path is the ESP bridge.
    let cfg = UsbConfig::new(0x1209, 0x0004, "NiusRobotLab", "NobroRTOS RA4M1", "NBROR4");
    let mut usb: Option<RaUsbfsCdc> = None; // concrete type exposes stage() for LED debug

    let mut ticks: u32 = 0;
    let mut native_route = false;
    let mut bridge_command = BridgeCommand::new();
    let mut host_command = HostCommand::new();
    loop {
        // Service the active transports frequently so neither USB EP0 nor bridge
        // commands wait behind slow diagnostics.
        for _ in 0..2000 {
            if let Some(active_usb) = usb.as_mut() {
                let _ = active_usb.poll();
            }
            if let Some(byte) = SCI9.read() {
                match bridge_command.push(byte) {
                    Some(LinkCommand::Bootloader) => {
                        SCI9.print("NOBRO-RA4M1 boot=bridge\r\n");
                        enter_bootloader();
                    }
                    Some(LinkCommand::NativeUsb) => {
                        SCI9.print("NOBRO-RA4M1 native_usb=enable\r\n");
                        route_usb_to_ra4(true);
                        usb = Some(RaUsbfsCdc::mount(&cfg));
                        native_route = true;
                        ticks = 0;
                    }
                    None => {}
                }
            }
            let mut usb_bytes = [0u8; 32];
            if let Some(active_usb) = usb.as_mut() {
                let count = active_usb.read(&mut usb_bytes);
                for &byte in &usb_bytes[..count] {
                    if host_command.push(byte) {
                        let _ = active_usb.write(b"NOBRO-RA4M1 boot=native\r\n");
                        enter_bootloader();
                    }
                }
            }
            cortex_m::asm::delay(400);
        }
        ticks += 1;
        if let Some(active_usb) = usb.as_mut() {
            if active_usb.configured() {
                let _ = active_usb.write(line.as_bytes()); // report over native USB
            }
        }
        // Keep the bridge/header UARTs alive and use a non-blocking LED cadence.
        // Blocking blink delays can starve EP0 while native USB is enumerating.
        SCI9.print(line);
        SCI2.print(line);
        let usb_stage = usb
            .as_ref()
            .map(RaUsbfsCdc::stage)
            .unwrap_or(nobro_usb::Stage::PoweredOff);
        let blink_divisor = match usb_stage {
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
        if native_route
            && !usb.as_ref().map(RaUsbfsCdc::configured).unwrap_or(false)
            && ticks >= NATIVE_USB_FALLBACK_TICKS
        {
            route_usb_to_ra4(false);
            native_route = false;
            usb = None;
        }
        if !native_route && ticks % 64 == 0 {
            SCI9.print("usb_stage=");
            SCI9.tx(b'0' + usb_stage as u8);
            SCI9.print("\r\n");
        }
    }
}
