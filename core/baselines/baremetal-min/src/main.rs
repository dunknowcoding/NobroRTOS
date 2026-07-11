//! Bare-metal floor for the baseline workload: no framework, hand-rolled loop.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

#[no_mangle]
#[used]
static mut BASELINE_REPORT: [u32; 4] = [0; 4];

const TIMER0: u32 = 0x4000_8000;
const GPIO_P0: u32 = 0x5000_0000;
const PIN: u32 = 15;

unsafe fn wr(a: u32, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v);
}
unsafe fn rd(a: u32) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}

/// Free-running 1 MHz microsecond counter on TIMER0.
fn timer_init() {
    unsafe {
        wr(TIMER0 + 0x504, 0); // MODE = Timer
        wr(TIMER0 + 0x508, 3); // BITMODE = 32bit
        wr(TIMER0 + 0x510, 4); // PRESCALER = 4 (16MHz/16 = 1MHz)
        wr(TIMER0 + 0x000, 1); // TASKS_START
    }
}

fn micros() -> u32 {
    unsafe {
        wr(TIMER0 + 0x040, 1); // TASKS_CAPTURE[0]
        rd(TIMER0 + 0x540) // CC[0]
    }
}

#[entry]
fn main() -> ! {
    unsafe {
        wr(GPIO_P0 + 0x700 + 4 * PIN, 1); // PIN_CNF[15] = output
    }
    timer_init();

    let mut control_ticks: u32 = 0;
    let mut samples: u32 = 0;
    let mut filtered: u32 = 0;
    let mut drops: u32 = 0;
    // One-slot "channel", hand-rolled.
    let mut slot: Option<u32> = None;

    let mut next_control = 0u32;
    let mut next_sensor = 0u32;
    loop {
        let now = micros();
        if now.wrapping_sub(next_control) < 0x8000_0000 {
            next_control = next_control.wrapping_add(20_000); // 50 Hz
            control_ticks = control_ticks.wrapping_add(1);
            unsafe {
                if control_ticks & 1 == 0 {
                    wr(GPIO_P0 + 0x508, 1 << PIN); // OUTSET
                } else {
                    wr(GPIO_P0 + 0x50C, 1 << PIN); // OUTCLR
                }
            }
        }
        if now.wrapping_sub(next_sensor) < 0x8000_0000 {
            next_sensor = next_sensor.wrapping_add(100_000); // 10 Hz
            samples = samples.wrapping_add(1);
            let value = samples.wrapping_mul(3).wrapping_add(7);
            if slot.replace(value).is_some() {
                drops = drops.wrapping_add(1);
            }
        }
        // consumer: EMA filter (alpha = 1/8)
        if let Some(value) = slot.take() {
            filtered = filtered - (filtered >> 3) + (value >> 3);
        }
        unsafe {
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!(BASELINE_REPORT),
                [control_ticks, samples, filtered, drops],
            );
        }
    }
}
