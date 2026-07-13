//! NobroRTOS portable core on the RA4M1 / Arduino UNO R4 WiFi.
//!
//! Provides target startup and serial status for the native Rust composition: clock,
//! one-shot deadline timer, and the selected USB stack. Clock/pin/SCI setup is a small
//! register-level port; Arduino ADC/PWM/I2C/SPI facades are a separate composition and
//! are not counted here. A report reaches `usb=1 all_pass=1` only after CDC configures.
//!
//! The image links at 0x4000 behind the stock DFU bootloader; SCB->VTOR is pointed at
//! our vector table first thing (the hard-won nRF lesson - the bootloader leaves VTOR
//! at its own table). Double-tap RESET always returns the board to DFU.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use nobro_hal::{HalAlarm, HalByteIo, HalClock};
use nobro_port_ra4m1::evidence::ProviderEvidence;
use nobro_port_ra4m1::providers::{Ra4m1Alarm, Ra4m1Clock, Ra4m1Usb};
use nobro_port_ra4m1::system::{
    configure_system, SystemRegisters, MEMWAIT_48MHZ, SCI_BRR, SCKDIVCR_VALUE, SCKSCR_HOCO,
};
use nobro_port_ra4m1::usb_session::{HostCommand, UsbReportCursor};
use nobro_usb::UsbIoError;
use panic_halt as _;

// ---------------------------------------------------------------- system (own driver)

const VTOR: *mut u32 = 0xE000_ED08 as *mut u32;
const PRCR: *mut u16 = 0x4001_E3FE as *mut u16;
const VBTBKR0: *mut u32 = 0x4001_E500 as *mut u32;
const VBTBKR1: *mut u8 = 0x4001_E501 as *mut u8;
const SCKDIVCR: *mut u32 = 0x4001_E020 as *mut u32;
const SCKSCR: *mut u8 = 0x4001_E026 as *mut u8;
const MEMWAIT: *mut u8 = 0x4001_E031 as *mut u8;
const HOCOCR: *mut u8 = 0x4001_E036 as *mut u8;
const OSCSF: *const u8 = 0x4001_E03C as *const u8;
const USBCKCR_ALT: *mut u8 = 0x4001_E0D0 as *mut u8;
const MSTPCRB: *mut u32 = 0x4004_7000 as *mut u32;
const USB_SYSCFG: *mut u16 = 0x4009_0000 as *mut u16;
const USB_DPRPU: u16 = 1 << 4;
const BOOT_DOUBLE_TAP_MAGIC: u32 = 0x0773_8135;
const OSCSF_HOCOSF: u8 = 1 << 0;
const USBCKCR_ALT_HOCO: u8 = 1 << 0;

struct RaSystemRegisters;

impl SystemRegisters for RaSystemRegisters {
    fn set_vector_table(&mut self) {
        unsafe { VTOR.write_volatile(0x4000) };
    }

    fn unlock_protected_registers(&mut self) {
        unsafe { PRCR.write_volatile(0xA503) };
    }

    fn start_hoco(&mut self) {
        unsafe { HOCOCR.write_volatile(HOCOCR.read_volatile() & !1) };
    }

    fn hoco_stable(&self) -> bool {
        unsafe { OSCSF.read_volatile() & OSCSF_HOCOSF != 0 }
    }

    fn enable_flash_wait_state(&mut self) {
        unsafe { MEMWAIT.write_volatile(MEMWAIT_48MHZ) };
    }

    fn program_clock_dividers(&mut self) {
        // ICLK=48 MHz and PCLKB=24 MHz, matching the board core's clock plan.
        unsafe { SCKDIVCR.write_volatile(SCKDIVCR_VALUE) };
    }

    fn select_hoco_as_system_clock(&mut self) {
        unsafe { SCKSCR.write_volatile(SCKSCR_HOCO) };
    }

    fn clock_tree_matches(&self) -> bool {
        unsafe {
            MEMWAIT.read_volatile() & 1 == MEMWAIT_48MHZ
                && SCKDIVCR.read_volatile() == SCKDIVCR_VALUE
                && SCKSCR.read_volatile() & 0x07 == SCKSCR_HOCO
        }
    }

