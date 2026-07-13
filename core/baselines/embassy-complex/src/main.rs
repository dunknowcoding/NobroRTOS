//! Embassy implementation of the Wave-59 five-stage workload.
#![no_std]
#![no_main]
#![cfg_attr(feature = "nightly-static", feature(impl_trait_in_assoc_type))]

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Ticker};
use panic_halt as _;

#[no_mangle]
#[used]
static BASELINE_REPORT: [AtomicU32; 4] = [
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
    AtomicU32::new(0),
];

// BENCH_INSTRUMENTATION_BEGIN
#[cfg(feature = "runtime-trace")]
#[no_mangle]
#[used]
static RUNTIME_BUSY_CYCLES: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "runtime-trace")]
static TRACE_START: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "runtime-trace")]
#[no_mangle]
#[used]
static RUNTIME_LATENCY: [AtomicU32; 7] = [const { AtomicU32::new(0) }; 7];
#[cfg(feature = "runtime-trace")]
#[no_mangle]
#[used]
static RUNTIME_IDLE_CYCLES: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "runtime-trace")]
#[no_mangle]
#[used]
static RUNTIME_IDLE_ENTRIES: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "runtime-trace")]
static IDLE_START: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "runtime-trace")]
static IDLE_ACTIVE: AtomicU32 = AtomicU32::new(0);

#[cfg(feature = "runtime-trace")]
fn runtime_cycles() -> u32 {
    unsafe { core::ptr::read_volatile(0xE000_1004 as *const u32) }
}

#[cfg(feature = "runtime-trace")]
fn timing_init() {
    unsafe {
        wr(0x4000_8000 + 0x504, 0);
        wr(0x4000_8000 + 0x508, 3);
        wr(0x4000_8000 + 0x510, 4);
        wr(0x4000_8000, 1);
    }
}

#[cfg(feature = "runtime-trace")]
fn timing_micros() -> u32 {
    unsafe {
        wr(0x4000_8000 + 0x040, 1);
        core::ptr::read_volatile((0x4000_8000 + 0x540) as *const u32)
    }
}

