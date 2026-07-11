//! Embassy implementation of the baseline workload: three static tasks, a
//! bounded channel, tickless timers. GPIO stays raw-register for parity.
#![no_std]
#![no_main]

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

const GPIO_P0: u32 = 0x5000_0000;
const PIN: u32 = 15;

unsafe fn wr(a: u32, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v);
}

static CHANNEL: Channel<ThreadModeRawMutex, u32, 1> = Channel::new();

#[embassy_executor::task]
async fn control() {
    let mut ticker = Ticker::every(Duration::from_millis(20)); // 50 Hz
    let mut ticks: u32 = 0;
    loop {
        ticker.next().await;
        ticks = ticks.wrapping_add(1);
        unsafe {
            if ticks & 1 == 0 {
                wr(GPIO_P0 + 0x508, 1 << PIN);
            } else {
                wr(GPIO_P0 + 0x50C, 1 << PIN);
            }
        }
        BASELINE_REPORT[0].store(ticks, Ordering::Relaxed);
    }
}

#[embassy_executor::task]
async fn sensor() {
    let mut ticker = Ticker::every(Duration::from_millis(100)); // 10 Hz
    let mut samples: u32 = 0;
    loop {
        ticker.next().await;
        samples = samples.wrapping_add(1);
        let value = samples.wrapping_mul(3).wrapping_add(7);
        if CHANNEL.try_send(value).is_err() {
            BASELINE_REPORT[3].fetch_add(1, Ordering::Relaxed);
        }
        BASELINE_REPORT[1].store(samples, Ordering::Relaxed);
    }
}

#[embassy_executor::task]
async fn consumer() {
    let mut filtered: u32 = 0;
    loop {
        let value = CHANNEL.receive().await;
        filtered = filtered - (filtered >> 3) + (value >> 3);
        BASELINE_REPORT[2].store(filtered, Ordering::Relaxed);
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let _p = embassy_nrf::init(Default::default());
    unsafe {
        wr(GPIO_P0 + 0x700 + 4 * PIN, 1); // PIN_CNF[15] = output
    }
    spawner.spawn(control()).unwrap();
    spawner.spawn(sensor()).unwrap();
    spawner.spawn(consumer()).unwrap();
}
