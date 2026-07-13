//! AI + hardware + resource management: run the trained int8 MLP
//! (nn-motion-ai) on synthetic idle/active windows and on the LIVE SPI MPU-9250, and
//! admit the model through the kernel's preflight_ai_invocation (enforcing its
//! arena/timeout/RAM budget). Proves NobroRTOS runs a real on-device neural network
//! against live hardware while managing the AI's resource contract. Self-certifies via
//! NOBRO_NN_REPORT (J-Link mem32).
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use embedded_hal::spi::SpiDevice as _;
use panic_halt as _;

use nobro_adapter_nn_motion_ai::{
    NnMotionClassifier, CLASS_ACTIVE, CLASS_IDLE, MODEL_ID, TRAIN_ACC_MILLI,
};
use nobro_eh_spi::NobroSpiDevice;
use nobro_hal::{
    board,
    lease::Resource,
    traits::{HalLease, HalTimebaseProvider},
    ActivePlatform as Hal,
};
use nobro_sal::{
    preflight_ai_invocation, AiInferenceRequest, AiInferenceSal, AiInvocationLimits, AiRoutePolicy,
    AiRoutePreference, AiRuntimeState,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct NnReport {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    who_am_i: u32,
    nn_idle_class: u32,
    nn_active_class: u32,
    nn_live_class: u32,
    nn_live_conf: u32,
    train_acc_milli: u32,
    preflight_pass: u32,
    required_ram: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E4E_4E31; // "NNN1"

#[no_mangle]
#[used]
static mut NOBRO_NN_REPORT: NnReport = NnReport {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    who_am_i: 0,
    nn_idle_class: 0xFF,
    nn_active_class: 0xFF,
    nn_live_class: 0xFF,
    nn_live_conf: 0,
    train_acc_milli: 0,
    preflight_pass: 0,
    required_ram: 0,
    checksum: 0,
};

fn rd(dev: &mut NobroSpiDevice, reg: u8) -> u8 {
    let mut rx = [0u8; 2];
    let _ = dev.transfer(&mut rx, &[0x80 | reg, 0]);
    rx[1]
}
fn wr(dev: &mut NobroSpiDevice, reg: u8, val: u8) {
    dev.write(&[reg & 0x7F, val])
        .unwrap_or_else(|_| defmt::panic!("SPI write"));
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

fn classify(clf: &mut NnMotionClassifier, window: &[u8]) -> (u8, u16) {
    let mut out = [0u8; 4];
    match clf.infer(AiInferenceRequest::new(MODEL_ID, window, 2_000), &mut out) {
        Ok(r) => (out[0], r.confidence_q15),
        Err(_) => (0xFF, 0),
    }
}

const OWNER_SPI: u8 = 4;

#[entry]
fn main() -> ! {
    Hal::acquire(Resource::Timer0, 2).unwrap_or_else(|_| defmt::panic!("timer lease"));
    unsafe {
        Hal::init_timebase();
    }
    let mut dev = unsafe {
        NobroSpiDevice::new(
            OWNER_SPI,
            board::SPI_SCK_PIN,
            board::SPI_MOSI_PIN,
            board::SPI_MISO_PIN,
            board::SPI_CS_PIN,
        )
        .unwrap_or_else(|_| defmt::panic!("SPI session"))
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
    let who_am_i = u32::from(rd(&mut dev, 0x75));

    let mut clf = NnMotionClassifier::new();

    // Synthetic windows: idle (low variance ~1 g) and active (large oscillation).
    let mut idle = [0u8; 64];
    let mut active = [0u8; 64];
    for i in 0..32usize {
        let iv = (1000i32 + (i as i32 * 7) % 17 - 8) as u16;
        let av = (1000i32 + if i % 2 == 0 { -150 } else { 150 } + (i as i32 * 13) % 40 - 20) as u16;
        idle[2 * i..2 * i + 2].copy_from_slice(&iv.to_le_bytes());
        active[2 * i..2 * i + 2].copy_from_slice(&av.to_le_bytes());
    }
    let (idle_class, _) = classify(&mut clf, &idle);
    let (active_class, _) = classify(&mut clf, &active);

    // Live: a window of accel-magnitude samples straight from the SPI IMU.
    let mut live = [0u8; 64];
    for i in 0..32usize {
        let ax = i16::from_be_bytes([rd(&mut dev, 0x3B), rd(&mut dev, 0x3C)]);
        let ay = i16::from_be_bytes([rd(&mut dev, 0x3D), rd(&mut dev, 0x3E)]);
        let az = i16::from_be_bytes([rd(&mut dev, 0x3F), rd(&mut dev, 0x40)]);
        let sq = (i64::from(ax) * i64::from(ax)
            + i64::from(ay) * i64::from(ay)
            + i64::from(az) * i64::from(az)) as u64;
        let mg = (isqrt(sq) * 1000 / 16384) as u16;
        live[2 * i..2 * i + 2].copy_from_slice(&mg.to_le_bytes());
    }
    let (live_class, live_conf) = classify(&mut clf, &live);

    // Kernel resource management: admit the model's AI contract within a RAM/time budget.
    let contract = clf.contract();
    let policy = AiRoutePolicy::new(AiRoutePreference::LocalOnly, 50_000, 2);
    let state = AiRuntimeState::new(true, false, 1_000, 0);
    let limits = AiInvocationLimits::new(4, 64, 8 * 1024, 2_500);
    let pf = preflight_ai_invocation(
        contract,
        policy,
        state,
        AiInferenceRequest::new(MODEL_ID, &idle, 2_000),
        limits,
    );
    let preflight_pass = u32::from(pf.passing());
    let required_ram = pf.required_ram_bytes;

    let who_ok = matches!(who_am_i, 0x70 | 0x71 | 0x73);
    let pass = who_ok
        && idle_class == CLASS_IDLE
        && active_class == CLASS_ACTIVE
        && preflight_pass == 1
        && TRAIN_ACC_MILLI >= 950;
    let ap = u32::from(pass);

    let cs = MAGIC
        ^ 1
        ^ 1
        ^ ap
        ^ who_am_i
        ^ u32::from(idle_class)
        ^ u32::from(active_class)
        ^ u32::from(live_class)
        ^ u32::from(live_conf)
        ^ TRAIN_ACC_MILLI
        ^ preflight_pass
        ^ required_ram;
    unsafe {
        NOBRO_NN_REPORT = NnReport {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            who_am_i,
            nn_idle_class: u32::from(idle_class),
            nn_active_class: u32::from(active_class),
            nn_live_class: u32::from(live_class),
            nn_live_conf: u32::from(live_conf),
            train_acc_milli: TRAIN_ACC_MILLI,
            preflight_pass,
            required_ram,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
