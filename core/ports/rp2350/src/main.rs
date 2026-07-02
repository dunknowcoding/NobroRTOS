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

use nobro_crypto::Aes128;
use nobro_kernel::{
    Capability, CapabilityGrantTable, CapabilitySet, ModuleId, QuotaLedger, SupervisionAction,
    SystemBudget, TaskSupervisor,
};
use nobro_ml::{ensemble_vote, RunningStats, Vote};
use nobro_net::{RoutingTable, SeenSet};
use nobro_power::{sampling_divisor, PowerManager, PowerMode};

/// RP2350 boot: the bootrom requires this image-definition block.
#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;

fn test_quota() -> bool {
    let mut ledger = QuotaLedger::<2>::new();
    ledger
        .register(ModuleId::Sensor, SystemBudget::new(1024, 256, 2))
        .is_ok()
        && ledger
            .reserve(ModuleId::Sensor, SystemBudget::new(512, 128, 1))
            .is_ok()
        && ledger
            .reserve(ModuleId::Sensor, SystemBudget::new(0, 200, 0))
            .is_err()
}

fn test_capability() -> bool {
    let mut table = CapabilityGrantTable::<2>::new();
    let granted = CapabilitySet::empty().with(Capability::Bus0);
    table.register(ModuleId::Bus, granted).is_ok()
        && table.authorize(ModuleId::Bus, Capability::Bus0).is_ok()
        && table.authorize(ModuleId::Bus, Capability::Radio).is_err()
}

fn test_supervision() -> bool {
    let mut sup = TaskSupervisor::<2>::new(1, 3, 5);
    sup.register(ModuleId::Sensor, 10_000, 0).ok();
    matches!(sup.poll(11_000), SupervisionAction::Restart(ModuleId::Sensor))
        && sup.checkin(ModuleId::Sensor, 12_000).is_ok()
        && matches!(sup.poll(13_000), SupervisionAction::Healthy)
}

fn test_mesh() -> bool {
    let mut rt = RoutingTable::<4>::new();
    rt.update(5, 2, 1, 1);
    rt.update(5, 9, 3, 2);
    let mut seen = SeenSet::<4>::new();
    rt.next_hop(5) == Some(9) && seen.observe(42) && !seen.observe(42)
}

fn test_ml() -> bool {
    let mut s = RunningStats::new();
    for x in [1000.0f32, 1001.0, 999.0, 1000.0, 1002.0, 998.0] {
        s.update(x);
    }
    let votes = [
        Vote { class: 1, confidence_milli: 900 },
        Vote { class: 0, confidence_milli: 600 },
        Vote { class: 1, confidence_milli: 800 },
    ];
    s.is_anomaly(1200.0, 3.0) && !s.is_anomaly(1001.0, 3.0)
        && ensemble_vote(&votes, 3) == Some((1, 739))
}

fn test_crypto() -> bool {
    let key = [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
        0x0d, 0x0e, 0x0f];
    let pt = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
        0xdd, 0xee, 0xff];
    let ct = [0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70,
        0xb4, 0xc5, 0x5a];
    Aes128::new(&key).encrypt_block(&pt) == ct
}

fn test_power() -> bool {
    let pm = PowerManager::new(1_000_000, 100_000);
    pm.select(false, Some(50_000)) == PowerMode::LowPower
        && sampling_divisor(100) == 1
        && sampling_divisor(2) == 16
}

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

    let results = [
        test_quota(),
        test_capability(),
        test_supervision(),
        test_mesh(),
        test_ml(),
        test_crypto(),
        test_power(),
    ];
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
