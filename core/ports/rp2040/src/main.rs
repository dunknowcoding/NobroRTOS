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
use nobro_hal::{RP2040_RUNTIME, Rp2MulticoreContract, Rp2Power, Rp2ResetBackend};
use nobro_kernel::{
    CrossCoreDataPlane, CrossCoreMessage, CrossCoreReceive, ModuleId, MulticoreExecutorLifecycle,
};

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
static CORE1_GENERATION: AtomicU32 = AtomicU32::new(0);
static CORE1_PAUSE: AtomicU32 = AtomicU32::new(0);
static CORE1_PAUSED: AtomicU32 = AtomicU32::new(0);
static CORE1_WEDGE: AtomicU32 = AtomicU32::new(0);
static CORE1_WEDGED: AtomicU32 = AtomicU32::new(0);
static CORE1_CANCELLED: AtomicU32 = AtomicU32::new(0);
static CORE1_STALE: AtomicU32 = AtomicU32::new(0);
static XCORE_WORK: CrossCoreDataPlane<u32, 8> = CrossCoreDataPlane::new();

const STRESS_ITEMS: u32 = 4_096;
const RECOVERY_VALUE: u32 = 0x5a5a_a5a5;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StressPhase {
    Feed,
    Drain,
    PauseForCancel,
    CancelDrain,
    PauseForFallback,
    PrepareWedge,
    WaitWedge,
    RecoverySend,
    RecoveryDrain,
    Failed,
}

struct StressRun {
    phase: StressPhase,
    accepted: u32,
    rejected: u32,
    base_processed: u32,
    base_acc: u32,
    base_cancelled: u32,
    base_stale: u32,
    recovery_processed: u32,
    expected_delta: u32,
    generation: u32,
    next_sequence: u32,
    fallback: u32,
}

impl StressRun {
    fn new() -> Self {
        Self {
            phase: StressPhase::Feed,
            accepted: 0,
            rejected: 0,
            base_processed: CORE1_PROCESSED.load(Ordering::Relaxed),
            base_acc: CORE1_ACC.load(Ordering::Relaxed),
            base_cancelled: CORE1_CANCELLED.load(Ordering::Relaxed),
            base_stale: CORE1_STALE.load(Ordering::Relaxed),
            recovery_processed: 0,
            expected_delta: 0,
            generation: CORE1_GENERATION.load(Ordering::Acquire),
            next_sequence: 2,
            fallback: 0,
        }
    }
}

fn core1_task() {
    loop {
        let generation = CORE1_GENERATION.load(Ordering::Acquire);
        if generation == 0 {
            cortex_m::asm::wfe();
            continue;
        }
        if CORE1_WEDGE.load(Ordering::Acquire) == generation {
            CORE1_WEDGED.store(generation, Ordering::Release);
            loop {
                core::hint::spin_loop();
            }
        }
        if CORE1_PAUSE.load(Ordering::Acquire) == generation {
            CORE1_PAUSED.store(generation, Ordering::Release);
            while CORE1_PAUSE.load(Ordering::Acquire) == generation {
                if CORE1_WEDGE.load(Ordering::Acquire) == generation {
                    CORE1_WEDGED.store(generation, Ordering::Release);
                    loop {
                        core::hint::spin_loop();
                    }
                }
                core::hint::spin_loop();
            }
            CORE1_PAUSED.store(0, Ordering::Release);
        }
        while let Some(disposition) = XCORE_WORK.try_receive() {
            match disposition {
                CrossCoreReceive::Work(message) => {
                    let acc = CORE1_ACC
                        .load(Ordering::Relaxed)
                        .wrapping_add(message.payload.wrapping_mul(3));
                    CORE1_ACC.store(acc, Ordering::Release);
                    let processed = CORE1_PROCESSED.load(Ordering::Relaxed).wrapping_add(1);
                    CORE1_PROCESSED.store(processed, Ordering::Release);
                }
                CrossCoreReceive::Cancelled(_) => {
                    let cancelled = CORE1_CANCELLED.load(Ordering::Relaxed).wrapping_add(1);
                    CORE1_CANCELLED.store(cancelled, Ordering::Release);
                }
                CrossCoreReceive::Stale(_) => {
                    let stale = CORE1_STALE.load(Ordering::Relaxed).wrapping_add(1);
                    CORE1_STALE.store(stale, Ordering::Release);
                }
            }
        }
        let idle = CORE1_IDLE_ENTRIES.load(Ordering::Relaxed).wrapping_add(1);
        CORE1_IDLE_ENTRIES.store(idle, Ordering::Release);
        cortex_m::asm::wfe();
    }
}

