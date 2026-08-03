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
    interrupt::{Priority, software::SoftwareInterrupt},
    timer::{OneShotTimer, timg::TimerGroup},
    uart::{Config as UartConfig, Uart},
};
use nobro_hal::{
    ESP32_BOARD_IO_ROUTES, EspIoRoute, HalAlarm, HalByteIo, HalClock, HalCompatibility,
    HardwareCapability, HardwareCapabilitySet,
};
use nobro_kernel::{ModuleId, MulticoreExecutorLifecycle};
use nobro_port_esp32::providers::prepare_app_core_start;
use nobro_port_esp32::providers::{Esp32Alarm, Esp32BridgeUart, Esp32Clock, Esp32Providers};

#[path = "../../esp_multicore_campaign.rs"]
mod multicore_campaign;

static mut APP_CORE_STACK: Stack<4096> = Stack::new();
static APP_CORE_IPI_READY: AtomicU32 = AtomicU32::new(0);

#[esp_hal::handler(priority = Priority::Priority1)]
fn cross_core_ipi() {
    let system = unsafe { &*esp32::DPORT::PTR };
    system
        .cpu_intr_from_cpu_0()
        .write(|w| w.cpu_intr_from_cpu_0().clear_bit());
}

fn wake_app_core() {
    let system = unsafe { &*esp32::DPORT::PTR };
    system
        .cpu_intr_from_cpu_0()
        .write(|w| w.cpu_intr_from_cpu_0().set_bit());
}

fn app_core_task() {
    APP_CORE_IPI_READY.store(1, Ordering::Release);
    // The peripheral singleton is never otherwise constructed. After binding,
    // wake/reset operations use the PAC register directly on their owning core.
    {
        let mut ipi = unsafe { SoftwareInterrupt::<0>::steal() };
        ipi.set_interrupt_handler(cross_core_ipi);
    }
    APP_CORE_IPI_READY.store(2, Ordering::Release);
    unsafe { core::arch::asm!("waiti 0") };
    APP_CORE_IPI_READY.store(3, Ordering::Release);
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
    let mut alarm = Esp32Alarm::new(OneShotTimer::new(timers.timer0));
    let mut bridge = Esp32BridgeUart::new(
        Uart::new(peripherals.UART0, UartConfig::default())
            .unwrap()
            .with_tx(peripherals.GPIO1)
            .with_rx(peripherals.GPIO3),
    );
    let mut cpu_control = CpuControl::new(peripherals.CPU_CTRL);
    let _ = prepare_app_core_start(&mut cpu_control);
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

    let started = Esp32Clock::now_us();
    let required = HardwareCapabilitySet::EMPTY
        .with(HardwareCapability::Timebase)
        .with(HardwareCapability::Deadline)
        .with(HardwareCapability::Multicore);
    let route_ok = ESP32_BOARD_IO_ROUTES == [EspIoRoute::ExternalUsbUartBridge]
        && ESP32_BOARD_IO_ROUTES[0].application_usb_lease().is_none();
    let providers_ok = Esp32Providers::supports(required) && route_ok;
    let multicore_deadline = started.saturating_add(100_000);
    while APP_CORE_IPI_READY.load(Ordering::Acquire) < 2
        && Esp32Clock::now_us() < multicore_deadline
    {
        core::hint::spin_loop();
    }
    if multicore_started && APP_CORE_IPI_READY.load(Ordering::Acquire) == 2 {
        wake_app_core();
    }
    while APP_CORE_IPI_READY.load(Ordering::Acquire) < 3
        && Esp32Clock::now_us() < multicore_deadline
    {
        core::hint::spin_loop();
    }
    let first_generation = lifecycle.generation(1).map(|generation| generation.get());
    let first_ready = multicore_started
        && APP_CORE_IPI_READY.load(Ordering::Acquire) == 3
        && first_generation.is_some_and(multicore_campaign::begin_generation);
    if first_ready {
        wake_app_core();
    }
    let pre = first_ready
        .then(|| {
            multicore_campaign::run_until_wedge(&mut lifecycle, Esp32Clock::now_us, wake_app_core)
        })
        .flatten();
    let mut campaign = multicore_campaign::CampaignReport::default();
    if let Some(pre) = pre {
        let faulted = lifecycle.fault(1).is_ok();
        drop(app_core.take());
        let _ = prepare_app_core_start(&mut cpu_control);
        APP_CORE_IPI_READY.store(0, Ordering::Release);
        let replacement_generation = lifecycle
            .generation(1)
            .and_then(|generation| generation.get().checked_add(1))
            .unwrap_or(0);
        let plane_prepared = multicore_campaign::begin_generation(replacement_generation);
        let recovered = faulted
            && plane_prepared
            && lifecycle
                .recover(1, |_| {
                    app_core = cpu_control
                        .start_app_core(
                            unsafe { &mut *addr_of_mut!(APP_CORE_STACK) },
                            app_core_task,
                        )
                        .ok();
                    app_core.is_some()
                })
                .is_ok();
        let ready_deadline = Esp32Clock::now_us().saturating_add(100_000);
        while recovered
            && APP_CORE_IPI_READY.load(Ordering::Acquire) < 2
            && Esp32Clock::now_us() < ready_deadline
        {
            core::hint::spin_loop();
        }
        if recovered && APP_CORE_IPI_READY.load(Ordering::Acquire) == 2 {
            wake_app_core();
        }
        while recovered
            && APP_CORE_IPI_READY.load(Ordering::Acquire) < 3
            && Esp32Clock::now_us() < ready_deadline
        {
            core::hint::spin_loop();
        }
        if recovered && APP_CORE_IPI_READY.load(Ordering::Acquire) == 3 {
            campaign = multicore_campaign::finish_recovery(
                pre,
                replacement_generation,
                Esp32Clock::now_us,
                wake_app_core,
            );
        }
    }
    let multicore_ok = campaign.passed && lifecycle.all_up() && app_core.is_some();
    let alarm_started = Esp32Clock::now_us();
    let armed = alarm.arm_after_us(2_000).is_ok();
    while armed && !alarm.poll_due(Esp32Clock::now_us()) {
        core::hint::spin_loop();
    }
    let elapsed = Esp32Clock::now_us().saturating_sub(alarm_started);
    let deadline_ok = armed && (2_000..20_000).contains(&elapsed);

    loop {
        let _ = writeln!(
            bridge,
            "NOBRO-ESP32 uart_bridge=1 native_usb=0 time={} deadline={} multicore={} started={} ipi_ready={} generation={} accepted={} rejected={} processed={} expected={} actual={} cancelled={} stale={} fallback={} restart={} all_pass={}",
            Esp32Clock::now_us(),
            u32::from(deadline_ok),
            u32::from(multicore_ok),
            u32::from(multicore_started),
            APP_CORE_IPI_READY.load(Ordering::Acquire),
            first_generation.unwrap_or(0),
            campaign.accepted,
            campaign.rejected,
            campaign.processed,
            campaign.expected,
            campaign.actual,
            campaign.cancelled,
            campaign.stale,
            campaign.fallback,
            campaign.restart,
            u32::from(providers_ok && deadline_ok && multicore_ok)
        );
        let _ = bridge.flush();
        let _ = &app_core;
        delay.delay_millis(1_000);
    }
}
