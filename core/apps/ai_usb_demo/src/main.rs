//! Edge AI you can watch: stream live motion classifications over USB serial.
//!
//! Brings up the IMU, runs the on-device MotionClassifier (AiInferenceSal) over a
//! window of accel-magnitude samples, and (1) records the result in
//! NOBRO_AI_USB_REPORT - written during a warm-up BEFORE USB, so it is J-Link
//! readable even if USB enumeration stalls - and (2) streams idle/active + a Q15
//! confidence over a COM port once the host configures the device. no_std / no-alloc.
#![no_std]
#![no_main]

use cortex_m::asm;
use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nrf_usbd::{UsbPeripheral, Usbd};
use usb_device::prelude::*;
use usbd_serial::SerialPort;

use nobro_adapter_motion_ai::{MotionClassifier, CLASS_ACTIVE};
use nobro_adapter_mpu9250_imu::{accel_mag_mg, Mpu9250Imu};
use nobro_hal::{
    lease::Resource,
    traits::{HalLease, HalTimebaseProvider},
    ActivePlatform as Hal,
};
use nobro_kernel::{pool::SamplePool, ImuPayload};
use nobro_sal::{AiInferenceRequest, AiInferenceSal, SensorSal};

struct Nrf52840Usbd;
unsafe impl UsbPeripheral for Nrf52840Usbd {
    const REGISTERS: *const () = 0x4002_7000 as *const ();
}

const OWNER_TWIM: u8 = 3;
const WINDOW: usize = 16;

#[repr(C)]
#[derive(Clone, Copy)]
struct AiUsbReport {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    model_id: u32,
    inferences: u32,
    last_class: u32,
    confidence_q15: u32,
    accel_mg: u32,
    checksum: u32,
}
const AI_USB_MAGIC: u32 = 0x4E42_4155; // "NBAU"

#[no_mangle]
#[used]
static mut NOBRO_AI_USB_REPORT: AiUsbReport = AiUsbReport {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    model_id: 0,
    inferences: 0,
    last_class: 0,
    confidence_q15: 0,
    accel_mg: 0,
    checksum: 0,
};

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
    periph
        .CLOCK
        .tasks_hfclkstart
        .write(|w| unsafe { w.bits(1) });
    while periph.CLOCK.events_hfclkstarted.read().bits() == 0 {}
    while periph.POWER.usbregstatus.read().vbusdetect().bit_is_clear() {}

    Hal::acquire(Resource::Timer0, 2).ok();
    unsafe {
        Hal::init_timebase();
    }
    Hal::acquire(Resource::Twim0, OWNER_TWIM).ok();
    let mut imu = Mpu9250Imu::probe_and_init(OWNER_TWIM).ok();

    let mut model = MotionClassifier::new();
    let model_id = model.contract().model_id;

    let mut window = [0u16; WINDOW];
    let mut widx = 0usize;
    let mut accel_mg = 0u16;
    let mut last_class = 0u8;
    let mut last_conf_q15 = 0u16;
    let mut inferences = 0u32;

    unsafe {
        NOBRO_AI_USB_REPORT.magic = AI_USB_MAGIC;
        NOBRO_AI_USB_REPORT.version = 1;
        NOBRO_AI_USB_REPORT.model_id = model_id;
    }

    // One inference step: pull a sample; on a full window classify + record the
    // result. Returns true when an inference completed.
    macro_rules! step {
        () => {{
            let mut did = false;
            if let Some(d) = imu.as_mut() {
                if let Ok(Some(sample)) = d.poll() {
                    if let Some(p) = ImuPayload::read_from_handle(sample.handle) {
                        accel_mg = accel_mag_mg(p.accel_g) as u16;
                        window[widx] = accel_mg;
                        widx += 1;
                        if widx >= WINDOW {
                            widx = 0;
                            let mut input = [0u8; WINDOW * 2];
                            for i in 0..WINDOW {
                                input[2 * i..2 * i + 2].copy_from_slice(&window[i].to_le_bytes());
                            }
                            let mut out = [0u8; 4];
                            let req = AiInferenceRequest::new(model_id, &input, 0);
                            if let Ok(res) = model.infer(req, &mut out) {
                                last_class = out[0];
                                last_conf_q15 = res.confidence_q15;
                                inferences = inferences.wrapping_add(1);
                                let pass = inferences >= 4
                                    && last_class != CLASS_ACTIVE
                                    && last_conf_q15 >= 16_000
                                    && (800..1200).contains(&u32::from(accel_mg));
                                let completed = u32::from(inferences >= 4);
                                let all_pass = u32::from(pass);
                                let cs = AI_USB_MAGIC
                                    ^ 1
                                    ^ completed
                                    ^ all_pass
                                    ^ model_id
                                    ^ inferences
                                    ^ u32::from(last_class)
                                    ^ u32::from(last_conf_q15)
                                    ^ u32::from(accel_mg);
                                unsafe {
                                    NOBRO_AI_USB_REPORT.completed = completed;
                                    NOBRO_AI_USB_REPORT.all_pass = all_pass;
                                    NOBRO_AI_USB_REPORT.inferences = inferences;
                                    NOBRO_AI_USB_REPORT.last_class = u32::from(last_class);
                                    NOBRO_AI_USB_REPORT.confidence_q15 = u32::from(last_conf_q15);
                                    NOBRO_AI_USB_REPORT.accel_mg = u32::from(accel_mg);
                                    NOBRO_AI_USB_REPORT.checksum = cs;
                                }
                                did = true;
                            }
                        }
                    }
                    SamplePool::release(sample.handle);
                }
            }
            did
        }};
    }

    // Warm-up: record several inferences BEFORE USB so the report is readable even
    // if USB enumeration stalls on a given host.
    let mut warm = 0u32;
    while warm < 6 {
        if step!() {
            warm += 1;
        }
        asm::delay(40_000);
    }

    // Bring up USB and stream live.
    let usb_alloc = usb_device::bus::UsbBusAllocator::new(Usbd::new(Nrf52840Usbd));
    let mut serial = SerialPort::new(&usb_alloc);
    let mut dev = UsbDeviceBuilder::new(&usb_alloc, UsbVidPid(0x1209, 0x0001))
        .strings(&[StringDescriptors::default()
            .manufacturer("NiusRobotLab")
            .product("NobroRTOS AI")
            .serial_number("nobro-ai")])
        .unwrap()
        .device_class(usbd_serial::USB_CLASS_CDC)
        .max_packet_size_0(64)
        .unwrap()
        .build();

    let mut spin = 0u32;
    loop {
        dev.poll(&mut [&mut serial]);
        spin = spin.wrapping_add(1);
        if spin % 4096 == 0 {
            let _ = step!();
        }
        if dev.state() == UsbDeviceState::Configured && spin % 600_000 == 0 {
            let mut buf = [0u8; 96];
            let mut n = 0usize;
            push(&mut buf, &mut n, b"NobroRTOS AI class=");
            push(
                &mut buf,
                &mut n,
                if last_class == CLASS_ACTIVE {
                    b"active"
                } else {
                    b"idle"
                },
            );
            push(&mut buf, &mut n, b" conf=");
            push_u32(&mut buf, &mut n, u32::from(last_conf_q15) * 1000 / 32767);
            push(&mut buf, &mut n, b"/1000 accel=");
            push_u32(&mut buf, &mut n, u32::from(accel_mg));
            push(&mut buf, &mut n, b"mg\r\n");
            let _ = serial.write(&buf[..n]);
        }
    }
}
