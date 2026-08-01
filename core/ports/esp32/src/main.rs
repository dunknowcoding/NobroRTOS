//! Classic ESP32 native provider witness. Output is UART0 through the board's
//! external USB-to-UART bridge; the MCU does not claim native USB.
#![no_std]
#![no_main]
#![feature(asm_experimental_arch)]

use core::{
    fmt::Write,
    ptr::addr_of_mut,
    sync::atomic::{AtomicU32, Ordering},
};

use esp_hal::{
    cpu_control::{CpuControl, Stack},
    delay::Delay,
    timer::{timg::TimerGroup, OneShotTimer},
    uart::{Config as UartConfig, Uart},
};
use nobro_hal::{
    EspIoRoute, HalAlarm, HalByteIo, HalClock, HalCompatibility, HardwareCapability,
    HardwareCapabilitySet, ESP32_BOARD_IO_ROUTES,
};
use nobro_port_esp32::providers::{Esp32Alarm, Esp32BridgeUart, Esp32Clock, Esp32Providers};

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
    let mut alarm = Esp32Alarm::new(OneShotTimer::new(timers.timer0));
    let mut bridge = Esp32BridgeUart::new(
        Uart::new(peripherals.UART0, UartConfig::default())
            .unwrap()
            .with_tx(peripherals.GPIO1)
            .with_rx(peripherals.GPIO3),
    );
    let mut cpu_control = CpuControl::new(peripherals.CPU_CTRL);
    let app_core = cpu_control
        .start_app_core(unsafe { &mut *addr_of_mut!(APP_CORE_STACK) }, || loop {
            APP_CORE_BEATS.fetch_add(1, Ordering::Release);
            for _ in 0..10_000 {
                core::hint::spin_loop();
            }
        })
        .ok();

    let started = Esp32Clock::now_us();
    let required = HardwareCapabilitySet::EMPTY
        .with(HardwareCapability::Timebase)
        .with(HardwareCapability::Deadline)
        .with(HardwareCapability::Multicore);
    let route_ok = ESP32_BOARD_IO_ROUTES == [EspIoRoute::ExternalUsbUartBridge]
        && ESP32_BOARD_IO_ROUTES[0].application_usb_lease().is_none();
    let providers_ok = Esp32Providers::supports(required) && route_ok;
    let multicore_deadline = started.saturating_add(20_000);
    while APP_CORE_BEATS.load(Ordering::Acquire) == 0 && Esp32Clock::now_us() < multicore_deadline {
        core::hint::spin_loop();
    }
    let multicore_ok = app_core.is_some() && APP_CORE_BEATS.load(Ordering::Acquire) != 0;
    let armed = alarm.arm_after_us(2_000).is_ok();
    while armed && !alarm.poll_due(Esp32Clock::now_us()) {
        core::hint::spin_loop();
    }
    let elapsed = Esp32Clock::now_us().saturating_sub(started);
    let deadline_ok = armed && (2_000..20_000).contains(&elapsed);

    loop {
        let _ = writeln!(
            bridge,
            "NOBRO-ESP32 uart_bridge=1 native_usb=0 time={} deadline={} multicore={} core1_beats={} all_pass={}",
            Esp32Clock::now_us(),
            u32::from(deadline_ok),
            u32::from(multicore_ok),
            APP_CORE_BEATS.load(Ordering::Acquire),
            u32::from(providers_ok && deadline_ok && multicore_ok)
        );
        let _ = bridge.flush();
        delay.delay_millis(1_000);
    }
}
