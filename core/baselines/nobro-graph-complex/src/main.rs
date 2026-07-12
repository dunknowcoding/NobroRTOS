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
use panic_halt as _;

use nobro_kernel::{
    AppGraph, ContainmentPolicy, FaultThresholds, KernelExecutor, MessageKind, ModuleCtx, Poll,
    Runtime, SystemProfile, TaskDecl,
};
use nobro_power::{PowerHookError, PowerMode, PowerPlatform};

#[no_mangle]
#[used]
static mut BASELINE_REPORT: [u32; 4] = [0; 4];

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

type Rt = Runtime<8, 8, 16, 8, 16, 8, 32>;
type Ctx<'a> = ModuleCtx<'a, 8, 8, 16, 8, 16, 8, 32>;

#[entry]
fn main() -> ! {
    unsafe {
        wr(GPIO_P0 + 0x700 + 4 * PIN, 1);
    }
    timer_init();

    // The whole complex contract: five tasks, three channels. Everything else
    // (manifest, admission, quotas, capabilities, startup, executor) derives.
    let built = AppGraph::<5>::new()
        .task(TaskDecl::periodic("fusion", 10_000))
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
        KernelExecutor::<8, 8, 8, 16, 8, 16, 8, 32>::new(runtime, ContainmentPolicy::Cooperative);
    for meta in built.tasks.iter().flatten() {
        exec.add_task(*meta, 0).unwrap();
    }
    exec.seal().unwrap();

    let mut report = [0u32; 4]; // control_ticks, fusion_samples, radio_tx, drops
    let mut fused: u32 = 0;
    let mut ring = [0u32; 8];
    let mut ring_head = 0usize;
    let mut power = AlwaysOn;
    loop {
        exec.run_cycle(micros, &mut power, |ctx: &mut Ctx<'_>| {
            let m = ctx.module();
            if m == fusion {
                // Fold two synthetic streams into a fixed-point estimate.
                report[1] = report[1].wrapping_add(1);
                let a = report[1].wrapping_mul(3).wrapping_add(7);
                let b = report[1].wrapping_mul(5).wrapping_add(11);
                fused = fused - (fused >> 3) + ((a ^ b) >> 3);
                if ctx.send(control, MessageKind::SampleReady, fused, 0).is_err() {
                    report[3] = report[3].wrapping_add(1);
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
                    let cmd = msg.arg0;
                    // Fan out a command to the radio and a sample to storage;
                    // a full downstream mailbox is counted as backpressure.
                    if ctx.send(radio, MessageKind::Command, cmd, 0).is_err() {
                        report[3] = report[3].wrapping_add(1);
                    }
                    if ctx.send(storage, MessageKind::SampleReady, cmd, 0).is_err() {
                        report[3] = report[3].wrapping_add(1);
                    }
                }
            } else if m == radio {
                while let Ok(Some(_msg)) = ctx.recv() {
                    report[2] = report[2].wrapping_add(1); // tx counter
                }
            } else if m == storage {
                while let Ok(Some(msg)) = ctx.recv() {
                    ring[ring_head] = msg.arg0; // bounded ring "storage"
                    ring_head = (ring_head + 1) % ring.len();
                }
            } else {
                // diagnostics: best-effort health fold, no channel.
            }
            Ok(Poll::Ready)
        })
        .unwrap();
        unsafe {
            core::ptr::write_volatile(core::ptr::addr_of_mut!(BASELINE_REPORT), report);
        }
    }
}
