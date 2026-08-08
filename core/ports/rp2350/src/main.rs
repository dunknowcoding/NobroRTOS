//! NobroRTOS portable core on the RP2350 / Pico 2 W with self-DFU autonomy.
//!
//! Runs the timebase provider and a bounded cross-core application over USB CDC.
//! Sending the line `DFU` over the same serial port reboots the chip into the BOOTSEL
//! UF2 bootloader, so the host can reflash without anyone touching the board.
#![no_std]
#![no_main]

use panic_halt as _;
use rp235x_hal as hal;

use hal::usb::UsbBus;
use usb_device::{class_prelude::*, prelude::*};
use usbd_serial::SerialPort;

use core::sync::atomic::{AtomicU32, Ordering};
#[cfg(feature = "dma-completion")]
use hal::clocks::Clock;
#[cfg(feature = "dma-completion")]
use hal::dma::DMAExt;
use hal::multicore::{Multicore, Stack};

#[cfg(feature = "deep-selftest")]
use nobro_hal::Rp2FlashBackend;
use nobro_hal::{
    Rp2Cache, Rp2Flash, Rp2MulticoreContract, Rp2Power, Rp2Pulse, Rp2ResetBackend, Rp2Rtc,
    Rp2Watchdog,
};
use nobro_kernel::{
    CrossCoreDataPlane, CrossCoreMessage, CrossCoreReceive, ModuleId, MulticoreExecutorLifecycle,
};

mod deep;
#[cfg(feature = "dma-completion")]
pub mod dma_completion;
mod pio_selftest;
mod portable;

/// RP2350 boot: the bootrom requires this image-definition block.
#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const XTAL_FREQ_HZ: u32 = 12_000_000;

#[cfg(feature = "deep-selftest")]
fn storage_selftest<B: Rp2FlashBackend>(flash: &mut Rp2Flash<B>) -> bool {
    let mut backup = [0u8; 4096];
    if flash.read(0, &mut backup).is_err() {
        return false;
    }
    let pattern = [0x5au8; 256];
    let mut observed = [0u8; 256];
    let exercised = flash.erase_sector(0).is_ok()
        && flash.program_page(0, &pattern).is_ok()
        && flash.read(0, &mut observed).is_ok()
        && observed == pattern;
    let mut restored = flash.erase_sector(0).is_ok();
    for (index, chunk) in backup.chunks_exact(256).enumerate() {
        let mut page = [0u8; 256];
        page.copy_from_slice(chunk);
        restored &= flash.program_page(index as u32 * 256, &page).is_ok();
    }
    let mut verify = [0u8; 4096];
    restored &= flash.read(0, &mut verify).is_ok() && verify == backup;
    exercised && restored
}

// Core 1 drains a generation-safe SPSC plane, computes a running
// multiply-accumulate, and publishes the live result. Release/acquire indices
// make the transport independent of target cache assumptions.
static mut CORE1_STACK: Stack<4096> = Stack::new();
static CORE1_ACC: AtomicU32 = AtomicU32::new(0); // live result core0 reports
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
                    CORE1_ACC.fetch_add(message.payload.wrapping_mul(3), Ordering::AcqRel);
                    CORE1_PROCESSED.fetch_add(1, Ordering::Release);
                }
                CrossCoreReceive::Cancelled(_) => {
                    CORE1_CANCELLED.fetch_add(1, Ordering::Release);
                }
                CrossCoreReceive::Stale(_) => {
                    CORE1_STALE.fetch_add(1, Ordering::Release);
                }
            }
        }
        CORE1_IDLE_ENTRIES.fetch_add(1, Ordering::Release);
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

unsafe fn stop_core1_for_recovery() {
    let psm = &*hal::pac::PSM::ptr();
    psm.frce_off().modify(|_, w| w.proc1().set_bit());
    while !psm.frce_off().read().proc1().bit_is_set() {
        cortex_m::asm::nop();
    }
}

