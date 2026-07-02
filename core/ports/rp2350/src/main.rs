//! NobroRTOS portable core on the RP2350 / Pico 2 W (M83) with self-DFU autonomy (M74).
//!
//! Runs the same 7 portable-core subsystem tests as the ESP32-C3 port - a fourth CPU
//! (Cortex-M33) executing the same kernel logic - and reports over USB-CDC:
//!   `NOBRO-RP2350 arch=thumbv8m subsystems=7 all_pass=1`
//! Sending the line `DFU` over the same serial port reboots the chip into the BOOTSEL
//! UF2 bootloader, so the host can reflash without anyone touching the board.
#![no_std]
#![no_main]

use panic_halt as _;
use rp235x_hal as hal;

use hal::usb::UsbBus;
use usb_device::{class_prelude::*, prelude::*};
use usbd_serial::SerialPort;

use nobro_conformance::run_all;

/// RP2350 boot: the bootrom requires this image-definition block.
#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;


#[hal::entry]
fn main() -> ! {
    let mut pac = hal::pac::Peripherals::take().unwrap();
    let mut watchdog = hal::Watchdog::new(pac.WATCHDOG);
    let clocks = hal::clocks::init_clocks_and_plls(
        XTAL_FREQ_HZ,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    )
    .unwrap();
    let mut timer = hal::Timer::new_timer0(pac.TIMER0, &mut pac.RESETS, &clocks);

    let usb_bus = UsbBusAllocator::new(UsbBus::new(
        pac.USB,
        pac.USB_DPRAM,
        clocks.usb_clock,
        true,
        &mut pac.RESETS,
    ));
    let mut serial = SerialPort::new(&usb_bus);
    let mut usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x2E8A, 0x000A))
        .strings(&[StringDescriptors::default()
            .manufacturer("NobroRTOS")
            .product("nobro-rp2350-selftest")
            .serial_number("NBRO2350")])
        .unwrap()
        .device_class(2) // CDC
        .build();

    let results = run_all(); // the shared cross-MCU conformance suite (M92)
    let all = results.iter().all(|&r| r);

    let mut line_buf = [0u8; 16];
    let mut line_len = 0usize;
    let mut last_report = timer.get_counter();

    loop {
        let _ = usb_dev.poll(&mut [&mut serial]);

        // heartbeat once a second
        let now = timer.get_counter();
        if (now - last_report).to_millis() >= 1000 {
            last_report = now;
            let mut msg = [0u8; 64];
            let text = if all {
                &b"NOBRO-RP2350 arch=thumbv8m subsystems=7 all_pass=1\r\n"[..]
            } else {
                &b"NOBRO-RP2350 arch=thumbv8m subsystems=7 all_pass=0\r\n"[..]
            };
            msg[..text.len()].copy_from_slice(text);
            let _ = serial.write(&msg[..text.len()]);
        }

        // self-DFU: the line "DFU" reboots into the BOOTSEL UF2 bootloader
        let mut rx = [0u8; 16];
        if let Ok(n) = serial.read(&mut rx) {
            for &c in &rx[..n] {
                if c == b'\n' || c == b'\r' {
                    if &line_buf[..line_len] == b"DFU" {
                        let _ = serial.write(b"rebooting to BOOTSEL\r\n");
                        // give the host a moment to drain the ack
                        let t0 = timer.get_counter();
                        while (timer.get_counter() - t0).to_millis() < 100 {
                            let _ = usb_dev.poll(&mut [&mut serial]);
                        }
                        hal::reboot::reboot(
                            hal::reboot::RebootKind::BootSel {
                                picoboot_disabled: false,
                                msd_disabled: false,
                            },
                            hal::reboot::RebootArch::Normal,
                        );
                    }
                    line_len = 0;
                } else if line_len < line_buf.len() {
                    line_buf[line_len] = c;
                    line_len += 1;
                }
            }
        }
    }
}
