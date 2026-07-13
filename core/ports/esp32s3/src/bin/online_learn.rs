//! On-device incremental learning: the ESP32-S3 trains a classifier by online
//! SGD over a stream it generates itself - no host, no pre-trained weights. It measures
//! accuracy on a held-out set before training (from zero weights ~ chance), runs
//! `nobro_nn::sgd_update` over a labelled stream, then measures again, reporting:
//!   `NOBRO-LEARN before=NN after=MM all_pass=1`
//! Proof the device adapts at runtime, not just runs a frozen model.
#![no_std]
#![no_main]

use esp_hal::delay::Delay;
use esp_println::println;
use nobro_nn::{argmax, dense, sgd_update};

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

/// Tiny LCG so the on-device data stream is deterministic and heap-free.
struct Lcg(u32);
impl Lcg {
    fn next_f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        // map to [-2, 2)
        ((self.0 >> 8) as f32 / (1u32 << 24) as f32) * 4.0 - 2.0
    }
}

/// Ground truth the device must learn: class 1 iff x0 + x1 > 0 (a rotated boundary,
/// so both weights and bias must move).
fn label_of(x: &[f32; 2]) -> usize {
    usize::from(x[0] + x[1] > 0.0)
}

fn accuracy(w: &[f32], b: &[f32], rng_seed: u32) -> u32 {
    let mut rng = Lcg(rng_seed);
    let mut out = [0.0f32; 2];
    let mut hits = 0u32;
    for _ in 0..100 {
        let x = [rng.next_f32(), rng.next_f32()];
        dense(&x, w, b, &mut out);
        if argmax(&out) == label_of(&x) {
            hits += 1;
        }
    }
    hits
}

#[esp_hal::main]
fn main() -> ! {
    let _p = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    // fixed test set (seed 0xC0FFEE) vs a disjoint training stream (seed 0x1234)
    let mut w = [0.0f32; 4];
    let mut b = [0.0f32; 2];
    let mut scratch = [0.0f32; 2];

    let before = accuracy(&w, &b, 0xC0FFEE);

    // online learning: 1500 SGD steps over the device-generated stream
    let mut rng = Lcg(0x1234);
    for _ in 0..1500 {
        let x = [rng.next_f32(), rng.next_f32()];
        let y = label_of(&x);
        let _loss = sgd_update(&x, &mut w, &mut b, y, 0.05, &mut scratch);
    }

    let after = accuracy(&w, &b, 0xC0FFEE);
    let all_pass = u32::from(after >= 90 && after > before);

    loop {
        println!(
            "NOBRO-LEARN before={} after={} all_pass={}",
            before, after, all_pass
        );
        delay.delay_millis(1000);
    }
}
