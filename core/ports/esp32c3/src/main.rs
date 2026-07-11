//! NobroRTOS portable core on the ESP32-C3 (M84): the same kernel control plane and
//! net/ml/crypto/power primitives that run on the nRF52840 (Cortex-M4) execute here on
//! RISC-V rv32imc, self-certifying over USB-Serial-JTAG. The collector's serial_regex
//! protocol can ingest the report line.
#![no_std]
#![no_main]

use esp_hal::delay::Delay;
use esp_println::println;

use nobro_conformance::{run_all, SUBSYSTEMS};

mod portable;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}


#[esp_hal::main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    println!("NobroRTOS ESP32-C3 port (M84) - portable core on RISC-V rv32imc");

    let results = run_all(); // the shared cross-MCU conformance suite (M92)
    let mut all = true;
    for (name, ok) in SUBSYSTEMS.iter().zip(results) {
        println!("  {}: {}", name, if ok { "PASS" } else { "FAIL" });
        all &= ok;
    }

    loop {
        println!("NOBRO-C3 arch=riscv32imc subsystems=7 all_pass={}", u32::from(all));
        delay.delay_millis(1000);
    }
}
