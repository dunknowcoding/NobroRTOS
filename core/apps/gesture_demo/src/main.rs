//! IMU gesture recognition on hardware (M143): the streaming GestureDetector classifies
//! synthetic tap/shake/tilt/idle windows on-target, then watches the live SPI MPU-9250
//! at rest for 200 samples (must stay None). NOBRO_GESTURE_REPORT (mem32).
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use embedded_hal::spi::SpiDevice as _;
use nobro_eh_spi::NobroSpiDevice;
use nobro_hal::{
    board,
    lease::Resource,
    traits::{HalLease, PlatformHal},
    ActivePlatform as Hal,
};
use nobro_ml::{Gesture, GestureDetector};

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    synth_ok: u32, // bit0 tap, bit1 shake, bit2 tilt, bit3 idle
    live_none: u32,
    live_samples: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E47_5354; // "NGST"

#[no_mangle]
#[used]
static mut NOBRO_GESTURE_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    synth_ok: 0,
    live_none: 0,
    live_samples: 0,
    checksum: 0,
};

fn rd(dev: &mut NobroSpiDevice, reg: u8) -> Result<u8, ()> {
    let mut rx = [0u8; 2];
    dev.transfer(&mut rx, &[0x80 | reg, 0]).map_err(|_| ())?;
    Ok(rx[1])
}
fn wr(dev: &mut NobroSpiDevice, reg: u8, val: u8) {
    let _ = dev.write(&[reg & 0x7F, val]);
}
fn rd16(dev: &mut NobroSpiDevice, reg_h: u8) -> i16 {
    let h = rd(dev, reg_h).unwrap_or(0);
    let l = rd(dev, reg_h + 1).unwrap_or(0);
    i16::from_be_bytes([h, l])
}

fn isqrt(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

fn fresh_detector() -> GestureDetector {
    let mut g = GestureDetector::new(400, 250, 80);
    for _ in 0..50 {
        g.calibrate(1000);
    }
    g
}

fn run(g: &mut GestureDetector, samples: &[i32]) -> Gesture {
    let mut got = Gesture::None;
    for &s in samples {
        let r = g.update(s);
        if r != Gesture::None {
            got = r;
        }
    }
    got
}

#[entry]
fn main() -> ! {
    // --- synthetic windows on-target ---
    let mut w = [1000i32; 40];
    w[20] = 1600;
    w[21] = 1700;
    w[22] = 1550;
    let tap_ok = run(&mut fresh_detector(), &w) == Gesture::Tap;

    let mut w2 = [1000i32; 40];
    let mut i = 0;
    while i < 16 {
        w2[10 + i] = if i % 2 == 0 { 1400 } else { 600 };
        i += 1;
    }
    let shake_ok = run(&mut fresh_detector(), &w2) == Gesture::Shake;

    let w3 = [1150i32; 40];
    let tilt_ok = run(&mut fresh_detector(), &w3) == Gesture::Tilt;

    let mut w4 = [1000i32; 60];
    let mut k = 0;
    while k < 60 {
        w4[k] += (k as i32 % 5) - 2;
        k += 1;
    }
    let idle_ok = run(&mut fresh_detector(), &w4) == Gesture::None;

    let synth_ok = u32::from(tap_ok)
        | (u32::from(shake_ok) << 1)
        | (u32::from(tilt_ok) << 2)
        | (u32::from(idle_ok) << 3);

    // --- live MPU-9250 at rest: 200 samples must produce no gesture ---
    Hal::acquire(Resource::Timer0, 2).ok();
    unsafe {
        Hal::init_timebase();
    }
    Hal::acquire(Resource::Spim0, 4).ok();
    let mut dev = unsafe {
        NobroSpiDevice::new(
            board::SPI_SCK_PIN,
            board::SPI_MOSI_PIN,
            board::SPI_MISO_PIN,
            board::SPI_CS_PIN,
        )
    };
    wr(&mut dev, 0x6B, 0x80);
    for _ in 0..400_000u32 {
        cortex_m::asm::nop();
    }
    wr(&mut dev, 0x6B, 0x01);
    wr(&mut dev, 0x6A, 0x10);
    wr(&mut dev, 0x6C, 0x00);
    wr(&mut dev, 0x1C, 0x00);
    for _ in 0..400_000u32 {
        cortex_m::asm::nop();
    }
    let afs = (rd(&mut dev, 0x1C).unwrap_or(0) >> 3) & 0x03;
    let div = u64::from(16384u32 >> afs);

    let mag = |dev: &mut NobroSpiDevice| -> i32 {
        let ax = i64::from(rd16(dev, 0x3B));
        let ay = i64::from(rd16(dev, 0x3D));
        let az = i64::from(rd16(dev, 0x3F));
        (isqrt((ax * ax + ay * ay + az * az) as u64) * 1000 / div) as i32
    };

    let mut live = GestureDetector::new(400, 250, 80);
    for _ in 0..50 {
        live.calibrate(mag(&mut dev));
    }
    let mut live_none = 1u32;
    let mut live_samples = 0u32;
    while live_samples < 200 {
        if live.update(mag(&mut dev)) != Gesture::None {
            live_none = 0;
        }
        live_samples += 1;
        for _ in 0..80_000u32 {
            cortex_m::asm::nop();
        }
    }

    let pass = synth_ok == 0xF && live_none == 1 && live_samples == 200;
    let ap = u32::from(pass);
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ synth_ok ^ live_none ^ live_samples;
    unsafe {
        NOBRO_GESTURE_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            synth_ok,
            live_none,
            live_samples,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
