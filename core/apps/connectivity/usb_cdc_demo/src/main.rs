//! USB-CDC diagnostics demo: bring up the IMU and stream its eval summary over a USB
//! serial port. This lets boards without a debug probe be verified by
//! opening a COM port - no debug probe or RTT needed. The USB stack is no_std /
//! no-alloc and lives entirely in this app; the kernel is not involved.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _; // provides defmt.x linker section + global logger
use panic_halt as _;

use nrf_usbd::{UsbPeripheral, Usbd};
use usb_device::prelude::*;
use usbd_serial::SerialPort;

use nobro_adapter_mpu9250_imu::Mpu9250Imu;
#[cfg(feature = "board-nicenano-s140")]
use nobro_hal::traits::HalClock;
use nobro_hal::{
    lease::Resource,
    traits::{HalLease, HalTimebaseProvider},
    ActivePlatform as Hal,
};
use nobro_kernel::{pool::SamplePool, CompactImuPayload};
use nobro_sal::SensorSal;

/// Zero-sized handle to the nRF52840 USBD register block (base 0x4002_7000).
/// nrf-usbd accesses the peripheral through this and applies the mandatory USB
/// errata workarounds itself.
struct Nrf52840Usbd;
unsafe impl UsbPeripheral for Nrf52840Usbd {
    const REGISTERS: *const () = 0x4002_7000 as *const ();
}

const OWNER_TWIM: u8 = 3;

fn push(buf: &mut [u8], pos: &mut usize, s: &[u8]) {
    for &b in s {
        if *pos < buf.len() {
            buf[*pos] = b;
            *pos += 1;
        }
    }
}

fn push_u32(buf: &mut [u8], pos: &mut usize, mut v: u32) {
    let mut tmp = [0u8; 10];
    let mut n = 0;
    if v == 0 {
        push(buf, pos, b"0");
        return;
    }
    while v > 0 {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        if *pos < buf.len() {
            buf[*pos] = tmp[n];
            *pos += 1;
        }
    }
}

fn device_serial(words: [u32; 2]) -> [u8; 16] {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut serial = [0u8; 16];
    for (word_index, word) in words.into_iter().rev().enumerate() {
        for nibble in 0..8 {
            let shift = 28 - nibble * 4;
            serial[word_index * 8 + nibble] = HEX[((word >> shift) & 0xF) as usize];
        }
    }
    serial
}

fn enter_uf2_bootloader() -> ! {
    unsafe {
        core::ptr::write_volatile(0x4000_051C as *mut u32, 0x57); // POWER.GPREGRET
    }
    cortex_m::peripheral::SCB::sys_reset();
}

