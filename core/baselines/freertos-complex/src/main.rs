//! FreeRTOS implementation of the Wave-59 five-stage workload.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

unsafe extern "C" {
    fn freertos_complex_start() -> !;
}

#[entry]
fn main() -> ! {
    unsafe { freertos_complex_start() }
}