#[cfg(feature = "runtime-trace")]
#[no_mangle]
fn _embassy_trace_task_exec_begin(_executor: u32, _task: u32) {
    let now = runtime_cycles();
    if IDLE_ACTIVE.swap(0, Ordering::Relaxed) != 0 {
        RUNTIME_IDLE_CYCLES.fetch_add(
            now.wrapping_sub(IDLE_START.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
    }
    TRACE_START.store(now, Ordering::Relaxed);
}

#[cfg(feature = "runtime-trace")]
#[no_mangle]
fn _embassy_trace_task_exec_end(_executor: u32, _task: u32) {
    let elapsed = runtime_cycles().wrapping_sub(TRACE_START.load(Ordering::Relaxed));
    RUNTIME_BUSY_CYCLES.fetch_add(elapsed, Ordering::Relaxed);
}

#[cfg(feature = "runtime-trace")]
#[no_mangle]
fn _embassy_trace_task_new(_executor: u32, _task: u32) {}

#[cfg(feature = "runtime-trace")]
#[no_mangle]
fn _embassy_trace_task_ready_begin(_executor: u32, _task: u32) {}

#[cfg(feature = "runtime-trace")]
#[no_mangle]
fn _embassy_trace_executor_idle(_executor: u32) {
    IDLE_START.store(runtime_cycles(), Ordering::Relaxed);
    IDLE_ACTIVE.store(1, Ordering::Relaxed);
    RUNTIME_IDLE_ENTRIES.fetch_add(1, Ordering::Relaxed);
}

#[cfg(feature = "runtime-trace")]
fn record_jitter(last_us: &mut u32, period_us: u32) {
    let now = timing_micros();
    let jitter = if *last_us == 0 {
        0
    } else {
        now.wrapping_sub(*last_us)
            .abs_diff(period_us)
            .saturating_mul(64)
    };
    *last_us = now;
    RUNTIME_LATENCY[0].fetch_add(1, Ordering::Relaxed);
    RUNTIME_LATENCY[1].fetch_max(jitter, Ordering::Relaxed);
    RUNTIME_LATENCY[2].fetch_add(jitter, Ordering::Relaxed);
    let bucket = if jitter <= 64 {
        3
    } else if jitter <= 640 {
        4
    } else if jitter <= 6_400 {
        5
    } else {
        6
    };
    RUNTIME_LATENCY[bucket].fetch_add(1, Ordering::Relaxed);
}
// BENCH_INSTRUMENTATION_END

const GPIO_P0: u32 = 0x5000_0000;
const PIN: u32 = 15;

unsafe fn wr(address: u32, value: u32) {
    core::ptr::write_volatile(address as *mut u32, value);
}

static FUSION_OUT: Channel<ThreadModeRawMutex, u32, 1> = Channel::new();
static RADIO_IN: Channel<ThreadModeRawMutex, u32, 1> = Channel::new();
static STORAGE_IN: Channel<ThreadModeRawMutex, u32, 1> = Channel::new();

#[embassy_executor::task]
async fn fusion() {
    let mut ticker = Ticker::every(Duration::from_millis(10));
    let mut samples = 0u32;
    let mut fused = 0u32;
    // BENCH_INSTRUMENTATION_BEGIN
    #[cfg(feature = "runtime-trace")]
    let mut expected = 0u32;
    // BENCH_INSTRUMENTATION_END
    loop {
        ticker.next().await;
        // BENCH_INSTRUMENTATION_BEGIN
        #[cfg(feature = "runtime-trace")]
        record_jitter(&mut expected, 10_000);
        // BENCH_INSTRUMENTATION_END
        samples = samples.wrapping_add(1);
        let a = samples.wrapping_mul(3).wrapping_add(7);
        let b = samples.wrapping_mul(5).wrapping_add(11);
        fused = fused - (fused >> 3) + ((a ^ b) >> 3);
        if FUSION_OUT.try_send(fused).is_err() {
            BASELINE_REPORT[3].fetch_add(1, Ordering::Relaxed);
        }
        BASELINE_REPORT[1].store(samples, Ordering::Relaxed);
    }
}

#[embassy_executor::task]
async fn control() {
    let mut ticker = Ticker::every(Duration::from_millis(20));
    let mut ticks = 0u32;
    // BENCH_INSTRUMENTATION_BEGIN
    #[cfg(feature = "runtime-trace")]
    let mut expected = 0u32;
    // BENCH_INSTRUMENTATION_END
    loop {
        ticker.next().await;
        // BENCH_INSTRUMENTATION_BEGIN
        #[cfg(feature = "runtime-trace")]
        record_jitter(&mut expected, 20_000);
        // BENCH_INSTRUMENTATION_END
        ticks = ticks.wrapping_add(1);
        unsafe {
            if ticks & 1 == 0 {
                wr(GPIO_P0 + 0x508, 1 << PIN);
            } else {
                wr(GPIO_P0 + 0x50C, 1 << PIN);
            }
        }
        if let Ok(command) = FUSION_OUT.try_receive() {
            if RADIO_IN.try_send(command).is_err() {
                BASELINE_REPORT[3].fetch_add(1, Ordering::Relaxed);
            }
            if STORAGE_IN.try_send(command).is_err() {
                BASELINE_REPORT[3].fetch_add(1, Ordering::Relaxed);
            }
        }
        BASELINE_REPORT[0].store(ticks, Ordering::Relaxed);
    }
}

#[embassy_executor::task]
async fn radio() {
    let mut ticker = Ticker::every(Duration::from_millis(50));
    let mut transmitted = 0u32;
    // BENCH_INSTRUMENTATION_BEGIN
    #[cfg(feature = "runtime-trace")]
    let mut expected = 0u32;
    // BENCH_INSTRUMENTATION_END
    loop {
        ticker.next().await;
        // BENCH_INSTRUMENTATION_BEGIN
        #[cfg(feature = "runtime-trace")]
        record_jitter(&mut expected, 50_000);
        // BENCH_INSTRUMENTATION_END
        if RADIO_IN.try_receive().is_ok() {
            transmitted = transmitted.wrapping_add(1);
            BASELINE_REPORT[2].store(transmitted, Ordering::Relaxed);
        }
    }
}

#[embassy_executor::task]
async fn storage() {
    let mut ticker = Ticker::every(Duration::from_millis(100));
    let mut ring = [0u32; 8];
    let mut head = 0usize;
    loop {
        ticker.next().await;
        if let Ok(value) = STORAGE_IN.try_receive() {
            ring[head] = value;
            head = (head + 1) % ring.len();
            core::hint::black_box(&ring);
        }
    }
}

#[embassy_executor::task]
async fn diagnostics() {
    let mut ticker = Ticker::every(Duration::from_millis(200));
    loop {
        ticker.next().await;
        core::hint::black_box(BASELINE_REPORT[3].load(Ordering::Relaxed));
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let _peripherals = embassy_nrf::init(Default::default());
    // BENCH_INSTRUMENTATION_BEGIN
    #[cfg(feature = "runtime-trace")]
    timing_init();
    // BENCH_INSTRUMENTATION_END
    unsafe {
        wr(GPIO_P0 + 0x700 + 4 * PIN, 1);
    }
    spawner.spawn(fusion()).unwrap();
    spawner.spawn(control()).unwrap();
    spawner.spawn(radio()).unwrap();
    spawner.spawn(storage()).unwrap();
    spawner.spawn(diagnostics()).unwrap();
}
