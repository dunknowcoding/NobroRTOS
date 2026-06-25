//! USB-CDC diagnostics demo: bring up the IMU and stream its eval summary over a USB
//! serial port. This lets boards WITHOUT a J-Link (board2-board5) be verified by
//! opening a COM port - no debug probe or RTT needed. The USB stack is no_std /
//! no-alloc and lives entirely in this app; the kernel is not involved.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _; // provides defmt.x linker section + global logger
use panic_halt as _;

use nrf_usbd::{Usbd, UsbPeripheral};
use usb_device::prelude::*;
use usbd_serial::SerialPort;

use nobro_adapter_mpu9250_imu::{accel_mag_mg, Mpu9250Imu};
use nobro_hal::{
    lease::Resource,
    traits::{HalLease, PlatformHal},
    ActivePlatform as Hal,
};
use nobro_kernel::{pool::SamplePool, ImuPayload};
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

#[entry]
fn main() -> ! {
    let periph = nrf52840_pac::Peripherals::take().unwrap();

    // USB requires the external 32 MHz crystal (HFXO).
    periph.CLOCK.tasks_hfclkstart.write(|w| unsafe { w.bits(1) });
    while periph.CLOCK.events_hfclkstarted.read().bits() == 0 {}
    // Gate on VBUS present. Do NOT wait on OUTPUTRDY: on a VDD-powered board the USB
    // regulator output is bypassed and OUTPUTRDY never sets, which would hang.
    while periph.POWER.usbregstatus.read().vbusdetect().bit_is_clear() {}

    // Bring up the timebase + IMU (TWIM I2C) before starting USB enumeration.
    Hal::acquire(Resource::Timer0, 2).ok();
    unsafe {
        Hal::init_timebase();
    }
    Hal::acquire(Resource::Twim0, OWNER_TWIM).ok();
    let imu = Mpu9250Imu::probe_and_init(OWNER_TWIM);
    let (who, addr, i2c_ok) = match &imu {
        Ok(d) => (u32::from(d.who_am_i()), u32::from(d.addr()), 1u32),
        Err(_) => (0, 0, 0),
    };
    let mut imu = imu.ok();

    let usb_alloc = usb_device::bus::UsbBusAllocator::new(Usbd::new(Nrf52840Usbd));
    let mut serial = SerialPort::new(&usb_alloc);
    let mut dev = UsbDeviceBuilder::new(&usb_alloc, UsbVidPid(0x1209, 0x0001))
        .strings(&[StringDescriptors::default()
            .manufacturer("NiusRobotLab")
            .product("NobroRTOS CDC")
            .serial_number("nobro-1")])
        .unwrap()
        .device_class(usbd_serial::USB_CLASS_CDC)
        .max_packet_size_0(64) // nRF52840 USBD EP0 buffer is 64 bytes
        .unwrap()
        .build();

    let mut reads: u32 = 0;
    let mut errors: u32 = 0;
    let mut accel_mg: u32 = 0;
    let mut spin: u32 = 0;

    loop {
        dev.poll(&mut [&mut serial]);
        if dev.state() != UsbDeviceState::Configured {
            continue;
        }

        // Sample the IMU occasionally; keep dev.poll() the hot path for USB timing.
        spin = spin.wrapping_add(1);
        if spin % 4096 == 0 {
            if let Some(d) = imu.as_mut() {
                match d.poll() {
                    Ok(Some(sample)) => {
                        reads += 1;
                        if let Some(p) = ImuPayload::read_from_handle(sample.handle) {
                            accel_mg = accel_mag_mg(p.accel_g);
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
            let hexd = |d: u32| if d < 10 { b'0' + d as u8 } else { b'a' + (d - 10) as u8 };
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
            let _ = serial.write(&buf[..n]);
        }
    }
}
