//! The graph-API implementation of the baseline workload (UX-01/FLEX-01):
//! the SAME contract as `nobro-min` — manifest, admission, quotas, executor —
//! declared once, by name, with safe defaults. Compare the line counts.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

use nobro_kernel::{
    AppGraph, ContainmentPolicy, FaultThresholds, KernelExecutor, MessageKind, Poll, Runtime,
    SystemProfile, TaskDecl,
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

#[entry]
fn main() -> ! {
    unsafe {
        wr(GPIO_P0 + 0x700 + 4 * PIN, 1);
    }
    timer_init();

    // The whole contract: two tasks, one channel. Everything else is derived.
    let built = AppGraph::<2>::new()
        .task(TaskDecl::control("motor", 20_000))
        .unwrap()
        .task(TaskDecl::periodic("sensor", 100_000))
        .unwrap()
        .channel("sensor", "motor")
        .unwrap()
        .build_for::<3>(SystemProfile::NRF52840_CORE)
        .unwrap();
    let motor = built.module_of("motor").unwrap();
    let sensor = built.module_of("sensor").unwrap();

    let mut runtime = Runtime::<4, 4, 8, 4, 8, 4, 16>::admit(
        &built.manifest,
        built.startup_nodes(),
        SystemProfile::NRF52840_CORE,
        FaultThresholds::DEFAULT,
    )
    .unwrap();
    runtime.boot_to_running(micros()).unwrap();
    let mut exec = KernelExecutor::<4, 4, 4, 8, 4, 8, 4, 16>::new(
        runtime,
        ContainmentPolicy::Cooperative,
    );
    for meta in built.tasks.iter().flatten() {
        exec.add_task(*meta, 0).unwrap();
    }
    exec.seal().unwrap();

    let mut report = [0u32; 4]; // ticks, samples, filtered, drops
    let mut power = AlwaysOn;
    loop {
        exec.run_cycle(micros, &mut power, |ctx: &mut nobro_kernel::ModuleCtx<'_, 4, 4, 8, 4, 8, 4, 16>| {
            if ctx.module() == motor {
                report[0] = report[0].wrapping_add(1);
                unsafe {
                    if report[0] & 1 == 0 {
                        wr(GPIO_P0 + 0x508, 1 << PIN);
                    } else {
                        wr(GPIO_P0 + 0x50C, 1 << PIN);
                    }
                }
                while let Ok(Some(message)) = ctx.recv() {
                    report[2] = report[2] - (report[2] >> 3) + (message.arg0 >> 3);
                }
            } else {
                report[1] = report[1].wrapping_add(1);
                let value = report[1].wrapping_mul(3).wrapping_add(7);
                if ctx.send(motor, MessageKind::SampleReady, value, 0).is_err() {
                    report[3] = report[3].wrapping_add(1);
                }
            }
            Ok(Poll::Ready)
        })
        .unwrap();
        unsafe {
            core::ptr::write_volatile(core::ptr::addr_of_mut!(BASELINE_REPORT), report);
        }
    }
}
