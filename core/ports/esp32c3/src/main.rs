//! NobroRTOS timebase provider on ESP32-C3, reporting status over USB Serial/JTAG.
#![no_std]
#![no_main]

use esp_hal::delay::Delay;
use esp_println::println;

mod portable;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[esp_hal::main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    println!("NobroRTOS ESP32-C3 port - portable core on RISC-V rv32imc");

    let timebase_ok = portable::verify_timebase_provider();
    loop {
        println!(
            "NOBRO-C3 arch=riscv32imc providers=1 timebase={} all_pass={}",
            u32::from(timebase_ok),
            u32::from(timebase_ok)
        );
        delay.delay_millis(1000);
    }
}
