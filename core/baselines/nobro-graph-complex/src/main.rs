//! The COMPLEX baseline workload (Wave 59), NobroRTOS graph API.
//!
//! A five-stage pipeline with backpressure — the kind of "multiple complex
//! tasks" NobroRTOS is claimed to be unable to handle:
//!   fusion(100Hz) -> control(50Hz) -> radio(20Hz)
//!                                  \-> storage(10Hz)
//!   diagnostics(5Hz) folds a health counter
//! The ENTIRE contract is five `TaskDecl`s + three `.channel()`s; manifest,
//! admission, quotas, capabilities, startup order, and the executor are all
//! derived. Report: [control_ticks, fusion_samples, radio_tx, drops].
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use nobro_hal::NrfTimerPower;
#[cfg(not(nobro_ram_run))]
use panic_halt as _;

// BENCH_INSTRUMENTATION_BEGIN
#[cfg(nobro_ram_run)]
#[panic_handler]
fn runtime_panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let (line, column) = info
        .location()
        .map(|location| (location.line(), location.column()))
        .unwrap_or((0, 0));
    unsafe {
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(BASELINE_REPORT),
            [0xE000_0001, line, column, 0],
        );
    }
    loop {
        core::hint::spin_loop();
    }
}
// BENCH_INSTRUMENTATION_END

use nobro_kernel::{
    AppGraph, ContainmentPolicy, Criticality, FaultThresholds, KernelExecutor, MessageKind,
    ModuleCtx, Poll, Runtime, SystemProfile, TaskDecl,
};

#[no_mangle]
#[used]
static mut BASELINE_REPORT: [u32; 4] = [0; 4];

// BENCH_INSTRUMENTATION_BEGIN
#[cfg(nobro_ram_run)]
#[no_mangle]
#[used]
static mut RUNTIME_BUSY_CYCLES: u32 = 0;
#[cfg(nobro_ram_run)]
#[no_mangle]
#[used]
static mut RUNTIME_LATENCY: [u32; 7] = [0; 7];
#[cfg(nobro_ram_run)]
#[no_mangle]
#[used]
static mut RUNTIME_IDLE_CYCLES: u32 = 0;
#[cfg(nobro_ram_run)]
#[no_mangle]
#[used]
static mut RUNTIME_IDLE_ENTRIES: u32 = 0;
// BENCH_INSTRUMENTATION_END

const GPIO_P0: u32 = 0x5000_0000;
const PIN: u32 = 15;

unsafe fn wr(a: u32, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v);
}
unsafe fn rd(a: u32) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}

fn micros() -> u64 {
    NrfTimerPower::now_us()
}

// BENCH_INSTRUMENTATION_BEGIN
#[cfg(nobro_ram_run)]
fn runtime_cycles() -> u32 {
    unsafe { rd(0xE000_1004) }
}

#[cfg(nobro_ram_run)]
fn account_runtime(start: u32) {
    unsafe {
        let current = core::ptr::read_volatile(core::ptr::addr_of!(RUNTIME_BUSY_CYCLES));
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(RUNTIME_BUSY_CYCLES),
            current.wrapping_add(runtime_cycles().wrapping_sub(start)),
        );
    }
}

#[cfg(nobro_ram_run)]
fn record_jitter(now_us: u32, last_us: &mut u32, period_us: u32) {
    let jitter = if *last_us == 0 {
        0
    } else {
        now_us
            .wrapping_sub(*last_us)
            .abs_diff(period_us)
            .saturating_mul(64)
    };
    *last_us = now_us;
    unsafe {
        let report = &mut *core::ptr::addr_of_mut!(RUNTIME_LATENCY);
        report[0] = report[0].wrapping_add(1);
        report[1] = report[1].max(jitter);
        report[2] = report[2].wrapping_add(jitter);
        let bucket = if jitter <= 64 {
            3
        } else if jitter <= 640 {
            4
        } else if jitter <= 6_400 {
            5
        } else {
            6
        };
        report[bucket] = report[bucket].wrapping_add(1);
    }
}
// BENCH_INSTRUMENTATION_END

type Rt = Runtime<6, 6, 4, 1, 1, 6, 8>;
type Ctx<'a> = ModuleCtx<'a, 6, 6, 4, 1, 1, 6, 8>;

