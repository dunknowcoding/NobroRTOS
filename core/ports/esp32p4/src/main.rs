//! ESP32-P4-Pico native provider witness over the integrated USB Serial/JTAG
//! controller. The external CH343 UART bridge is a separate identity.
#![no_std]
#![no_main]

use core::{
    fmt::Write,
    ptr::addr_of_mut,
    sync::atomic::{AtomicU32, Ordering},
};

use esp_hal::{
    delay::Delay,
    system::{CpuControl, Stack},
    timer::{OneShotTimer, timg::TimerGroup},
    uart::{Config as UartConfig, Uart},
    usb::usb_serial_jtag::UsbSerialJtag,
};
use nobro_hal::{
    ESP32P4_PICO_IO_ROUTES, ESP32P4_PICO_MEDIA, EspIoRoute, EspP4CsiPlan, EspP4CsiSession,
    HalAlarm, HalByteIo, HalClock, HalCompatibility, HardwareCapability, HardwareCapabilitySet,
};
use nobro_port_esp32p4::providers::{
    Esp32P4Alarm, Esp32P4BridgeUart, Esp32P4Clock, Esp32P4Providers, Esp32P4Usb,
};

esp_bootloader_esp_idf::esp_app_desc!();

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
    let mut alarm = Esp32P4Alarm::new(OneShotTimer::new(timers.timer0));
    let mut usb = Esp32P4Usb::new(UsbSerialJtag::new(peripherals.USB_DEVICE));
    let mut bridge = Esp32P4BridgeUart::new(
        Uart::new(peripherals.UART0, UartConfig::default())
            .unwrap()
            .with_tx(peripherals.GPIO37)
            .with_rx(peripherals.GPIO38),
    );
    let mut cpu_control = CpuControl::new(peripherals.CPU_CTRL);
    let app_core = cpu_control
        .start_app_core(unsafe { &mut *addr_of_mut!(APP_CORE_STACK) }, || {
            loop {
                APP_CORE_BEATS.fetch_add(1, Ordering::Release);
                for _ in 0..10_000 {
                    core::hint::spin_loop();
                }
            }
        })
        .ok();

    let started = Esp32P4Clock::now_us();
    let required = HardwareCapabilitySet::EMPTY
        .with(HardwareCapability::Timebase)
        .with(HardwareCapability::Deadline)
        .with(HardwareCapability::Usb)
        .with(HardwareCapability::Multicore);
    let routes_ok = ESP32P4_PICO_IO_ROUTES
        .iter()
        .filter(|route| route.application_usb_lease().is_some())
        .count()
        == 3
        && ESP32P4_PICO_IO_ROUTES
            .iter()
            .filter(|route| route.is_debug_route())
            .count()
            == 2
        && ESP32P4_PICO_IO_ROUTES[0] == EspIoRoute::ExternalUsbUartBridge;
    let media_truthful = ESP32P4_PICO_MEDIA.board_camera_connector
        && ESP32P4_PICO_MEDIA.mipi_csi_data_lanes == 2
        && !ESP32P4_PICO_MEDIA.native_csi_driver;
    let csi_plan = EspP4CsiPlan::new(ESP32P4_PICO_MEDIA, 0, 2, 640, 480, 2).unwrap();
    let csi_session = EspP4CsiSession::try_acquire(csi_plan, 0xC5).unwrap();
    let csi_contract_ok = csi_session.ensure_live().is_ok() && csi_session.plan() == csi_plan;
    let usb_stack_ok =
        nobro_usb::capabilities().backend_id == nobro_usb::backend_id::USB_SERIAL_JTAG;
    let providers_ok = Esp32P4Providers::supports(required)
        && routes_ok
        && media_truthful
        && csi_contract_ok
        && usb_stack_ok;
    let multicore_deadline = started.saturating_add(20_000);
    while APP_CORE_BEATS.load(Ordering::Acquire) == 0 && Esp32P4Clock::now_us() < multicore_deadline
    {
        core::hint::spin_loop();
    }
    let multicore_ok = app_core.is_some() && APP_CORE_BEATS.load(Ordering::Acquire) != 0;
    let armed = alarm.arm_after_us(2_000).is_ok();
    while armed && !alarm.poll_due(Esp32P4Clock::now_us()) {
        core::hint::spin_loop();
    }
    let elapsed = Esp32P4Clock::now_us().saturating_sub(started);
    let deadline_ok = armed && (2_000..20_000).contains(&elapsed);

    loop {
        let _ = writeln!(
            usb,
            "NOBRO-P4 hp_cores=2 lp_core_separate=1 usb_app_routes=3 debug_routes=2 ch343_external=1 csi_lanes=2 csi_native_driver=0 time={} deadline={} multicore={} core1_beats={} all_pass={}",
            Esp32P4Clock::now_us(),
            u32::from(deadline_ok),
            u32::from(multicore_ok),
            APP_CORE_BEATS.load(Ordering::Acquire),
            u32::from(providers_ok && deadline_ok && multicore_ok)
        );
        let _ = usb.flush();
        let _ = writeln!(
            bridge,
            "NOBRO-P4-BRIDGE time={} controller=external-uart",
            Esp32P4Clock::now_us()
        );
        let _ = bridge.flush();
        delay.delay_millis(1_000);
    }
}
