//! Universal Driver Interface proof (UDI): one app, one sensor, swappable backends.
//!
//! The evaluation logic below is written **only** against `nobro_sal::ImuSal` - it
//! never names SPI, a register map, or a driver crate. Which backend actually talks to
//! the MPU-9250 is a cargo feature:
//!
//! ```text
//! cargo build -p udi-imu-demo                                      # backend-native
//! cargo build -p udi-imu-demo --no-default-features \
//!       --features board-promicro-nosd,backend-eh                  # embedded-hal
//! ```
//!
//! Both builds must seal the same PASS in `NOBRO_UDI_IMU_REPORT` on the same board;
//! `backend_id` records which one produced it (1 = native HAL, 2 = embedded-hal).
//! This is the Adafruit-Unified-Sensor idea done with compile-time mounts: category
//! trait + catalog part + N interchangeable transports.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use nobro_sal::{ImuSal, ImuSample};
use panic_halt as _;

#[repr(C)]
#[derive(Clone, Copy)]
struct UdiImuReport {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    backend_id: u32,
    who_am_i: u32,
    accel_mag_mg: u32,
    reads: u32,
    errors: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E55_4449; // "NUDI"

#[no_mangle]
#[used]
static mut NOBRO_UDI_IMU_REPORT: UdiImuReport = UdiImuReport {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    backend_id: 0,
    who_am_i: 0,
    accel_mag_mg: 0,
    reads: 0,
    errors: 0,
    checksum: 0,
};

/// Integer square root (Newton) for the accel magnitude - no float, no libm.
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

/// Raw MPU-9250 accel counts (+/-2 g, 16384 LSB/g) -> category-level ImuSample.
fn counts_to_sample(ax: i16, ay: i16, az: i16) -> ImuSample {
    let to_mg = |v: i16| (i32::from(v) * 1000) / 16384;
    let (mx, my, mz) = (to_mg(ax), to_mg(ay), to_mg(az));
    let sum = (i64::from(mx) * i64::from(mx)
        + i64::from(my) * i64::from(my)
        + i64::from(mz) * i64::from(mz)) as u64;
    ImuSample { accel_mg: [mx, my, mz], accel_mag_mg: isqrt(sum) as u32 }
}

/// The MPU-9250 bring-up sequence in transport-neutral form: (reg, value) writes.
/// Reset is handled separately (needs a settle delay after it).
const MPU_INIT: [(u8, u8); 7] = [
    (0x6B, 0x01), // PWR_MGMT_1: wake, auto clock
    (0x6A, 0x10), // USER_CTRL: I2C_IF_DIS (SPI only)
    (0x6C, 0x00), // PWR_MGMT_2: accel + gyro on
    (0x1A, 0x03), // CONFIG: gyro DLPF 41 Hz
    (0x19, 0x04), // SMPLRT_DIV: 200 Hz
    (0x1B, 0x00), // GYRO_CONFIG: +/-250 dps
    (0x1C, 0x00), // ACCEL_CONFIG: +/-2 g
];

fn settle() {
    for _ in 0..400_000u32 {
        cortex_m::asm::nop();
    }
}

// =============================== backend: native HAL ===============================
#[cfg(feature = "backend-native")]
mod backend {
    use super::*;
    use nobro_hal::{board, lease::Resource, traits::HalLease, ActivePlatform as Hal, Spim0};

    pub const BACKEND_ID: u32 = 1; // native HAL SPI driver

    pub struct Mpu9250Native {
        spim: Spim0,
    }

    pub fn mount() -> Mpu9250Native {
        Hal::acquire(Resource::Spim0, 4).ok();
        let spim = unsafe {
            Spim0::init(
                board::SPI_SCK_PIN,
                board::SPI_MOSI_PIN,
                board::SPI_MISO_PIN,
                board::SPI_CS_PIN,
            )
        };
        let _ = spim.write_reg(0x6B, 0x80); // device reset
        settle();
        for (reg, val) in MPU_INIT {
            let _ = spim.write_reg(reg, val);
        }
        settle();
        Mpu9250Native { spim }
    }