/// Append a decimal u32 to `buf` at `pos`, advancing `pos`.
fn put_u32(buf: &mut [u8], pos: &mut usize, mut v: u32) {
    let mut tmp = [0u8; 10];
    let mut n = 0;
    if v == 0 {
        tmp[0] = b'0';
        n = 1;
    }
    while v > 0 {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 && *pos < buf.len() {
        n -= 1;
        buf[*pos] = tmp[n];
        *pos += 1;
    }
}

fn put_bytes(buf: &mut [u8], pos: &mut usize, bytes: &[u8]) {
    let room = buf.len().saturating_sub(*pos);
    let count = room.min(bytes.len());
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
    let timer = hal::Timer::new_timer0(pac.TIMER0, &mut pac.RESETS, &clocks);
    let mut powman = hal::powman::Powman::new(pac.POWMAN, None);
    powman
        .aot_set_clock(hal::powman::AotClockSource::Xosc(
            hal::powman::FractionalFrequency::from_hz(XTAL_FREQ_HZ),
        ))
        .unwrap();
    powman.aot_start();
    let mut rtc = Rp2Rtc::try_new(deep::Rp2350Rtc(powman), 3).unwrap();
    let rtc_ok = rtc.ticks().is_ok() && rtc.ticks_per_second() == Ok(1_000);
    let mut pulse = Rp2Pulse::try_new(deep::Rp2350Pulse::new(21).unwrap(), 4).unwrap();
    let pulse_ok = pulse.read_pulse_us(1).is_ok();
    let mut cache = Rp2Cache::try_new(deep::Rp2350Cache, 5).unwrap();
    let cache_ok = cache.flush_xip().is_ok();
    let flash_backend = unsafe { deep::Rp2350Flash::before_core1_start() };
    #[allow(unused_mut)]
    let mut flash = Rp2Flash::try_new(flash_backend, 6).unwrap();
    let mut flash_byte = [0];
    #[allow(unused_mut)]
    let mut flash_ok = flash.read(0, &mut flash_byte).is_ok();
    #[cfg(feature = "deep-selftest")]
    {
        flash_ok &= storage_selftest(&mut flash);
    }
    let mut hardware_watchdog = Rp2Watchdog::try_new(deep::Rp2350Watchdog(watchdog), 7).unwrap();
    hardware_watchdog.arm(8_000_000).unwrap();
    let _reset_cause = <portable::Rp2350Reset as Rp2ResetBackend>::reset_cause();
    let _power = Rp2Power::try_new(portable::Rp2350Power, 2).unwrap();
    #[cfg(feature = "dma-completion")]
    let dma_report = {
        let dma_channels = pac.DMA.split(&mut pac.RESETS);
        let mut provider = dma_completion::Dma0Completion::new(
            dma_channels.ch0,
            dma_completion::DmaCompletionPriority::port_default(),
        );
        dma_completion::run_dma_selftest(&mut provider, clocks.system_clock.freq().to_Hz())
    };

    // Bring up core1 under the portable lifecycle contract.
    let core1_lease = Rp2MulticoreContract::try_acquire(1).unwrap();
    let mut sio = hal::Sio::new(pac.SIO);
    let mut mc = Multicore::new(&mut pac.PSM, &mut pac.PPB, &mut sio.fifo);
    let core1 = &mut mc.cores()[1];
    let mut lifecycle = MulticoreExecutorLifecycle::<2, 2>::new();
    lifecycle.place(0, ModuleId::Kernel, 1_000).unwrap();
    lifecycle.place(1, ModuleId::App(1), 4_000).unwrap();
    let multicore_started = lifecycle
        .start_all(
            |core| {
                if core == 0 {
                    true
                } else {
                    #[allow(static_mut_refs)]
                    let stack = unsafe { CORE1_STACK.take().unwrap() };
                    core1.spawn(stack, core1_task).is_ok()
                }
            },
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
        pac.USB,
        pac.USB_DPRAM,
        clocks.usb_clock,
        true,
        &mut pac.RESETS,
    ));
    let mut serial = SerialPort::new(&usb_bus);
    let mut usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x2E8A, 0x000A))
        .strings(&[StringDescriptors::default()
            .manufacturer("NobroRTOS")
            .product("nobro-rp2350-selftest")
            .serial_number("NBRO2350")])
        .unwrap()
        .device_class(2) // CDC
        .build();

    let timebase_ok = portable::verify_timebase_provider();
    let cyw_backend_ok = matches!(
        portable::CYW43439_BACKEND,
        nobro_hal::Rp2Cyw43Backend::PioSpi | nobro_hal::Rp2Cyw43Backend::Vendor
    );
    #[cfg(feature = "dma-completion")]
    let all = timebase_ok
        && rtc_ok
        && pulse_ok
        && cache_ok
        && flash_ok
        && pio_concurrent
        && cyw_backend_ok
        && dma_report.passed
        && lifecycle.all_up();
    #[cfg(not(feature = "dma-completion"))]
    let all = timebase_ok
        && rtc_ok
        && pulse_ok
        && cache_ok
        && flash_ok
        && pio_concurrent
        && cyw_backend_ok
        && lifecycle.all_up();

    let mut line_buf = [0u8; 16];
    let mut line_len = 0usize;
    let mut last_report = timer.get_counter();
    #[cfg(feature = "dma-completion")]
    let mut report = [0u8; 384];
    #[cfg(not(feature = "dma-completion"))]
    let mut report = [0u8; 256];
    let mut report_len = 0usize;
    let mut report_sent = 0usize;
    let mut stress: Option<StressRun> = None;

    loop {
        hardware_watchdog.feed().unwrap();
        let _ = usb_dev.poll(&mut [&mut serial]);

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
                            CORE1_ACC.fetch_add(fallback_value.wrapping_mul(3), Ordering::AcqRel);
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
                        }
                        CORE1_GENERATION.store(0, Ordering::Release);
                        CORE1_WEDGE.store(0, Ordering::Release);
                        CORE1_WEDGED.store(0, Ordering::Release);
                        let next_generation = run.generation.wrapping_add(1);
                        #[allow(static_mut_refs)]
                        let mut allocation = unsafe {
                            CORE1_STACK.reset();
                            CORE1_STACK.take()
                        };
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
                            u32::from(!failed && lifecycle.all_up()),
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
        }

        // heartbeat once a second
        let now = timer.get_counter();
        if stress.is_none() && (now - last_report).to_millis() >= 1000 && report_sent == report_len
        {
            last_report = now;
            let mut pos = 0;
            #[cfg(feature = "dma-completion")]
            put_bytes(
                &mut report,
                &mut pos,
                b"NOBRO-RP2350 arch=thumbv8m providers=2 timebase=",
            );
            #[cfg(not(feature = "dma-completion"))]
            put_bytes(
                &mut report,
                &mut pos,
                b"NOBRO-RP2350 arch=thumbv8m providers=1 timebase=",
            );
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
                put_bytes(&mut report, &mut pos, b" dma_polls=");
                put_u32(&mut report, &mut pos, dma_report.polls);
                put_bytes(&mut report, &mut pos, b" dma_irq=");
                put_u32(&mut report, &mut pos, dma_report.irq_wakes);
                put_bytes(&mut report, &mut pos, b" dma_wake=");
                put_u32(&mut report, &mut pos, dma_report.task_wakes);
                put_bytes(&mut report, &mut pos, b" dma_idle=");
                put_u32(&mut report, &mut pos, dma_report.idle_entries);
                put_bytes(&mut report, &mut pos, b" dma_res_us=");
                put_u32(&mut report, &mut pos, dma_report.idle_residence_us);
                put_bytes(&mut report, &mut pos, b" dma_total_us=");
                put_u32(&mut report, &mut pos, dma_report.completion_us);
                put_bytes(&mut report, &mut pos, b" dma_wake_us=");
                put_u32(&mut report, &mut pos, dma_report.wake_latency_us);
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
            put_bytes(&mut report, &mut pos, b" all_pass=");
            put_u32(
                &mut report,
                &mut pos,
                u32::from(all && CORE1_IDLE_ENTRIES.load(Ordering::Relaxed) != 0),
            );
            // Report the LIVE cross-core reactor result: how many work items
            // core1 processed and its running accumulator.
            put_bytes(&mut report, &mut pos, b" cores=2 core1_processed=");
            put_u32(
                &mut report,
                &mut pos,
                CORE1_PROCESSED.load(Ordering::Relaxed),
            );
            put_bytes(&mut report, &mut pos, b" core1_acc=");
            put_u32(&mut report, &mut pos, CORE1_ACC.load(Ordering::Relaxed));
            put_bytes(&mut report, &mut pos, b" core1_idle=");
            put_u32(
                &mut report,
                &mut pos,
                CORE1_IDLE_ENTRIES.load(Ordering::Relaxed),
            );
            if pos + 2 <= report.len() {
                report[pos] = b'\r';
                report[pos + 1] = b'\n';
                pos += 2;
            }
            report_len = pos;
            report_sent = 0;
        }

        // USB CDC writes may accept only one endpoint packet. Retain the
        // unsent suffix and advance it over later scheduler iterations rather
        // than dropping a partial status line or busy-waiting.
        if report_sent < report_len {
            if let Ok(written) = serial.write(&report[report_sent..report_len]) {
                report_sent += written;
            }
        }

        // self-DFU: the line "DFU" reboots into the BOOTSEL UF2 bootloader
        let mut rx = [0u8; 16];
        if let Ok(n) = serial.read(&mut rx) {
            for &c in &rx[..n] {
                if c == b'\n' || c == b'\r' {
                    if &line_buf[..line_len] == b"DFU" {
                        let _ = serial.write(b"rebooting to BOOTSEL\r\n");
                        // give the host a moment to drain the ack
                        let t0 = timer.get_counter();
                        while (timer.get_counter() - t0).to_millis() < 100 {
                            let _ = usb_dev.poll(&mut [&mut serial]);
                        }
                        hal::reboot::reboot(
                            hal::reboot::RebootKind::BootSel {
                                picoboot_disabled: false,
                                msd_disabled: false,
                            },
                            hal::reboot::RebootArch::Normal,
                        );
                    } else if &line_buf[..line_len] == b"STRESS" && stress.is_none() && all {
                        stress = Some(StressRun::new());
                    }
                    line_len = 0;
                } else if line_len < line_buf.len() {
                    line_buf[line_len] = c;
                    line_len += 1;
                }
            }
        }
    }
}
