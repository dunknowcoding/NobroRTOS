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
use nobro_power::{PowerHookError, PowerMode, PowerPlatform};

#[no_mangle]
#[used]
static mut BASELINE_REPORT: [u32; 4] = [0; 4];

// BENCH_INSTRUMENTATION_BEGIN
#[cfg(nobro_ram_run)]
#[no_mangle]
#[used]
static mut RUNTIME_BUSY_CYCLES: u32 = 0;
// BENCH_INSTRUMENTATION_END

const TIMER0: u32 = 0x4000_8000;
const GPIO_P0: u32 = 0x5000_0000;
const PIN: u32 = 15;

unsafe fn wr(a: u32, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v);
}
unsafe fn rd(a: u32) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}

fn timer_init() {
    unsafe {
        wr(TIMER0 + 0x504, 0);
        wr(TIMER0 + 0x508, 3);
        wr(TIMER0 + 0x510, 4);
        wr(TIMER0 + 0x000, 1);
    }
}

fn micros() -> u64 {
    unsafe {
        wr(TIMER0 + 0x040, 1);
        u64::from(rd(TIMER0 + 0x540))
    }
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
// BENCH_INSTRUMENTATION_END

struct AlwaysOn;
impl PowerPlatform for AlwaysOn {
    fn program_wake(&mut self, _d: Option<u64>) -> Result<(), PowerHookError> {
        Ok(())
    }
    fn enter(&mut self, _m: PowerMode) -> Result<(), PowerHookError> {
        Ok(())
    }
    fn suspend(&mut self, _t: u16) -> Result<(), PowerHookError> {
        Ok(())
    }
    fn resume(&mut self, _t: u16) -> Result<(), PowerHookError> {
        Ok(())
    }
}

type Rt = Runtime<6, 6, 4, 1, 1, 6, 8>;
type Ctx<'a> = ModuleCtx<'a, 6, 6, 4, 1, 1, 6, 8>;

#[entry]
fn main() -> ! {
    unsafe {
        wr(GPIO_P0 + 0x700 + 4 * PIN, 1);
    }
    timer_init();

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
    let mut power = AlwaysOn;
    loop {
        exec.run_cycle(micros, &mut power, |ctx: &mut Ctx<'_>| {
            // BENCH_INSTRUMENTATION_BEGIN
            #[cfg(nobro_ram_run)]
            let runtime_start = runtime_cycles();
            // BENCH_INSTRUMENTATION_END
            let m = ctx.module();
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
        unsafe {
            core::ptr::write_volatile(core::ptr::addr_of_mut!(BASELINE_REPORT), report);
        }
    }
}
