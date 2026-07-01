//! 3-class motion NN on a development board (M33): run the trained int8 idle/walk/shake
//! classifier (nn-motion-ai `Nn3MotionClassifier`) on synthetic |accel| windows for each
//! class and verify each is classified correctly. Self-certifies via NOBRO_NN3_REPORT
//! (J-Link mem32).
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_adapter_nn_motion_ai::{
    Nn3MotionClassifier, CLASS3_IDLE, CLASS3_SHAKE, CLASS3_WALK, MODEL3_ID,
};
use nobro_sal::{AiInferenceRequest, AiInferenceSal};

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    idle_class: u32,
    walk_class: u32,
    shake_class: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E4E_4E33; // "NNN3"

#[no_mangle]
#[used]
static mut NOBRO_NN3_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    idle_class: 0,
    walk_class: 0,
    shake_class: 0,
    checksum: 0,
};

fn build_window(pattern: impl Fn(usize) -> u16) -> [u8; 64] {
    let mut w = [0u8; 64];
    for i in 0..32 {
        let b = pattern(i).to_le_bytes();
        w[2 * i] = b[0];
        w[2 * i + 1] = b[1];
    }
    w
}

fn classify(clf: &mut Nn3MotionClassifier, window: &[u8]) -> u8 {
    let mut out = [0u8; 4];
    match clf.infer(AiInferenceRequest::new(MODEL3_ID, window, 2_000), &mut out) {
        Ok(_) => out[0],
        Err(_) => 0xFF,
    }
}

#[entry]
fn main() -> ! {
    let mut clf = Nn3MotionClassifier::new();

    // idle: near-still, tiny variation
    let idle = build_window(|i| if i % 2 == 0 { 1000 } else { 1004 });
    // walk: moderate periodic sway (~70 amplitude triangle)
    let walk = build_window(|i| {
        let tri = [0i32, 35, 70, 35, 0, -35, -70, -35][i % 8];
        (1000 + tri) as u16
    });
    // shake: large erratic motion (~250 amplitude)
    let shake = build_window(|i| {
        let s = [0i32, 250, -200, 220, -250, 180, -230, 240][i % 8];
        (1000 + s) as u16
    });

    let idle_class = classify(&mut clf, &idle);
    let walk_class = classify(&mut clf, &walk);
    let shake_class = classify(&mut clf, &shake);

    let pass = idle_class == CLASS3_IDLE
        && walk_class == CLASS3_WALK
        && shake_class == CLASS3_SHAKE;
    let ap = u32::from(pass);
    let (ic, wc, sc) = (
        u32::from(idle_class),
        u32::from(walk_class),
        u32::from(shake_class),
    );
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ ic ^ wc ^ sc;
    unsafe {
        NOBRO_NN3_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            idle_class: ic,
            walk_class: wc,
            shake_class: sc,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