#[entry]
fn main() -> ! {
    let periph = nrf52840_pac::Peripherals::take().unwrap();

    // USB requires the external 32 MHz crystal (HFXO).
    periph
        .CLOCK
        .tasks_hfclkstart
        .write(|w| unsafe { w.bits(1) });
    while periph.CLOCK.events_hfclkstarted.read().bits() == 0 {}
    // Gate on VBUS present. Do NOT wait on OUTPUTRDY: on a VDD-powered board the USB
    // regulator output is bypassed and OUTPUTRDY never sets, which would hang.
    while periph.POWER.usbregstatus.read().vbusdetect().bit_is_clear() {}

    // A UF2 bootloader may drive USBD for mass storage and hand off without fully
    // tearing it down, so reinitialization would start from a dirty state and the host
    // could reject it. Mirror TaichiUSB's clean start: disconnect the pullup,
    // disable USBD, clear leftover events, and zero the device address before nrf-usbd
    // re-enables. No-op on a board that already starts clean.
    periph.USBD.usbpullup.write(|w| w.connect().disabled());
    periph.USBD.enable.write(|w| w.enable().disabled());
    periph.USBD.events_usbreset.reset();
    periph.USBD.events_usbevent.reset();
    periph.USBD.events_ep0setup.reset();
    periph
        .USBD
        .eventcause
        .write(|w| unsafe { w.bits(0xFFFF_FFFF) }); // W1C: clear all
    for _ in 0..400_000u32 {
        cortex_m::asm::nop();
    }
    // LED progress indicator so a probe-less board shows how far USB got: it is lit
    // after this point, then in the loop flickers fast once USB is Configured (working)
    // or blinks slowly while enumeration is stuck.
    unsafe {
        nobro_hal::ppi::led_init_output();
        nobro_hal::ppi::led_toggle();
    }

    // Bring up the timebase + IMU (TWIM I2C) before starting USB enumeration.
    Hal::acquire(Resource::Timer0, 2).unwrap_or_else(|_| defmt::panic!("timer lease"));
    unsafe {
        Hal::init_timebase();
    }
    Hal::acquire(Resource::Twim0, OWNER_TWIM).unwrap_or_else(|_| defmt::panic!("I2C lease"));
    let imu = Mpu9250Imu::probe_and_init(OWNER_TWIM);
    let (who, addr, i2c_ok) = match &imu {
        Ok(d) => (u32::from(d.who_am_i()), u32::from(d.addr()), 1u32),
        Err(_) => (0, 0, 0),
    };
    let mut imu = imu.ok();

    let usb_alloc = usb_device::bus::UsbBusAllocator::new(Usbd::new(Nrf52840Usbd));
    let mut serial = SerialPort::new(&usb_alloc);
    // A per-MCU serial prevents Windows from merging identical CDC identities when
    // several boards run this image at the same time.
    let serial_bytes = device_serial([
        periph.FICR.deviceid[0].read().bits(),
        periph.FICR.deviceid[1].read().bits(),
    ]);
    let serial_id = unsafe { core::str::from_utf8_unchecked(&serial_bytes) };
    let mut dev = UsbDeviceBuilder::new(&usb_alloc, UsbVidPid(0x1209, 0x0001))
        .strings(&[StringDescriptors::default()
            .manufacturer("NiusRobotLab")
            .product("NobroRTOS CDC")
            .serial_number(serial_id)])
        .unwrap()
        .device_class(usbd_serial::USB_CLASS_CDC)
        .max_packet_size_0(64) // nRF52840 USBD EP0 buffer is 64 bytes
        .unwrap()
        .build();

    let mut reads: u32 = 0;
    let mut errors: u32 = 0;
    let mut accel_mg: u32 = 0;
    let mut spin: u32 = 0;

    let mut blink: u32 = 0;
    #[cfg(feature = "board-nicenano-s140")]
    let usb_start = Hal::now_us();
    #[cfg(feature = "board-nicenano-s140")]
    let mut ever_configured = false;
    let mut max_state: u8 = 0; // 0=Default, 1=Addressed, 2=Configured (max reached)
    let mut dfu_command_pos: u8 = 0;
    loop {
        dev.poll(&mut [&mut serial]);
        blink = blink.wrapping_add(1);
        let s = match dev.state() {
            UsbDeviceState::Addressed => 1u8,
            UsbDeviceState::Configured => 2u8,
            _ => 0u8,
        };
        if s > max_state {
            max_state = s;
        }
        let configured = s >= 2;
        #[cfg(feature = "board-nicenano-s140")]
        if configured {
            ever_configured = true;
        }
        // Self-recovery: if USB never enumerates, reboot into the UF2 bootloader so the
        // next firmware can be flashed without a manual double-tap. GPREGRET = 0x57 is
        // the Adafruit-compatible DFU magic. This path is gated to the S140 profile.
        #[cfg(feature = "board-nicenano-s140")]
        if !ever_configured && Hal::now_us().saturating_sub(usb_start) > 30_000_000 {
            enter_uf2_bootloader();
        }
        // Fast flicker once Configured (USB working); slow blink while enumeration is
        // stuck. Lets a probe-less board report its USB state on the LED.
        // LED rate reports how far USB enumeration got: SLOW (~1 Hz) = stuck at Default
        // (device descriptor / first control transfer fails); MEDIUM (~5 Hz) = reached
        // Addressed (SET_ADDRESS ok, config stage fails); FAST flicker = Configured.
        let period = match max_state {
            2 => 20_000,
            1 => 150_000,
            _ => 700_000,
        };
        if blink % period == 0 {
            unsafe {
                nobro_hal::ppi::led_toggle();
            }
        }
        if !configured {
            continue;
        }

        // A line containing only "DFU" enters the UF2 bootloader. This keeps future
        // update cycles host-driven even on boards without a debug probe.
        let mut command_bytes = [0u8; 16];
        if let Ok(count) = serial.read(&mut command_bytes) {
            for &byte in &command_bytes[..count] {
                dfu_command_pos = match (dfu_command_pos, byte) {
                    (0, b'D') => 1,
                    (1, b'F') => 2,
                    (2, b'U') => 3,
                    (3, b'\r') => 3,
                    (3, b'\n') => enter_uf2_bootloader(),
                    (_, b'\n') => 0,
                    _ => 0,
                };
            }
        }

        // Sample the IMU occasionally; keep dev.poll() the hot path for USB timing.
        spin = spin.wrapping_add(1);
        if spin % 4096 == 0 {
            if let Some(d) = imu.as_mut() {
                match d.poll() {
                    Ok(Some(sample)) => {
                        reads += 1;
                        if let Some(p) = CompactImuPayload::read_from_handle(sample.handle) {
                            accel_mg = p.into_sample(sample.captured_us).accel_mag_mg;
                        }
                        SamplePool::release(sample.handle);
                    }
                    Ok(None) => {}
                    Err(_) => errors += 1,
                }
            }
        }

        if spin % 600_000 == 0 {
            let mut buf = [0u8; 128];
            let mut n = 0usize;
            push(&mut buf, &mut n, b"NobroRTOS IMU who=0x");
            let hi = (who >> 4) & 0xF;
            let lo = who & 0xF;
            let hexd = |d: u32| {
                if d < 10 {
                    b'0' + d as u8
                } else {
                    b'a' + (d - 10) as u8
                }
            };
            if n + 2 <= buf.len() {
                buf[n] = hexd(hi);
                buf[n + 1] = hexd(lo);
                n += 2;
            }
            push(&mut buf, &mut n, b" addr=");
            push_u32(&mut buf, &mut n, addr);
            push(&mut buf, &mut n, b" i2c=");
            push_u32(&mut buf, &mut n, i2c_ok);
            push(&mut buf, &mut n, b" reads=");
            push_u32(&mut buf, &mut n, reads);
            push(&mut buf, &mut n, b" err=");
            push_u32(&mut buf, &mut n, errors);
            push(&mut buf, &mut n, b" accel=");
            push_u32(&mut buf, &mut n, accel_mg);
            push(&mut buf, &mut n, b"mg temp=");
            push_u32(
                &mut buf,
                &mut n,
                imu.as_ref().map(|d| d.last_temp_centi_c()).unwrap_or(0),
            );
            push(&mut buf, &mut n, b" gyro=");
            push_u32(
                &mut buf,
                &mut n,
                imu.as_ref().map(|d| d.last_gyro_mag_mdps()).unwrap_or(0),
            );
            push(&mut buf, &mut n, b"mdps ");
            let pass = i2c_ok == 1 && reads >= 10 && (800..1200).contains(&accel_mg);
            push(&mut buf, &mut n, if pass { b"PASS\r\n" } else { b"..\r\n" });
            if serial.write(&buf[..n]).is_err() {
                defmt::warn!("USB telemetry backpressure");
            }

            // Machine-decodable twin of the line above, in the standard
            // `NOBRO-<NAME> key=value` shape the host tools and the web-flasher
            // report console parse (nobro_rtos.node / parseStatusLine).
            let mut mline = [0u8; 96];
            let mut m = 0usize;
            push(&mut mline, &mut m, b"NOBRO-CDC who=");
            push_u32(&mut mline, &mut m, who);
            push(&mut mline, &mut m, b" reads=");
            push_u32(&mut mline, &mut m, reads);
            push(&mut mline, &mut m, b" errors=");
            push_u32(&mut mline, &mut m, errors);
            push(&mut mline, &mut m, b" accel_mg=");
            push_u32(&mut mline, &mut m, accel_mg);
            push(&mut mline, &mut m, b" all_pass=");
            push_u32(&mut mline, &mut m, u32::from(pass));
            push(&mut mline, &mut m, b"\r\n");
            if serial.write(&mline[..m]).is_err() {
                defmt::warn!("USB model-report backpressure");
            }
        }
    }
}
