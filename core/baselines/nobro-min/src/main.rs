//! NobroRTOS implementation of the baseline workload.
//!
//! Everything the framework stands for stays on: manifest validation,
//! admission, capability grants, object quotas, the authoritative executor,
//! and the traced ModuleCtx mailbox. The GPIO/TIMER access is raw-register,
//! byte-identical in spirit to `baremetal-min`, so the measured delta is the
//! kernel itself - not a driver stack.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

use nobro_kernel::{
    kernel_module_spec, Capability, CapabilitySet, ContainmentPolicy, Criticality,
    DeadlineContract, DependencySet, FaultThresholds, KernelExecutor, MemoryBudget, MessageKind,
    ModuleId, ModuleSpec, Poll, StartupNode, SystemManifest, SystemProfile, TaskMeta,
};
use nobro_power::{PowerHookError, PowerMode, PowerPlatform};

/// Baseline power backend: always on (sleep behavior is measured on the HIL
/// rig, not in the size specimen).
struct AlwaysOn;

impl PowerPlatform for AlwaysOn {
    fn program_wake(&mut self, _deadline_us: Option<u64>) -> Result<(), PowerHookError> {
        Ok(())
    }
    fn enter(&mut self, _mode: PowerMode) -> Result<(), PowerHookError> {
        Ok(())
    }
    fn suspend(&mut self, _task_id: u16) -> Result<(), PowerHookError> {
        Ok(())
    }
    fn resume(&mut self, _task_id: u16) -> Result<(), PowerHookError> {
        Ok(())
    }
}

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

type Exec = KernelExecutor<4, 4, 4, 8, 4, 8, 4, 16>;

#[entry]
fn main() -> ! {
    unsafe {
        wr(GPIO_P0 + 0x700 + 4 * PIN, 1);
    }
    timer_init();

    // The contract: two modules, declared budgets, message capability.
    let mut manifest = SystemManifest::<3>::new();
    manifest
        .add(kernel_module_spec(
            MemoryBudget::new(8 * 1024, 2 * 1024, 1),
            DeadlineContract::new(20_000, 100),
        ))
        .unwrap();
    manifest
        .add(
            ModuleSpec::new(ModuleId::Actuator, Criticality::HardRealtime)
                .requires(CapabilitySet::empty().with(Capability::Mailbox))
                .memory(MemoryBudget::new(1024, 256, 0))
                .deadline(DeadlineContract::new(20_000, 500).execution_budget(2_000)),
        )
        .unwrap();
    manifest
        .add(
            ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
                .owns(CapabilitySet::empty().with(Capability::Mailbox))
                .memory(MemoryBudget::new(1024, 256, 0))
                .deadline(DeadlineContract::new(100_000, 1_000).execution_budget(2_000)),
        )
        .unwrap();
    let nodes = [
        StartupNode::new(ModuleId::Kernel, DependencySet::empty()),
        StartupNode::new(ModuleId::Actuator, DependencySet::empty()),
        StartupNode::new(ModuleId::Sensor, DependencySet::empty()),
    ];
    let mut runtime = nobro_kernel::Runtime::admit(
        &manifest,
        &nodes,
        SystemProfile::NRF52840_CORE,
        FaultThresholds::DEFAULT,
    )
    .unwrap();
    runtime.boot_to_running(micros()).unwrap();

    let mut exec = Exec::new(runtime, ContainmentPolicy::Cooperative);
    exec.add_task(
        TaskMeta::new(ModuleId::Actuator, Criticality::HardRealtime, 20_000, 2_000),
        0,
    )
    .unwrap();
    exec.add_task(
        TaskMeta::new(ModuleId::Sensor, Criticality::Driver, 100_000, 2_000),
        0,
    )
    .unwrap();
    exec.seal().unwrap();

    let mut control_ticks: u32 = 0;
    let mut samples: u32 = 0;
    let mut filtered: u32 = 0;
    let mut drops: u32 = 0;
    let mut power = AlwaysOn;

    loop {
        let outcome = exec
            .run_cycle(micros, &mut power, |ctx: &mut nobro_kernel::ModuleCtx<'_, 4, 4, 8, 4, 8, 4, 16>| {
                match ctx.module() {
                    ModuleId::Actuator => {
                        // control: toggle + drain the sensor channel (consumer).
                        control_ticks = control_ticks.wrapping_add(1);
                        unsafe {
                            if control_ticks & 1 == 0 {
                                wr(GPIO_P0 + 0x508, 1 << PIN);
                            } else {
                                wr(GPIO_P0 + 0x50C, 1 << PIN);
                            }
                        }
                        while let Ok(Some(message)) = ctx.recv() {
                            filtered = filtered - (filtered >> 3) + (message.arg0 >> 3);
                        }
                    }
                    ModuleId::Sensor => {
                        samples = samples.wrapping_add(1);
                        let value = samples.wrapping_mul(3).wrapping_add(7);
                        if ctx
                            .send(ModuleId::Actuator, MessageKind::SampleReady, value, 0)
                            .is_err()
                        {
                            drops = drops.wrapping_add(1);
                        }
                    }
                    _ => {}
                }
                Ok(Poll::Ready)
            })
            .unwrap();
        let _ = outcome;
        unsafe {
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!(BASELINE_REPORT),
                [control_ticks, samples, filtered, drops],
            );
        }
    }
}