    impl ImuSal for Mpu9250Native {
        type Error = ();

        fn who_am_i(&mut self) -> Result<u8, ()> {
            self.spim.read_reg(0x75).map_err(|_| ())
        }

        fn sample(&mut self) -> Result<ImuSample, ()> {
            let mut raw = [0u8; 6];
            self.spim.read_burst(0x3B, &mut raw).map_err(|_| ())?;
            let ax = i16::from_be_bytes([raw[0], raw[1]]);
            let ay = i16::from_be_bytes([raw[2], raw[3]]);
            let az = i16::from_be_bytes([raw[4], raw[5]]);
            Ok(counts_to_sample(ax, ay, az))
        }
    }
}

// ============================= backend: embedded-hal ===============================
#[cfg(feature = "backend-eh")]
mod backend {
    use super::*;
    use embedded_hal::spi::SpiDevice as _;
    use nobro_eh_spi::NobroSpiDevice;
    use nobro_hal::{board, lease::Resource, traits::HalLease, ActivePlatform as Hal};

    pub const BACKEND_ID: u32 = 2; // embedded-hal SpiDevice driver

    pub struct Mpu9250Eh {
        dev: NobroSpiDevice,
    }

    fn wr(dev: &mut NobroSpiDevice, reg: u8, val: u8) {
        let _ = dev.write(&[reg & 0x7F, val]);
    }

    pub fn mount() -> Mpu9250Eh {
        Hal::acquire(Resource::Spim0, 4).ok();
        let mut dev = unsafe {
            NobroSpiDevice::new(
                board::SPI_SCK_PIN,
                board::SPI_MOSI_PIN,
                board::SPI_MISO_PIN,
                board::SPI_CS_PIN,
            )
        };
        wr(&mut dev, 0x6B, 0x80); // device reset
        settle();
        for (reg, val) in MPU_INIT {
            wr(&mut dev, reg, val);
        }
        settle();
        Mpu9250Eh { dev }
    }

    impl ImuSal for Mpu9250Eh {
        type Error = ();

        fn who_am_i(&mut self) -> Result<u8, ()> {
            let mut rx = [0u8; 2];
            self.dev.transfer(&mut rx, &[0x80 | 0x75, 0]).map_err(|_| ())?;
            Ok(rx[1])
        }

        fn sample(&mut self) -> Result<ImuSample, ()> {
            let mut rx = [0u8; 7];
            self.dev.transfer(&mut rx, &[0x80 | 0x3B, 0, 0, 0, 0, 0, 0]).map_err(|_| ())?;
            let ax = i16::from_be_bytes([rx[1], rx[2]]);
            let ay = i16::from_be_bytes([rx[3], rx[4]]);
            let az = i16::from_be_bytes([rx[5], rx[6]]);
            Ok(counts_to_sample(ax, ay, az))
        }
    }
}

// ========================= backend: Arduino-library shim ===========================
#[cfg(feature = "backend-arduino")]
mod backend {
    use super::*;
    use nobro_hal::{board, lease::Resource, traits::HalLease, ActivePlatform as Hal, Spim0};

    pub const BACKEND_ID: u32 = 3; // Arduino-style C++ library via the shim

    // The Arduino-style driver compiled by build.rs (bindings/cpp/arduino_shim).
    extern "C" {
        fn arduino_imu_begin() -> i32;
        fn arduino_imu_whoami() -> u8;
        fn arduino_imu_read_accel(out_counts: *mut i32);
    }

    // The shim's host callbacks: Arduino's SPI/digitalWrite/delay surface, served by
    // the same leased Spim0 the native backend uses. One SPI device per module.
    static mut SHIM_SPIM: Option<Spim0> = None;