fn send_work(generation: u32, sequence: u32, payload: u32) -> bool {
    XCORE_WORK
        .try_send(CrossCoreMessage {
            generation,
            sequence,
            payload,
        })
        .is_ok()
}

/// Stop core 1 before reusing its owned stack. The campaign wedges core 1 only
/// outside critical sections, so forcing PROC1 off cannot strand the RP2040
/// cross-core critical-section lock.
unsafe fn stop_core1_for_recovery() {
    let psm = &*hal::pac::PSM::ptr();
    psm.frce_off().modify(|_, w| w.proc1().set_bit());
    while !psm.frce_off().read().proc1().bit_is_set() {
        cortex_m::asm::nop();
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
    let core1 = &mut multicore.cores()[1];
    let mut lifecycle = MulticoreExecutorLifecycle::<2, 2>::new();
    lifecycle.place(0, ModuleId::Kernel, 1_000).unwrap();
    lifecycle.place(1, ModuleId::App(1), 4_000).unwrap();
    let multicore_started = lifecycle
        .start_all(
            |core| core == 0 || core1.spawn(CORE1_STACK.take().unwrap(), core1_task).is_ok(),
            |_| {},
        )
        .is_ok();
    let first_generation = lifecycle.generation(1).unwrap().get();
    let generation_started =
        multicore_started && XCORE_WORK.begin_generation(first_generation).is_ok();
    if generation_started {
        CORE1_GENERATION.store(first_generation, Ordering::Release);
        cortex_m::asm::sev();
    }
    core::mem::forget(core1_lease);

    // Run PIO on core 0 while admitted work is live on core 1. PIO instruction
    // memory is write-only; the functional FIFO result is the oracle.
    let startup_processed = CORE1_PROCESSED.load(Ordering::Acquire).wrapping_add(1);
    let startup_sent = generation_started && send_work(first_generation, 1, 0x1234_5678);
    if startup_sent {
        cortex_m::asm::sev();
    }
    let pio_ok = pio_selftest::run(pac.PIO0, &mut pac.RESETS);
    let pio_wait_start = timer.get_counter();
    while startup_sent
        && CORE1_PROCESSED.load(Ordering::Acquire) != startup_processed
        && (timer.get_counter() - pio_wait_start).to_millis() < 100
    {
        cortex_m::asm::sev();
    }
    let pio_concurrent =
        startup_sent && pio_ok && CORE1_PROCESSED.load(Ordering::Acquire) == startup_processed;

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
    let all = timebase_ok
        && pio_concurrent
        && dma_report.passed
        && RP2040_RUNTIME.cores == 2
        && lifecycle.all_up();
    #[cfg(not(feature = "dma-completion"))]
    let all = timebase_ok && pio_concurrent && RP2040_RUNTIME.cores == 2 && lifecycle.all_up();
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
                        if send_work(run.generation, run.next_sequence, value) {
                            run.next_sequence = run.next_sequence.wrapping_add(1);
                            run.accepted = run.accepted.wrapping_add(1);
                            run.expected_delta =
                                run.expected_delta.wrapping_add(value.wrapping_mul(3));
                            sent = true;
                        } else {
                            run.rejected = run.rejected.wrapping_add(1);
                            break;
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
                        CORE1_PAUSE.store(run.generation, Ordering::Release);
                        cortex_m::asm::sev();
                        run.phase = StressPhase::PauseForCancel;
                    }
                }
                StressPhase::PauseForCancel => {
                    if CORE1_PAUSED.load(Ordering::Acquire) == run.generation {
                        let cancelled_sequence = run.next_sequence;
                        let cancelled_sent =
                            send_work(run.generation, cancelled_sequence, 0xcace_1100);
                        run.next_sequence = run.next_sequence.wrapping_add(1);
                        let live_value = 0xcace_2200;
                        let live_sent = send_work(run.generation, run.next_sequence, live_value);
                        run.next_sequence = run.next_sequence.wrapping_add(1);
                        let cancelled =
                            XCORE_WORK.cancel_through(run.generation, cancelled_sequence);
                        if cancelled_sent && live_sent && cancelled {
                            run.expected_delta =
                                run.expected_delta.wrapping_add(live_value.wrapping_mul(3));
                            run.recovery_processed =
                                CORE1_PROCESSED.load(Ordering::Acquire).wrapping_add(1);
                            CORE1_PAUSE.store(0, Ordering::Release);
                            cortex_m::asm::sev();
                            run.phase = StressPhase::CancelDrain;
                        } else {
                            run.phase = StressPhase::Failed;
                        }
                    }
                }
                StressPhase::CancelDrain => {
                    if CORE1_PROCESSED.load(Ordering::Acquire) == run.recovery_processed
                        && CORE1_CANCELLED
                            .load(Ordering::Acquire)
                            .wrapping_sub(run.base_cancelled)
                            == 1
                    {
                        CORE1_PAUSE.store(run.generation, Ordering::Release);
                        cortex_m::asm::sev();
                        run.phase = StressPhase::PauseForFallback;
                    }
                }
                StressPhase::PauseForFallback => {
                    if CORE1_PAUSED.load(Ordering::Acquire) == run.generation {
                        let fallback_value = 0xfa11_bacc;
                        let moved_to_core0 = lifecycle.transfer(ModuleId::App(1), 1, 0).is_ok();
                        let sent = send_work(run.generation, run.next_sequence, fallback_value);
                        run.next_sequence = run.next_sequence.wrapping_add(1);
                        let consumed = matches!(
                            XCORE_WORK.try_receive(),
                            Some(CrossCoreReceive::Work(CrossCoreMessage {
                                payload,
                                ..
                            })) if payload == fallback_value
                        );
                        let moved_back = lifecycle.transfer(ModuleId::App(1), 0, 1).is_ok();
                        if moved_to_core0 && sent && consumed && moved_back {
                            let acc = CORE1_ACC
                                .load(Ordering::Acquire)
                                .wrapping_add(fallback_value.wrapping_mul(3));
                            CORE1_ACC.store(acc, Ordering::Release);
                            run.expected_delta = run
                                .expected_delta
                                .wrapping_add(fallback_value.wrapping_mul(3));
                            run.fallback = 1;
                            CORE1_PAUSE.store(0, Ordering::Release);
                            cortex_m::asm::sev();
                            run.phase = StressPhase::PrepareWedge;
                        } else {
                            run.phase = StressPhase::Failed;
                        }
                    }
                }
                StressPhase::PrepareWedge => {
                    CORE1_PAUSE.store(run.generation, Ordering::Release);
                    cortex_m::asm::sev();
                    if CORE1_PAUSED.load(Ordering::Acquire) == run.generation {
                        let stale_sent = send_work(run.generation, run.next_sequence, 0xdead_beef);
                        run.next_sequence = run.next_sequence.wrapping_add(1);
                        if stale_sent {
                            CORE1_WEDGE.store(run.generation, Ordering::Release);
                            CORE1_PAUSE.store(0, Ordering::Release);
                            cortex_m::asm::sev();
                            run.phase = StressPhase::WaitWedge;
                        } else {
                            run.phase = StressPhase::Failed;
                        }
                    }
                }
                StressPhase::WaitWedge => {
                    if CORE1_WEDGED.load(Ordering::Acquire) == run.generation {
                        let faulted = lifecycle.fault(1).is_ok();
                        unsafe {
                            stop_core1_for_recovery();
                            CORE1_STACK.reset();
                        }
                        CORE1_GENERATION.store(0, Ordering::Release);
                        CORE1_WEDGE.store(0, Ordering::Release);
                        CORE1_WEDGED.store(0, Ordering::Release);
                        let next_generation = run.generation.wrapping_add(1);
                        let mut allocation = CORE1_STACK.take();
                        let restarted = faulted
                            && allocation.is_some()
                            && lifecycle
                                .recover(1, |_| {
                                    allocation
                                        .take()
                                        .is_some_and(|stack| core1.spawn(stack, core1_task).is_ok())
                                })
                                .is_ok()
                            && lifecycle.generation(1).map(|g| g.get()) == Some(next_generation)
                            && XCORE_WORK.begin_generation(next_generation).is_ok();
                        if restarted {
                            run.generation = next_generation;
                            run.next_sequence = 1;
                            CORE1_GENERATION.store(next_generation, Ordering::Release);
                            cortex_m::asm::sev();
                            run.phase = StressPhase::RecoverySend;
                        } else {
                            run.phase = StressPhase::Failed;
                        }
                    }
                }
                StressPhase::RecoverySend => {
                    if send_work(run.generation, run.next_sequence, RECOVERY_VALUE) {
                        run.next_sequence = run.next_sequence.wrapping_add(1);
                        run.expected_delta = run
                            .expected_delta
                            .wrapping_add(RECOVERY_VALUE.wrapping_mul(3));
                        run.recovery_processed =
                            CORE1_PROCESSED.load(Ordering::Acquire).wrapping_add(1);
                        run.phase = StressPhase::RecoveryDrain;
                        cortex_m::asm::sev();
                    } else {
                        run.rejected = run.rejected.wrapping_add(1);
                    }
                }
                StressPhase::RecoveryDrain | StressPhase::Failed => {
                    let failed = run.phase == StressPhase::Failed;
                    if failed
                        || (CORE1_PROCESSED.load(Ordering::Acquire) == run.recovery_processed
                            && CORE1_STALE
                                .load(Ordering::Acquire)
                                .wrapping_sub(run.base_stale)
                                == 1)
                    {
                        let processed = CORE1_PROCESSED
                            .load(Ordering::Acquire)
                            .wrapping_sub(run.base_processed);
                        let actual_delta =
                            CORE1_ACC.load(Ordering::Acquire).wrapping_sub(run.base_acc);
                        stress_result = Some((
                            run.accepted,
                            run.rejected,
                            processed,
                            run.expected_delta,
                            actual_delta,
                            CORE1_CANCELLED
                                .load(Ordering::Acquire)
                                .wrapping_sub(run.base_cancelled),
                            CORE1_STALE
                                .load(Ordering::Acquire)
                                .wrapping_sub(run.base_stale),
                            run.fallback,
                            u32::from(!failed && lifecycle.state(1).is_some()),
                        ));
                    }
                }
            }
        }

        if let Some((
            accepted,
            rejected,
            processed,
            expected,
            actual,
            cancelled,
            stale,
            fallback,
            restart,
        )) = stress_result
        {
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
                put_bytes(&mut report, &mut pos, b" cancelled=");
                put_u32(&mut report, &mut pos, cancelled);
                put_bytes(&mut report, &mut pos, b" stale=");
                put_u32(&mut report, &mut pos, stale);
                put_bytes(&mut report, &mut pos, b" fallback=");
                put_u32(&mut report, &mut pos, fallback);
                put_bytes(&mut report, &mut pos, b" restart=");
                put_u32(&mut report, &mut pos, restart);
                put_bytes(&mut report, &mut pos, b" result=");
                put_bytes(
                    &mut report,
                    &mut pos,
                    if accepted == STRESS_ITEMS
                        && rejected != 0
                        && processed == STRESS_ITEMS + 2
                        && actual == expected
                        && cancelled == 1
                        && stale == 1
                        && fallback == 1
                        && restart == 1
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
            put_u32(&mut report, &mut pos, u32::from(pio_concurrent));
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
                    } else if &command[..command_len] == b"STRESS" && stress.is_none() && all {
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