    fn select_hoco_for_usb(&mut self) {
        // RA4M1 uses the alternate USB clock selector at 0x4001_E0D0.
        unsafe { USBCKCR_ALT.write_volatile(USBCKCR_ALT_HOCO) };
    }

    fn usb_hoco_selected(&self) -> bool {
        unsafe { USBCKCR_ALT.read_volatile() & USBCKCR_ALT_HOCO != 0 }
    }

    fn wake_required_modules(&mut self) {
        // PRCR.PRC1 remains unlocked until this write completes.
        unsafe {
            MSTPCRB.write_volatile(MSTPCRB.read_volatile() & !((1 << 30) | (1 << 29) | (1 << 22)))
        };
    }

    fn lock_protected_registers(&mut self) {
        unsafe { PRCR.write_volatile(0xA500) };
    }
}

fn system_init() -> bool {
    configure_system(&mut RaSystemRegisters).is_ok()
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

fn clock_failure_signal() -> ! {
    // Do not touch SCI, DWT, SysTick, USB, or any peripheral whose clock assumptions
    // failed validation. GPIO remains available from reset and gives a bounded visual
    // fault signal without risking a wait on a stopped module.
    unsafe { PORT1_PCNTR1.write_volatile(LED_BIT) };
    loop {
        led_toggle();
        cortex_m::asm::delay(1_000_000);
    }
}

// ---------------------------------------------------------------- SCI UARTs (own driver)

/// SCIn register block (0x20 apart): SMR+0, BRR+1, SCR+2, TDR+3, SSR+4, SCMR+6, SEMR+7.
struct Sci(u32);

const SCR_TE_RE: u8 = 0x30;
const SSR_TDRE: u8 = 0x80;
const SSR_RDRF: u8 = 0x40;
const SSR_TEND: u8 = 0x04;
const SSR_ERROR_MASK: u8 = 0x38; // ORER | FER | PER
const SSR_CLEAR_ERROR: u8 = 0xC7; // keep TDRE/RDRF/TEND/MPB/MPBT, clear ORER/FER/PER

impl Sci {
    /// 115200 8N1 from PCLKB=HOCO/2=24 MHz with BGDM=1, ABCS=0, CKS=0.
    /// The rounded divisor is BRR=12 (115384 baud, about +0.16%).
    fn init(&self) {
        unsafe {
            let b = self.0;
            ((b + 2) as *mut u8).write_volatile(0); // SCR: all off while configuring
            (b as *mut u8).write_volatile(0); // SMR: async 8N1, PCLK/1
            ((b + 6) as *mut u8).write_volatile(0xF2); // SCMR: not smart-card mode
            ((b + 7) as *mut u8).write_volatile(0x40); // SEMR.BGDM=1, Arduino/FSP-compatible mode
            ((b + 1) as *mut u8).write_volatile(SCI_BRR);
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

    fn try_print_until(&self, s: &str, deadline_us: u64) -> bool {
        unsafe {
            let b = self.0;
            for &byte in s.as_bytes() {
                while ((b + 4) as *const u8).read_volatile() & SSR_TDRE == 0 {
                    if Ra4m1Clock::now_us() >= deadline_us {
                        return false;
                    }
                }
                ((b + 3) as *mut u8).write_volatile(byte);
            }
            while ((b + 4) as *const u8).read_volatile() & SSR_TEND == 0 {
                if Ra4m1Clock::now_us() >= deadline_us {
                    return false;
                }
            }
        }
        true
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

/// SCI9 = the upload-visible USB bridge; SCI2 = D0/D1; SCI1 = WiFi/AT.
const SCI2: Sci = Sci(0x4007_0040);
const SCI1: Sci = Sci(0x4007_0020);
const SCI9: Sci = Sci(0x4007_0120);

const NATIVE_USB_FALLBACK_US: u64 = 3_000_000;
const RESET_CONFIRMATION_US: u64 = 50_000;

fn try_native_reset_confirmation(usb: &mut Ra4m1Usb) {
    const CONFIRMATION: &[u8] = b"NOBRO-RA4M1 boot=native\r\n";
    let deadline = Ra4m1Clock::now_us().saturating_add(RESET_CONFIRMATION_US);
    let queued = loop {
        usb.poll();
        match usb.write_all(CONFIRMATION) {
            Ok(()) => break true,
            Err(UsbIoError::Backpressure) if Ra4m1Clock::now_us() < deadline => {}
            Err(_) => break false,
        }
        if Ra4m1Clock::now_us() >= deadline {
            break false;
        }
    };
    if !queued {
        return;
    }
    while Ra4m1Clock::now_us() < deadline {
        usb.poll();
        match usb.flush() {
            Ok(()) => return,
            Err(UsbIoError::Backpressure) => {}
            Err(_) => return,
        }
    }
}

/// Drive the UNO R4 D5 output pin (P107) high for the GPIO example.
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
    let system_ok = system_init();
    if !system_ok {
        clock_failure_signal();
    }
    let mut core = cortex_m::Peripherals::take().unwrap();
    Ra4m1Clock::init(&mut core.DCB, &mut core.DWT);
    let mut alarm = Ra4m1Alarm::new(core.SYST);
    pins_init();
    SCI2.init();
    SCI1.init();
    SCI9.init();
    drive_loopback();

    let started = Ra4m1Clock::now_us();
    let armed = alarm.arm_after_us(2_000).is_ok();
    while armed && !alarm.poll_due(Ra4m1Clock::now_us()) {}
    let elapsed = Ra4m1Clock::now_us().saturating_sub(started);
    let core_evidence = ProviderEvidence::new(
        system_ok,
        elapsed != 0,
        armed && (2_000..20_000).contains(&elapsed),
        false,
    );
    let bridge_line = if core_evidence.core_passes() {
        "NOBRO-RA4M1 arch=thumbv7em providers=3 timebase=1 deadline=1 usb=0 all_pass=0\r\n"
    } else {
        "NOBRO-RA4M1 arch=thumbv7em providers=3 timebase=0 deadline=0 usb=0 all_pass=0\r\n"
    };
    let native_line = if core_evidence.with_usb(true).all_passes() {
        "NOBRO-RA4M1 arch=thumbv7em providers=3 timebase=1 deadline=1 usb=1 all_pass=1\r\n"
    } else {
        "NOBRO-RA4M1 arch=thumbv7em providers=3 timebase=0 deadline=0 usb=1 all_pass=0\r\n"
    };

    // Keep the UNO R4 WiFi connector on the stock ESP32-S3 bridge. The bridge
    // firmware tunnels COM traffic to SCI9/P109-P110 at 115200 by default and
    // changes the RA-side baud when the host changes CDC line coding.
    route_usb_to_ra4(false);
    SCI9.print(bridge_line);
    SCI1.print("NOBRO-RA4M1 wifi_uart=ready\r\n");

    // Native RA4M1 USBFS is available on request; it is not the boot default on
    // UNO R4 WiFi because the board's normal upload/debug path is the ESP bridge.
    let mut usb: Option<Ra4m1Usb> = None;
    let mut native_report = UsbReportCursor::new();

    let mut ticks: u32 = 0;
    let mut native_route = false;
    let mut native_fallback_deadline: Option<u64> = None;
    let mut bridge_command = BridgeCommand::new();
    let mut host_command = HostCommand::new();
    loop {
        // Service the active transports frequently so neither USB EP0 nor bridge
        // commands wait behind slow diagnostics.
        for _ in 0..2000 {
            if let Some(active_usb) = usb.as_mut() {
                active_usb.poll();
            }
            if let Some(byte) = SCI9.read() {
                match bridge_command.push(byte) {
                    Some(LinkCommand::Bootloader) => {
                        let deadline = Ra4m1Clock::now_us().saturating_add(RESET_CONFIRMATION_US);
                        let _ = SCI9.try_print_until("NOBRO-RA4M1 boot=bridge\r\n", deadline);
                        enter_bootloader();
                    }
                    Some(LinkCommand::NativeUsb) => {
                        if system_ok {
                            SCI9.print("NOBRO-RA4M1 native_usb=enable\r\n");
                            route_usb_to_ra4(true);
                            let mount_result = if let Some(active_usb) = usb.as_mut() {
                                active_usb.reconnect_link();
                                Ok(())
                            } else {
                                Ra4m1Usb::try_mount().map(|mounted| usb = Some(mounted))
                            };
                            match mount_result {
                                Ok(()) => {
                                    native_report.reset();
                                    host_command.observe_link(false);
                                    native_route = true;
                                    native_fallback_deadline = Some(
                                        Ra4m1Clock::now_us().saturating_add(NATIVE_USB_FALLBACK_US),
                                    );
                                    ticks = 0;
                                }
                                Err(nobro_usb::UsbMountError::AlreadyMounted) => {
                                    route_usb_to_ra4(false);
                                    SCI9.print("NOBRO-RA4M1 native_usb=already_mounted\r\n");
                                }
                                Err(nobro_usb::UsbMountError::UnsupportedConfig) => {
                                    route_usb_to_ra4(false);
                                    SCI9.print("NOBRO-RA4M1 native_usb=unsupported_config\r\n");
                                }
                                Err(_) => {
                                    route_usb_to_ra4(false);
                                    SCI9.print("NOBRO-RA4M1 native_usb=mount_error\r\n");
                                }
                            }
                        } else {
                            SCI9.print("NOBRO-RA4M1 native_usb=clock_error\r\n");
                        }
                    }
                    None => {}
                }
            }
            let mut usb_bytes = [0u8; 32];
            if let Some(active_usb) = usb.as_mut() {
                let configured = active_usb.configured();
                host_command.observe_link(configured);
                let count = match active_usb.read_available(&mut usb_bytes) {
                    Ok(count) => count,
                    Err(_) => {
                        // A failed read cannot contribute bytes to a command that a
                        // later USB session completes.
                        host_command.observe_link(false);
                        0
                    }
                };
                for &byte in &usb_bytes[..count] {
                    if host_command.push(byte) {
                        try_native_reset_confirmation(active_usb);
                        enter_bootloader();
                    }
                }
            }
            cortex_m::asm::delay(400);
        }
        ticks += 1;
        if let Some(active_usb) = usb.as_mut() {
            let configured = active_usb.configured();
            if configured {
                // Finish a short write promptly; otherwise emit at a bounded cadence.
                if native_report.pending() || ticks.is_multiple_of(64) {
                    native_report.service(true, native_line.as_bytes(), |packet| {
                        active_usb.write_all(packet)
                    });
                }
            } else {
                native_report.service(false, native_line.as_bytes(), |_| {
                    unreachable!("detached report callback")
                });
            }
        }
        // Keep the bridge/header UARTs alive and use a non-blocking LED cadence.
        // Blocking blink delays can starve EP0 while native USB is enumerating.
        SCI9.print(bridge_line);
        SCI2.print(bridge_line);
        let usb_stage = usb
            .as_ref()
            .map(Ra4m1Usb::stage)
            .unwrap_or(nobro_usb::Stage::PoweredOff);
        let blink_divisor = match usb_stage {
            nobro_usb::Stage::Configured => 64,
            nobro_usb::Stage::Addressed => 32,
            nobro_usb::Stage::Reset => 16,
            _ => 8,
        };
        if ticks.is_multiple_of(blink_divisor) {
            led_toggle();
        }
        // A failed native stack must not strand a probe-less board. Keep the
        // native route long enough for Windows to enumerate a fresh CDC device,
        // then restore the upload-visible route if enumeration still failed.
        if native_route
            && !usb.as_ref().map(Ra4m1Usb::configured).unwrap_or(false)
            && native_fallback_deadline.is_some_and(|deadline| Ra4m1Clock::now_us() >= deadline)
        {
            if let Some(active_usb) = usb.as_mut() {
                active_usb.disconnect_link();
            }
            route_usb_to_ra4(false);
            native_report.reset();
            host_command.observe_link(false);
            native_route = false;
            native_fallback_deadline = None;
        }
        if !native_route && ticks.is_multiple_of(64) {
            SCI9.print("usb_stage=");
            SCI9.tx(b'0' + usb_stage as u8);
            SCI9.print("\r\n");
        }
    }
}
