//! NobroRTOS typed providers on ESP32-S3, reporting status over USB Serial/JTAG.
#![no_std]
#![no_main]

use core::{
    fmt::Write,
    ptr::addr_of_mut,
    sync::atomic::{AtomicU32, Ordering},
};

use esp_hal::{
    cpu_control::{CpuControl, Stack},
    delay::Delay,
    timer::{timg::TimerGroup, OneShotTimer},
    usb_serial_jtag::UsbSerialJtag,
};
use nobro_hal::{HalAlarm, HalClock, HalCompatibility, HardwareCapability, HardwareCapabilitySet};
use nobro_port_esp32s3::providers::{
    prepare_app_core_start, Esp32S3Alarm, Esp32S3Clock, Esp32S3Providers, Esp32S3Usb,
};

static mut APP_CORE_STACK: Stack<4096> = Stack::new();
static APP_CORE_BEATS: AtomicU32 = AtomicU32::new(0);

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
    let mut cpu_control = CpuControl::new(peripherals.CPU_CTRL);
    let recovered_stale_core = prepare_app_core_start(&mut cpu_control);
    let app_core = cpu_control
        .start_app_core(unsafe { &mut *addr_of_mut!(APP_CORE_STACK) }, || loop {
            APP_CORE_BEATS.fetch_add(1, Ordering::Release);
            for _ in 0..10_000 {
                core::hint::spin_loop();
            }
        })
        .ok();

    let started = Esp32S3Clock::now_us();
    let required = HardwareCapabilitySet::EMPTY
        .with(HardwareCapability::Timebase)
        .with(HardwareCapability::Deadline)
        .with(HardwareCapability::Usb);
    let providers_ok = Esp32S3Providers::supports(required);
    let multicore_deadline = started.saturating_add(20_000);
    while APP_CORE_BEATS.load(Ordering::Acquire) == 0 && Esp32S3Clock::now_us() < multicore_deadline
    {
    }
    let multicore_ok = app_core.is_some() && APP_CORE_BEATS.load(Ordering::Acquire) != 0;
    let armed = alarm.arm_after_us(2_000).is_ok();
    while armed && !alarm.poll_due(Esp32S3Clock::now_us()) {}
    let alarm_elapsed = Esp32S3Clock::now_us().saturating_sub(started);
    let deadline_ok = armed && (2_000..20_000).contains(&alarm_elapsed);

    let _ = writeln!(usb, "NobroRTOS ESP32-S3 portable provider check");

    let all = providers_ok && deadline_ok && multicore_ok;

    loop {
        let _ = writeln!(
            usb,
            "NOBRO-S3 arch=xtensa-lx7 contract_providers=19 exercised=4 timebase={} time_us={} alarm_us={} deadline_ok={} multicore={} core1_beats={} stale_core_recovered={} usb=1 all_pass={}",
            u32::from(providers_ok),
            Esp32S3Clock::now_us(),
            alarm_elapsed,
            u32::from(deadline_ok),
            u32::from(multicore_ok),
            APP_CORE_BEATS.load(Ordering::Acquire),
            u32::from(recovered_stale_core),
            u32::from(all)
        );
        delay.delay_millis(1000);
    }
}
