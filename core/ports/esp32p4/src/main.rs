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
    system::{Cpu, CpuControl, Stack},
    timer::{OneShotTimer, timg::TimerGroup},
    uart::{Config as UartConfig, Uart},
    usb::usb_serial_jtag::UsbSerialJtag,
};
use nobro_hal::{
    ESP32P4_PICO_IO_ROUTES, ESP32P4_PICO_MEDIA, EspIoRoute, EspP4CsiPlan, EspP4CsiSession,
    HalAlarm, HalByteIo, HalClock, HalCompatibility, HardwareCapability, HardwareCapabilitySet,
};
use nobro_kernel::{ModuleId, MulticoreExecutorLifecycle};
use nobro_port_esp32p4::providers::{
    Esp32P4Alarm, Esp32P4BridgeUart, Esp32P4Clock, Esp32P4Providers, Esp32P4Usb,
};

#[path = "../../esp_multicore_campaign.rs"]
mod multicore_campaign;

esp_bootloader_esp_idf::esp_app_desc!();

static mut APP_CORE_STACK: Stack<4096> = Stack::new();
static APP_CORE_READY: AtomicU32 = AtomicU32::new(0);
static APP_CORE_WAKE: AtomicU32 = AtomicU32::new(0);

fn wake_app_core() {
    APP_CORE_WAKE.fetch_add(1, Ordering::Release);
}

fn app_core_task() {
    // Revision-v1 ESP32-P4 silicon has no CLIC mintthresh CSR (0x347), while
    // the current upstream interrupt dispatcher reads it. Use bounded coherent
    // polling on this exact port instead of presenting a trapping IPI as usable.
    let wake = APP_CORE_WAKE.load(Ordering::Acquire);
    APP_CORE_READY.store(2, Ordering::Release);
    while APP_CORE_WAKE.load(Ordering::Acquire) == wake {
        core::hint::spin_loop();
    }
    APP_CORE_READY.store(3, Ordering::Release);
    multicore_campaign::secondary_core(core::hint::spin_loop);
}

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
    let mut bridge = Esp32P4BridgeUart::new(
        Uart::new(peripherals.UART0, UartConfig::default())
            .unwrap()
            .with_tx(peripherals.GPIO37)
            .with_rx(peripherals.GPIO38),
    );
    // USB Serial/JTAG owns the descriptors. The request is retained only for the
    // common mount receipt and is not advertised by the controller.
    let usb_cfg = nobro_usb::UsbConfig::controller_owned();
    let usb_controller = UsbSerialJtag::new(peripherals.USB_DEVICE);
    let mut usb = match nobro_usb::try_mount(&usb_cfg) {
        Ok(usb) => Esp32P4Usb::new(usb_controller, usb),
        Err(_) => loop {
            core::hint::spin_loop();
        },
    };
    let mut cpu_control = CpuControl::new(peripherals.CPU_CTRL);
    let mut lifecycle = MulticoreExecutorLifecycle::<2, 2>::new();
    lifecycle.place(0, ModuleId::Kernel, 1_000).unwrap();
    lifecycle.place(1, ModuleId::App(1), 4_000).unwrap();
    let mut app_core = None;
    let multicore_started = lifecycle
        .start_all(
            |core| {
                if core == 0 {
                    true
                } else {
                    app_core = cpu_control
                        .start_app_core(
                            unsafe { &mut *addr_of_mut!(APP_CORE_STACK) },
                            app_core_task,
                        )
                        .ok();
                    app_core.is_some()
                }
            },
            |_| {},
        )
        .is_ok();

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
        nobro_usb::capabilities().backend_id == nobro_usb::backend_id::USB_SERIAL_JTAG_ESP32P4;
    let providers_ok = Esp32P4Providers::supports(required)
        && routes_ok
        && media_truthful
        && csi_contract_ok
        && usb_stack_ok;
    let multicore_deadline = Esp32P4Clock::now_us().saturating_add(100_000);
    while APP_CORE_READY.load(Ordering::Acquire) < 2 && Esp32P4Clock::now_us() < multicore_deadline
    {
        core::hint::spin_loop();
    }
    if multicore_started && APP_CORE_READY.load(Ordering::Acquire) == 2 {
        wake_app_core();
    }
    while APP_CORE_READY.load(Ordering::Acquire) < 3 && Esp32P4Clock::now_us() < multicore_deadline
    {
        core::hint::spin_loop();
    }
    let first_generation = lifecycle.generation(1).map(|generation| generation.get());
    let first_ready = multicore_started
        && APP_CORE_READY.load(Ordering::Acquire) == 3
        && first_generation.is_some_and(multicore_campaign::begin_generation);
    if first_ready {
        wake_app_core();
    }
    let pre = first_ready
        .then(|| {
            multicore_campaign::run_until_wedge(&mut lifecycle, Esp32P4Clock::now_us, wake_app_core)
        })
        .flatten();
    let mut campaign = multicore_campaign::CampaignReport::default();
    if let Some(pre) = pre {
        let faulted = lifecycle.fault(1).is_ok();
        // SAFETY: this runs on core 0 and parks only the admitted app core.
        unsafe { cpu_control.park_core(Cpu::AppCpu) };
        let replacement_generation = lifecycle
            .generation(1)
            .and_then(|generation| generation.get().checked_add(1))
            .unwrap_or(0);
        let plane_prepared = multicore_campaign::begin_generation(replacement_generation);
        let recovered = faulted
            && plane_prepared
            && lifecycle
                .recover(1, |_| {
                    cpu_control.unpark_core(Cpu::AppCpu);
                    true
                })
                .is_ok();
        if recovered {
            campaign = multicore_campaign::finish_recovery(
                pre,
                replacement_generation,
                Esp32P4Clock::now_us,
                wake_app_core,
            );
        }
    }
    let multicore_ok = campaign.passed && lifecycle.all_up() && app_core.is_some();
    let alarm_started = Esp32P4Clock::now_us();
    let armed = alarm.arm_after_us(2_000).is_ok();
    while armed && !alarm.poll_due(Esp32P4Clock::now_us()) {
        core::hint::spin_loop();
    }
    let elapsed = Esp32P4Clock::now_us().saturating_sub(alarm_started);
    let deadline_ok = armed && (2_000..20_000).contains(&elapsed);

    loop {
        // The external bridge remains observable regardless of native USB state.
        // Service the bounded native endpoint only after publishing this heartbeat.
        let _ = writeln!(
            bridge,
            "NOBRO-P4-BRIDGE multicore={} accepted={} rejected={} processed={} expected={} actual={} cancelled={} stale={} fallback={} restart={} all_pass={}",
            u32::from(multicore_ok),
            campaign.accepted,
            campaign.rejected,
            campaign.processed,
            campaign.expected,
            campaign.actual,
            campaign.cancelled,
            campaign.stale,
            campaign.fallback,
            campaign.restart,
            u32::from(providers_ok && deadline_ok && multicore_ok),
        );
        let _ = bridge.flush();
        // Keep the initialized controller and bounded stack owned for this session.
        // Data-plane servicing starts only after physical enumeration is established.
        let _ = &mut usb;
        let _ = &app_core;
        delay.delay_millis(1_000);
    }
}
