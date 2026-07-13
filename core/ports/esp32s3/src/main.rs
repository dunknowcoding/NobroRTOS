//! NobroRTOS portable core on the ESP32-S3 (M85): the same kernel control plane and
//! net/ml/crypto/power primitives that run on the nRF52840 (Cortex-M4) execute here on
//! Xtensa LX7, self-certifying over USB-Serial-JTAG. The collector's serial_regex
//! protocol can ingest the report line.
#![no_std]
#![no_main]

use core::fmt::Write;

use esp_hal::{
    delay::Delay,
    timer::{timg::TimerGroup, OneShotTimer},
    usb_serial_jtag::UsbSerialJtag,
};
use nobro_hal::{HalAlarm, HalClock, HalCompatibility, HardwareCapability, HardwareCapabilitySet};
use nobro_port_esp32s3::providers::{Esp32S3Alarm, Esp32S3Clock, Esp32S3Providers, Esp32S3Usb};

use nobro_conformance::{run_all, SUBSYSTEMS};

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();
    let timers = TimerGroup::new(peripherals.TIMG0);
    let mut alarm = Esp32S3Alarm::new(OneShotTimer::new(timers.timer0));
    let mut usb = Esp32S3Usb::new(UsbSerialJtag::new(peripherals.USB_DEVICE));

    let started = Esp32S3Clock::now_us();
    let required = HardwareCapabilitySet::EMPTY
        .with(HardwareCapability::Timebase)
        .with(HardwareCapability::DeadlineTimer)
        .with(HardwareCapability::Usb);
    let providers_ok = Esp32S3Providers::supports(required);
    let armed = alarm.arm_after_us(2_000).is_ok();
    while armed && !alarm.poll_due(Esp32S3Clock::now_us()) {}
    let alarm_elapsed = Esp32S3Clock::now_us().saturating_sub(started);
    let deadline_ok = armed && (2_000..20_000).contains(&alarm_elapsed);

    let _ = writeln!(usb, "NobroRTOS ESP32-S3 portable provider check");

    let results = run_all(); // the shared cross-MCU conformance suite (M92)
    let mut all = true;
    for (name, ok) in SUBSYSTEMS.iter().zip(results) {
        let _ = writeln!(usb, "  {}: {}", name, if ok { "PASS" } else { "FAIL" });
        all &= ok;
    }
    all &= providers_ok && deadline_ok;

    loop {
        let _ = writeln!(
            usb,
            "NOBRO-S3 arch=xtensa-lx7 subsystems=7 timebase={} time_us={} alarm_us={} deadline_ok={} usb=1 all_pass={}",
            u32::from(providers_ok),
            Esp32S3Clock::now_us(),
            alarm_elapsed,
            u32::from(deadline_ok),
            u32::from(all)
        );
        delay.delay_millis(1000);
    }
}
