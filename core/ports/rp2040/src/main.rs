//! NobroRTOS shared-RP contract status firmware for the official Pico.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};

use panic_halt as _;
use rp2040_hal as hal;
use usb_device::{class_prelude::UsbBusAllocator, prelude::*};
use usbd_serial::SerialPort;

#[cfg(feature = "dma-completion")]
use hal::dma::DMAExt;
use hal::{
    multicore::{Multicore, Stack},
    usb::UsbBus,
};
use nobro_hal::{Rp2MulticoreContract, Rp2Power, Rp2ResetBackend, RP2040_RUNTIME};
use nobro_kernel::MpmcChannel;

#[cfg(feature = "dma-completion")]
mod dma_completion;
mod pio_selftest;
mod portable;

#[link_section = ".boot2"]
#[used]
pub static BOOT2: [u8; 256] = rp2040_boot2::BOOT_LOADER_W25Q080;

const XTAL_FREQ_HZ: u32 = 12_000_000;
static CORE1_STACK: Stack<1024> = Stack::new();
static CORE1_ACC: AtomicU32 = AtomicU32::new(0);
static CORE1_PROCESSED: AtomicU32 = AtomicU32::new(0);
static CORE1_IDLE_ENTRIES: AtomicU32 = AtomicU32::new(0);
static XCORE_WORK: MpmcChannel<u32, 4, 2> = MpmcChannel::new();

const STRESS_ITEMS: u32 = 4_096;
const RECOVERY_VALUE: u32 = 0x5a5a_a5a5;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StressPhase {
    Feed,
    Drain,
    RecoverSend,
    RecoverDrain,
}

struct StressRun {
    phase: StressPhase,
    accepted: u32,
    rejected: u32,
    base_processed: u32,
    base_acc: u32,
    recovery_processed: u32,
    expected_delta: u32,
}

impl StressRun {
    fn new() -> Self {
        Self {
            phase: StressPhase::Feed,
            accepted: 0,
            rejected: 0,
            base_processed: CORE1_PROCESSED.load(Ordering::Relaxed),
            base_acc: CORE1_ACC.load(Ordering::Relaxed),
            recovery_processed: 0,
            expected_delta: 0,
        }
    }
}

fn core1_task() {
    loop {
        while let Some(value) = XCORE_WORK.try_recv() {
            let acc = CORE1_ACC
                .load(Ordering::Relaxed)
                .wrapping_add(value.wrapping_mul(3));
            CORE1_ACC.store(acc, Ordering::Relaxed);
            let processed = CORE1_PROCESSED.load(Ordering::Relaxed).wrapping_add(1);
            CORE1_PROCESSED.store(processed, Ordering::Relaxed);
        }
        let idle = CORE1_IDLE_ENTRIES.load(Ordering::Relaxed).wrapping_add(1);
        CORE1_IDLE_ENTRIES.store(idle, Ordering::Relaxed);
        cortex_m::asm::wfe();
    }
}

fn put_u32(buf: &mut [u8], pos: &mut usize, mut value: u32) {
    let mut digits = [0u8; 10];
    let mut count = 0usize;
    if value == 0 {
        digits[0] = b'0';
        count = 1;
    }
    while value != 0 {
        digits[count] = b'0' + (value % 10) as u8;
        value /= 10;
        count += 1;
    }
    while count != 0 && *pos < buf.len() {
        count -= 1;
        buf[*pos] = digits[count];
        *pos += 1;
    }
}

fn put_bytes(buf: &mut [u8], pos: &mut usize, bytes: &[u8]) {
    let count = bytes.len().min(buf.len().saturating_sub(*pos));
    buf[*pos..*pos + count].copy_from_slice(&bytes[..count]);
    *pos += count;
}

