//! On-device vision-model inference latency (M109, latency half).
//!
//! Times the face/person-presence model's core compute - a 256->2 int8 dense layer, the
//! same shape M100/M107 deploy - using our own nobro_nn::dense_int8 kernel on the
//! ESP32-S3. Reports microseconds per inference and inferences/second, measured with the
//! S3's hardware timer over many runs. (The power-budget half of M109 needs an INA3221 on
//! the model board's rail, which the bench INA does not sit on.)
#![no_std]
#![no_main]

use esp_hal::delay::Delay;
use esp_hal::time::now;
use esp_println::println;
use nobro_nn::dense_int8;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

const IN: usize = 256; // 16x16 vision feature vector (M100/M107 face model)
const OUT: usize = 2;
const RUNS: u32 = 2000;

#[esp_hal::main]
fn main() -> ! {
    let _p = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    // Representative int8 weights/input (a fixed pseudo-pattern; latency is data-independent
    // for a dense layer, so exact values don't matter - the MAC count does).
    static mut W: [i8; IN * OUT] = [0; IN * OUT];
    let w = unsafe { &mut *core::ptr::addr_of_mut!(W) };
    for (i, v) in w.iter_mut().enumerate() {
        *v = ((i as i32 % 63) - 31) as i8;
    }
    let mut x = [0i8; IN];
    for (i, v) in x.iter_mut().enumerate() {
        *v = ((i as i32 % 40) - 20) as i8;
    }
    let bias = [0i32; OUT];
    let mut out = [0i32; OUT];

    // warm up, then time RUNS inferences with the hardware timer
    dense_int8(&x, w, &bias, &mut out);
    let t0 = now();
    for k in 0..RUNS {
        x[0] = (k & 0x7F) as i8; // vary input so nothing is optimized away
        dense_int8(&x, w, &bias, &mut out);
    }
    let t1 = now();
    let total_us = (t1 - t0).to_micros();
    let per_inf_ns = (total_us * 1000) / RUNS as u64;
    let per_sec = if per_inf_ns > 0 { 1_000_000_000u64 / per_inf_ns } else { 0 };
    let all_pass = u32::from(per_inf_ns > 0 && out[0] != out[1]);

    loop {
        println!(
            "NOBRO-NNLAT model=256x2-int8 runs={} us_total={} ns_per_inf={} inf_per_s={} all_pass={}",
            RUNS, total_us, per_inf_ns, per_sec, all_pass
        );
        delay.delay_millis(1000);
    }
}