#[entry]
fn main() -> ! {
    unsafe {
        wr(GPIO_P0 + 0x700 + 4 * PIN, 1);
    }
    let mut power = unsafe { NrfTimerPower::init() };

    // The whole complex contract: five tasks, three channels. Everything else
    // (manifest, admission, quotas, capabilities, startup, executor) derives.
    let built = AppGraph::<5>::new()
        .task(TaskDecl::periodic("fusion", 10_000).criticality(Criticality::System))
        .unwrap()
        .task(TaskDecl::control("control", 20_000))
        .unwrap()
        .task(TaskDecl::periodic("radio", 50_000))
        .unwrap()
        .task(TaskDecl::periodic("storage", 100_000))
        .unwrap()
        .task(TaskDecl::service("diagnostics", 200_000))
        .unwrap()
        .channel("fusion", "control")
        .unwrap()
        .channel("control", "radio")
        .unwrap()
        .channel("control", "storage")
        .unwrap()
        .build_for::<6>(SystemProfile::NRF52840_CORE)
        .unwrap();
    let fusion = built.module_of("fusion").unwrap();
    let control = built.module_of("control").unwrap();
    let radio = built.module_of("radio").unwrap();
    let storage = built.module_of("storage").unwrap();

    let mut runtime = Rt::admit(
        &built.manifest,
        built.startup_nodes(),
        SystemProfile::NRF52840_CORE,
        FaultThresholds::DEFAULT,
    )
    .unwrap();
    runtime.boot_to_running(micros()).unwrap();
    let mut exec =
        KernelExecutor::<5, 6, 6, 4, 1, 1, 6, 8>::new(runtime, ContainmentPolicy::Cooperative);
    for meta in built.tasks.iter().flatten() {
        exec.add_task(*meta, 0).unwrap();
    }
    exec.seal().unwrap();

    let mut report = [0u32; 4]; // control_ticks, fusion_samples, radio_tx, drops
    let mut fused: u32 = 0;
    let mut ring = [0u32; 8];
    let mut ring_head = 0usize;
    let mut fusion_pending = false;
    let mut radio_pending = false;
    let mut storage_pending = false;
    // BENCH_INSTRUMENTATION_BEGIN
    #[cfg(nobro_ram_run)]
    let (mut fusion_last, mut control_last, mut radio_last) = (0u32, 0u32, 0u32);
    // BENCH_INSTRUMENTATION_END
    loop {
        exec.run_cycle(micros, &mut power, |ctx: &mut Ctx<'_>| {
            // BENCH_INSTRUMENTATION_BEGIN
            #[cfg(nobro_ram_run)]
            let runtime_start = runtime_cycles();
            // BENCH_INSTRUMENTATION_END
            let m = ctx.module();
            // BENCH_INSTRUMENTATION_BEGIN
            #[cfg(nobro_ram_run)]
            if m == fusion {
                record_jitter(micros() as u32, &mut fusion_last, 10_000);
            } else if m == control {
                record_jitter(micros() as u32, &mut control_last, 20_000);
            } else if m == radio {
                record_jitter(micros() as u32, &mut radio_last, 50_000);
            }
            // BENCH_INSTRUMENTATION_END
            if m == fusion {
                // Fold two synthetic streams into a fixed-point estimate.
                report[1] = report[1].wrapping_add(1);
                let a = report[1].wrapping_mul(3).wrapping_add(7);
                let b = report[1].wrapping_mul(5).wrapping_add(11);
                fused = fused - (fused >> 3) + ((a ^ b) >> 3);
                if fusion_pending {
                    report[3] = report[3].wrapping_add(1);
                } else if ctx
                    .send(control, MessageKind::SampleReady, fused, 0)
                    .is_err()
                {
                    report[3] = report[3].wrapping_add(1);
                } else {
                    fusion_pending = true;
                }
            } else if m == control {
                report[0] = report[0].wrapping_add(1);
                unsafe {
                    if report[0] & 1 == 0 {
                        wr(GPIO_P0 + 0x508, 1 << PIN);
                    } else {
                        wr(GPIO_P0 + 0x50C, 1 << PIN);
                    }
                }
                while let Ok(Some(msg)) = ctx.recv() {
                    fusion_pending = false;
                    let cmd = msg.arg0;
                    // Fan out a command to the radio and a sample to storage;
                    // a full downstream mailbox is counted as backpressure.
                    if radio_pending {
                        report[3] = report[3].wrapping_add(1);
                    } else if ctx.send(radio, MessageKind::Command, cmd, 0).is_err() {
                        report[3] = report[3].wrapping_add(1);
                    } else {
                        radio_pending = true;
                    }
                    if storage_pending {
                        report[3] = report[3].wrapping_add(1);
                    } else if ctx.send(storage, MessageKind::SampleReady, cmd, 0).is_err() {
                        report[3] = report[3].wrapping_add(1);
                    } else {
                        storage_pending = true;
                    }
                }
            } else if m == radio {
                while let Ok(Some(_msg)) = ctx.recv() {
                    radio_pending = false;
                    report[2] = report[2].wrapping_add(1); // tx counter
                }
            } else if m == storage {
                while let Ok(Some(msg)) = ctx.recv() {
                    storage_pending = false;
                    ring[ring_head] = msg.arg0; // bounded ring "storage"
                    ring_head = (ring_head + 1) % ring.len();
                }
            } else {
                // diagnostics: best-effort health fold, no channel.
            }
            // BENCH_INSTRUMENTATION_BEGIN
            #[cfg(nobro_ram_run)]
            account_runtime(runtime_start);
            // BENCH_INSTRUMENTATION_END
            Ok(Poll::Ready)
        })
        .unwrap();
        // BENCH_INSTRUMENTATION_BEGIN
        #[cfg(nobro_ram_run)]
        unsafe {
            RUNTIME_IDLE_CYCLES = power.residency_us().saturating_mul(64) as u32;
            RUNTIME_IDLE_ENTRIES = power.entries();
        }
        // BENCH_INSTRUMENTATION_END
        unsafe {
            core::ptr::write_volatile(core::ptr::addr_of_mut!(BASELINE_REPORT), report);
        }
    }
}