    fn spim() -> &'static Spim0 {
        unsafe { (*core::ptr::addr_of!(SHIM_SPIM)).as_ref().unwrap() }
    }

    #[no_mangle]
    extern "C" fn nobro_shim_spi_select() {
        spim().select();
    }
    #[no_mangle]
    extern "C" fn nobro_shim_spi_deselect() {
        spim().deselect();
    }
    #[no_mangle]
    extern "C" fn nobro_shim_spi_transfer(out: u8) -> u8 {
        let mut rx = [0u8; 1];
        let _ = spim().transfer_held(&[out], &mut rx);
        rx[0]
    }
    #[no_mangle]
    extern "C" fn nobro_shim_delay_ms(ms: u32) {
        cortex_m::asm::delay(ms.saturating_mul(64_000)); // 64 MHz core
    }

    pub struct Mpu9250Arduino;

    pub fn mount() -> Mpu9250Arduino {
        Hal::acquire(Resource::Spim0, 4).ok();
        let spim = unsafe {
            Spim0::init(
                board::SPI_SCK_PIN,
                board::SPI_MOSI_PIN,
                board::SPI_MISO_PIN,
                board::SPI_CS_PIN,
            )
        };
        unsafe { *core::ptr::addr_of_mut!(SHIM_SPIM) = Some(spim) };
        let _ = unsafe { arduino_imu_begin() }; // the LIBRARY does the bring-up
        Mpu9250Arduino
    }

    impl ImuSal for Mpu9250Arduino {
        type Error = ();

        fn who_am_i(&mut self) -> Result<u8, ()> {
            Ok(unsafe { arduino_imu_whoami() })
        }

        fn sample(&mut self) -> Result<ImuSample, ()> {
            let mut counts = [0i32; 3];
            unsafe { arduino_imu_read_accel(counts.as_mut_ptr()) };
            Ok(counts_to_sample(counts[0] as i16, counts[1] as i16, counts[2] as i16))
        }
    }
}

#[cfg(any(
    all(feature = "backend-native", feature = "backend-eh"),
    all(feature = "backend-native", feature = "backend-arduino"),
    all(feature = "backend-eh", feature = "backend-arduino")
))]
compile_error!("mount exactly one IMU backend: backend-native, backend-eh, or backend-arduino");
#[cfg(not(any(feature = "backend-native", feature = "backend-eh", feature = "backend-arduino")))]
compile_error!("mount one IMU backend: backend-native, backend-eh, or backend-arduino");

// ============================ backend-agnostic evaluation ===========================

/// Everything below this line is the actual app - written against `ImuSal` only.
fn evaluate(imu: &mut impl ImuSal) -> (u32, u32, u32, u32) {
    let who = u32::from(imu.who_am_i().unwrap_or(0));
    let mut reads = 0u32;
    let mut errors = 0u32;
    let mut last_mag = 0u32;
    for _ in 0..50 {
        match imu.sample() {
            Ok(s) => {
                reads += 1;
                last_mag = s.accel_mag_mg;
            }
            Err(_) => errors += 1,
        }
        for _ in 0..80_000u32 {
            cortex_m::asm::nop();
        }
    }
    (who, last_mag, reads, errors)
}

#[entry]
fn main() -> ! {
    let mut imu = backend::mount();
    let (who, mag, reads, errors) = evaluate(&mut imu);

    // PASS: right silicon, all reads landed, magnitude ~1 g at rest (750..1250 mg).
    let ok = who == 0x71 && reads == 50 && errors == 0 && (750..=1250).contains(&mag);
    let ap = u32::from(ok);
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ backend::BACKEND_ID ^ who ^ mag ^ reads ^ errors;
    unsafe {
        NOBRO_UDI_IMU_REPORT = UdiImuReport {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            backend_id: backend::BACKEND_ID,
            who_am_i: who,
            accel_mag_mg: mag,
            reads,
            errors,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