#[hal::entry]
fn main() -> ! {
    let mut pac = hal::pac::Peripherals::take().unwrap();
    let mut watchdog = hal::Watchdog::new(pac.WATCHDOG);
    let clocks = hal::clocks::init_clocks_and_plls(
        XTAL_FREQ_HZ,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    )
    .unwrap();
    let timer = hal::Timer::new(pac.TIMER, &mut pac.RESETS, &clocks);
    let _reset_cause = <portable::Rp2040Reset as Rp2ResetBackend>::reset_cause();
    let _power = Rp2Power::try_new(portable::Rp2040Power, 2).unwrap();
    let pio_ok = pio_selftest::run(pac.PIO0, &mut pac.RESETS);
    #[cfg(feature = "dma-completion")]
    let dma_report = {
        let channels = pac.DMA.split(&mut pac.RESETS);
        let mut provider = dma_completion::Dma0Completion::new(
            channels.ch0,
            dma_completion::DmaCompletionPriority::port_default(),
        );
        dma_completion::run_dma_selftest(&mut provider)
    };

    let core1_lease = Rp2MulticoreContract::try_acquire(1).unwrap();
    let mut sio = hal::Sio::new(pac.SIO);
    let mut multicore = Multicore::new(&mut pac.PSM, &mut pac.PPB, &mut sio.fifo);
    multicore.cores()[1]
        .spawn(CORE1_STACK.take().unwrap(), core1_task)
        .unwrap();
    core::mem::forget(core1_lease);

    let usb_bus = UsbBusAllocator::new(UsbBus::new(
        pac.USBCTRL_REGS,
        pac.USBCTRL_DPRAM,
        clocks.usb_clock,
        true,
        &mut pac.RESETS,
    ));
    let mut serial = SerialPort::new(&usb_bus);
    let mut usb = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x1209, 0x4e43))
        .strings(&[StringDescriptors::default()
            .manufacturer("NobroRTOS")
            .product("NobroRTOS RP2040")
            .serial_number("NOBRO-RP2040")])
        .unwrap()
        .device_class(usbd_serial::USB_CLASS_CDC)
        .build();

    let timebase_ok = portable::verify_timebase_provider();
    #[cfg(feature = "dma-completion")]
    let all = timebase_ok && pio_ok && dma_report.passed && RP2040_RUNTIME.cores == 2;
    #[cfg(not(feature = "dma-completion"))]
    let all = timebase_ok && pio_ok && RP2040_RUNTIME.cores == 2;
    let mut last_report = timer.get_counter();
    let mut command = [0u8; 16];
    let mut command_len = 0usize;
    let mut report = [0u8; 320];
    let mut report_len = 0usize;
    let mut report_sent = 0usize;
    let mut stress: Option<StressRun> = None;

    loop {
        let _ = usb.poll(&mut [&mut serial]);

        let mut stress_result = None;
        if let Some(run) = stress.as_mut() {
            match run.phase {
                StressPhase::Feed => {
                    let mut sent = false;
                    for _ in 0..64 {
                        if run.accepted == STRESS_ITEMS {
                            run.phase = StressPhase::Drain;
                            break;
                        }
                        let value = run.accepted.wrapping_add(1);
                        match XCORE_WORK.try_send(value) {
                            Ok(()) => {
                                run.accepted = run.accepted.wrapping_add(1);
                                run.expected_delta =
                                    run.expected_delta.wrapping_add(value.wrapping_mul(3));
                                sent = true;
                            }
                            Err(_) => {
                                run.rejected = run.rejected.wrapping_add(1);
                                break;
                            }
                        }
                    }
                    if sent {
                        cortex_m::asm::sev();
                    }
                }
                StressPhase::Drain => {
                    if CORE1_PROCESSED
                        .load(Ordering::Relaxed)
                        .wrapping_sub(run.base_processed)
                        == STRESS_ITEMS
                    {
                        run.phase = StressPhase::RecoverSend;
                    }
                }
                StressPhase::RecoverSend => match XCORE_WORK.try_send(RECOVERY_VALUE) {
                    Ok(()) => {
                        run.expected_delta = run
                            .expected_delta
                            .wrapping_add(RECOVERY_VALUE.wrapping_mul(3));
                        run.recovery_processed =
                            CORE1_PROCESSED.load(Ordering::Relaxed).wrapping_add(1);
                        run.phase = StressPhase::RecoverDrain;
                        cortex_m::asm::sev();
                    }
                    Err(_) => run.rejected = run.rejected.wrapping_add(1),
                },
                StressPhase::RecoverDrain => {
                    if CORE1_PROCESSED.load(Ordering::Relaxed) == run.recovery_processed {
                        let processed = CORE1_PROCESSED
                            .load(Ordering::Relaxed)
                            .wrapping_sub(run.base_processed);
                        let actual_delta =
                            CORE1_ACC.load(Ordering::Relaxed).wrapping_sub(run.base_acc);
                        stress_result = Some((
                            run.accepted,
                            run.rejected,
                            processed,
                            run.expected_delta,
                            actual_delta,
                        ));
                    }
                }
            }
        }

        if let Some((accepted, rejected, processed, expected, actual)) = stress_result {
            if report_sent == report_len {
                let mut pos = 0usize;
                put_bytes(&mut report, &mut pos, b"STRESS target=");
                put_u32(&mut report, &mut pos, STRESS_ITEMS);
                put_bytes(&mut report, &mut pos, b" accepted=");
                put_u32(&mut report, &mut pos, accepted);
                put_bytes(&mut report, &mut pos, b" rejected=");
                put_u32(&mut report, &mut pos, rejected);
                put_bytes(&mut report, &mut pos, b" processed=");
                put_u32(&mut report, &mut pos, processed);
                put_bytes(&mut report, &mut pos, b" expected=");
                put_u32(&mut report, &mut pos, expected);
                put_bytes(&mut report, &mut pos, b" actual=");
                put_u32(&mut report, &mut pos, actual);
                put_bytes(&mut report, &mut pos, b" recovery=1 result=");
                put_bytes(
                    &mut report,
                    &mut pos,
                    if accepted == STRESS_ITEMS
                        && rejected != 0
                        && processed == STRESS_ITEMS + 1
                        && actual == expected
                    {
                        b"PASS"
                    } else {
                        b"FAIL"
                    },
                );
                put_bytes(&mut report, &mut pos, b"\r\n");
                report_len = pos;
                report_sent = 0;
                stress = None;
            }
        } else if stress.is_none()
            && (timer.get_counter() - last_report).to_millis() >= 1_000
            && report_sent == report_len
        {
            last_report = timer.get_counter();
            let mut pos = 0usize;
            put_bytes(&mut report, &mut pos, b"NOBRO-RP2040 shared=1 timebase=");
            put_u32(&mut report, &mut pos, u32::from(timebase_ok));
            put_bytes(&mut report, &mut pos, b" pio=");
            put_u32(&mut report, &mut pos, u32::from(pio_ok));
            #[cfg(feature = "dma-completion")]
            {
                put_bytes(&mut report, &mut pos, b" dma=");
                put_u32(&mut report, &mut pos, u32::from(dma_report.passed));
                put_bytes(&mut report, &mut pos, b" dma_cancel=");
                put_u32(
                    &mut report,
                    &mut pos,
                    u32::from(dma_report.cancellation_output_untouched),
                );
                put_bytes(&mut report, &mut pos, b" dma_words=");
                put_u32(&mut report, &mut pos, dma_report.words as u32);
                put_bytes(&mut report, &mut pos, b" dma_polls=");
                put_u32(&mut report, &mut pos, dma_report.polls);
                put_bytes(&mut report, &mut pos, b" dma_irq=");
                put_u32(&mut report, &mut pos, dma_report.irq_wakes);
                put_bytes(&mut report, &mut pos, b" dma_wake=");
                put_u32(&mut report, &mut pos, dma_report.task_wakes);
                put_bytes(&mut report, &mut pos, b" dma_owner_fault=");
                put_u32(
                    &mut report,
                    &mut pos,
                    u32::from(dma_report.ownership_fault_rejected),
                );
                put_bytes(&mut report, &mut pos, b" dma_stale=");
                put_u32(
                    &mut report,
                    &mut pos,
                    u32::from(dma_report.stale_generation_rejected),
                );
                put_bytes(&mut report, &mut pos, b" dma_partial=");
                put_u32(
                    &mut report,
                    &mut pos,
                    u32::from(dma_report.partial_completion),
                );
                put_bytes(&mut report, &mut pos, b" dma_recover=");
                put_u32(
                    &mut report,
                    &mut pos,
                    u32::from(dma_report.timeout_recovered),
                );
            }
            put_bytes(&mut report, &mut pos, b" cores=2 core1_processed=");
            put_u32(
                &mut report,
                &mut pos,
                CORE1_PROCESSED.load(Ordering::Relaxed),
            );
            put_bytes(&mut report, &mut pos, b" core1_idle=");
            put_u32(
                &mut report,
                &mut pos,
                CORE1_IDLE_ENTRIES.load(Ordering::Relaxed),
            );
            put_bytes(&mut report, &mut pos, b" all_pass=");
            put_u32(
                &mut report,
                &mut pos,
                u32::from(all && CORE1_IDLE_ENTRIES.load(Ordering::Relaxed) != 0),
            );
            put_bytes(&mut report, &mut pos, b"\r\n");
            report_len = pos;
            report_sent = 0;
        }

        if report_sent < report_len {
            if let Ok(written) = serial.write(&report[report_sent..report_len]) {
                report_sent += written;
            }
        }

        let mut input = [0u8; 16];
        if let Ok(count) = serial.read(&mut input) {
            for &byte in &input[..count] {
                if matches!(byte, b'\r' | b'\n') {
                    if &command[..command_len] == b"DFU" {
                        let _ = serial.write(b"rebooting to BOOTSEL\r\n");
                        hal::rom_data::reset_to_usb_boot(0, 0);
                    } else if &command[..command_len] == b"STRESS" && stress.is_none() {
                        stress = Some(StressRun::new());
                    }
                    command_len = 0;
                } else if command_len < command.len() {
                    command[command_len] = byte;
                    command_len += 1;
                }
            }
        }

        // USB is deliberately polled on core 0. Do not park this loop in WFE:
        // the polling bus does not promise an interrupt/event for every
        // enumeration transaction, so sleeping here can deadlock CDC bring-up.
        // Core 1 and the DMA completion path provide the bounded idle/wake
        // evidence without compromising the control-plane USB endpoint.
        cortex_m::asm::nop();
    }
}
