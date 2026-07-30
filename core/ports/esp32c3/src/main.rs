//! NobroRTOS timebase provider on ESP32-C3, reporting status over USB Serial/JTAG.
#![no_std]
#![no_main]

use core::fmt::Write;

use esp_hal::{
    delay::Delay,
    timer::{timg::TimerGroup, OneShotTimer},
    usb_serial_jtag::UsbSerialJtag,
};
use nobro_hal::{HalAlarm, HalClock};
use nobro_port_esp32c3::portable::{self, Esp32C3, Esp32C3Alarm, Esp32C3Usb};

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();
    let timers = TimerGroup::new(peripherals.TIMG0);
    let mut alarm = Esp32C3Alarm::new(OneShotTimer::new(timers.timer0));
    let mut usb = Esp32C3Usb::new(UsbSerialJtag::new(peripherals.USB_DEVICE));

    let timebase_ok = portable::verify_timebase_provider();
    let started = Esp32C3::now_us();
    let armed = alarm.arm_after_us(2_000).is_ok();
    while armed && !alarm.poll_due(Esp32C3::now_us()) {}
    let alarm_elapsed = Esp32C3::now_us().saturating_sub(started);
    let deadline_ok = armed && (2_000..20_000).contains(&alarm_elapsed);
    let all = timebase_ok && deadline_ok;

    let _ = writeln!(usb, "NobroRTOS ESP32-C3 deep provider check");
    loop {
        let _ = writeln!(
            usb,
            "NOBRO-C3 arch=riscv32imc contract_providers=18 exercised=3 timebase={} alarm_us={} deadline_ok={} usb=1 all_pass={}",
            u32::from(timebase_ok),
            alarm_elapsed,
            u32::from(deadline_ok),
            u32::from(all)
        );
        delay.delay_millis(1000);
    }
}
