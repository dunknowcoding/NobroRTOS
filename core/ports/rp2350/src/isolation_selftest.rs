//! Exact RP2350 PMSAv8-M isolation and restart self-test.
#![no_main]
#![no_std]

use core::{
    mem::MaybeUninit,
    sync::atomic::{AtomicU32, Ordering},
};

use cortex_m_rt::exception;
use nobro_hal::{
    hardware_isolation_capabilities,
    mpu::{capture_active_mpu_fault, ModuleMpuContext},
    ContextRecord, CortexMSliceSwitch, IsolationCapabilities, IsolationEpoch, IsolationPlan,
    IsolationReceipt, IsolationRegion, LeaseError, LeaseGuard, Resource, ResourceLease,
};
use panic_halt as _;
use rp235x_hal as hal;

#[link_section = ".start_block"]
#[used]
static IMAGE_DEF: hal::block::ImageDef = hal::block::ImageDef::secure_exe();

const MAGIC: u32 = 0x4E49_3850; // "NI8P"
const VERSION: u32 = 1;
const PROVIDER_ID: u32 = 0x5250_3233;
const MODULE_A_ID: u16 = 1;
const MODULE_B_ID: u16 = 2;
const CFSR: u32 = 0xE000_ED28;
const MMFAR: u32 = 0xE000_ED34;
const TIMER0_BASE: u32 = 0x400B_0000;
const TIMER1_BASE: u32 = 0x400B_8000;

#[repr(C)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    faults: u32,
    fault_mask: u32,
    last_module: u32,
    last_address: u32,
    last_pc: u32,
    fault_control: u32,
    final_control: u32,
    stale_rejections: u32,
    lease_stale: u32,
    lease_recoveries: u32,
    final_generation: u32,
    switched_record: u32,
    module_a_value: u32,
    module_b_value: u32,
    denied_write_value: u32,
    allowed_peripheral: u32,
    allowed_stack: u32,
    diagnostic: u32,
}

#[no_mangle]
#[used]
static mut NOBRO_RP2350_ISOLATION_REPORT: Report = Report {
    magic: MAGIC,
    version: VERSION,
    completed: 0,
    all_pass: 0,
    faults: 0,
    fault_mask: 0,
    last_module: 0,
    last_address: 0,
    last_pc: 0,
    fault_control: 0,
    final_control: 0,
    stale_rejections: 0,
    lease_stale: 0,
    lease_recoveries: 0,
    final_generation: 0,
    switched_record: 0,
    module_a_value: 0,
    module_b_value: 0,
    denied_write_value: 0,
    allowed_peripheral: 0,
    allowed_stack: 0,
    diagnostic: 0,
};

#[repr(align(256))]
struct Region([u32; 64]);
#[repr(align(256))]
struct Stack([u8; 256]);

static mut MODULE_A_DATA: Region = Region([0; 64]);
static mut MODULE_B_DATA: Region = Region([0; 64]);
static mut MODULE_A_GUARD: Region = Region([0; 64]);
static mut MODULE_A_STACK: Stack = Stack([0; 256]);
static mut MODULE_B_STACK: Stack = Stack([0; 256]);
static MODULE_A_CONTEXT: ContextRecord = ContextRecord::empty();
static MODULE_B_CONTEXT: ContextRecord = ContextRecord::empty();
static MODULE_A_EPOCH: IsolationEpoch = IsolationEpoch::new();
static MODULE_B_EPOCH: IsolationEpoch = IsolationEpoch::new();
static mut MODULE_A_MPU: MaybeUninit<ModuleMpuContext> = MaybeUninit::uninit();
static mut MODULE_B_MPU: MaybeUninit<ModuleMpuContext> = MaybeUninit::uninit();
static mut MODULE_A_RECEIPT: MaybeUninit<IsolationReceipt> = MaybeUninit::uninit();
static mut MODULE_A_LEASE: MaybeUninit<LeaseGuard> = MaybeUninit::uninit();

static STAGE: AtomicU32 = AtomicU32::new(0);
static FAULTS: AtomicU32 = AtomicU32::new(0);
static FAULT_MASK: AtomicU32 = AtomicU32::new(0);
static LAST_MODULE: AtomicU32 = AtomicU32::new(0);
static LAST_ADDRESS: AtomicU32 = AtomicU32::new(0);
static LAST_PC: AtomicU32 = AtomicU32::new(0);
static FAULT_CONTROL: AtomicU32 = AtomicU32::new(0);
static STALE_REJECTIONS: AtomicU32 = AtomicU32::new(0);
static LEASE_STALE: AtomicU32 = AtomicU32::new(0);
static LEASE_RECOVERIES: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" {
    static __module_a_code_start: u8;
    static __module_b_code_start: u8;
}

fn capabilities() -> IsolationCapabilities {
    hardware_isolation_capabilities(PROVIDER_ID, 1).unwrap()
}

#[inline(always)]
unsafe fn read_register(address: u32) -> u32 {
    core::ptr::read_volatile(address as *const u32)
}

unsafe fn write_register(address: u32, value: u32) {
    core::ptr::write_volatile(address as *mut u32, value);
}

fn interrupts_unmasked() -> bool {
    let primask: u32;
    let faultmask: u32;
    unsafe {
        core::arch::asm!("mrs {}, PRIMASK", out(reg) primask, options(nostack, preserves_flags));
        core::arch::asm!("mrs {}, FAULTMASK", out(reg) faultmask, options(nostack, preserves_flags));
    }
    primask & 1 == 0 && faultmask & 1 == 0
}

fn module_a_plan() -> IsolationPlan<5> {
    let mut plan = IsolationPlan::new(MODULE_A_ID, MODULE_A_ID);
    plan.add(IsolationRegion::code(
        core::ptr::addr_of!(__module_a_code_start) as u32,
        256,
    ))
    .unwrap();
    plan.add(IsolationRegion::stack(
        core::ptr::addr_of!(MODULE_A_STACK) as u32,
        256,
    ))
    .unwrap();
    plan.add(IsolationRegion::data(
        core::ptr::addr_of!(MODULE_A_DATA) as u32,
        256,
    ))
    .unwrap();
    plan.add(IsolationRegion::stack_guard(
        core::ptr::addr_of!(MODULE_A_GUARD) as u32,
        256,
    ))
    .unwrap();
    plan.add(IsolationRegion::peripheral(
        TIMER0_BASE,
        4096,
        Resource::Timer1.isolation_id(),
    ))
    .unwrap();
    plan
}

fn module_b_plan() -> IsolationPlan<3> {
    let mut plan = IsolationPlan::new(MODULE_B_ID, MODULE_B_ID);
    plan.add(IsolationRegion::code(
        core::ptr::addr_of!(__module_b_code_start) as u32,
        256,
    ))
    .unwrap();
    plan.add(IsolationRegion::stack(
        core::ptr::addr_of!(MODULE_B_STACK) as u32,
        256,
    ))
    .unwrap();
    plan.add(IsolationRegion::data(
        core::ptr::addr_of!(MODULE_B_DATA) as u32,
        256,
    ))
    .unwrap();
    plan
}

unsafe fn prepare_module_a(receipt: IsolationReceipt, stage: u32) {
    let plan = module_a_plan();
    core::ptr::addr_of_mut!(MODULE_A_MPU).write(MaybeUninit::new(
        ModuleMpuContext::from_isolation_plan(&plan, receipt).unwrap(),
    ));
    MODULE_A_CONTEXT
        .initialize_isolated(
            &mut *core::ptr::addr_of_mut!(MODULE_A_STACK.0),
            module_a,
            stage as usize,
            &*core::ptr::addr_of!(MODULE_A_MPU).cast::<ModuleMpuContext>(),
        )
        .unwrap();
    core::ptr::addr_of_mut!(MODULE_A_RECEIPT).write(MaybeUninit::new(receipt));
    let lease = ResourceLease::acquire_guard_isolated(Resource::Timer1, receipt).unwrap();
    core::ptr::addr_of_mut!(MODULE_A_LEASE).write(MaybeUninit::new(lease));
}

#[link_section = ".module_a_code"]
extern "C" fn module_a(stage: usize) -> ! {
    let mut stack_witness = [0x2468_ACE0u32; 4];
    unsafe {
        *stack_witness.get_unchecked_mut(stage & 3) ^= stage as u32;
        core::ptr::write_volatile(core::ptr::addr_of_mut!(MODULE_A_DATA.0[0]), 0xAAAA_2350);
        let timer = read_register(TIMER0_BASE + 0x28).wrapping_add(1);
        core::ptr::write_volatile(core::ptr::addr_of_mut!(MODULE_A_DATA.0[2]), timer);
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(MODULE_A_DATA.0[3]),
            *stack_witness.get_unchecked(stage & 3),
        );
        match stage {
            0 => {
                core::ptr::write_volatile(core::ptr::addr_of_mut!(MODULE_B_DATA.0[1]), 0xBAD0_0001)
            }
            1 => {
                let _ = core::ptr::read_volatile(TIMER1_BASE as *const u32);
            }
            2 => {
                core::ptr::write_volatile(core::ptr::addr_of_mut!(MODULE_A_GUARD.0[0]), 0xBAD0_0002)
            }
            _ => {
                let address = (core::ptr::addr_of!(MODULE_A_DATA) as usize) | 1;
                let execute_data: extern "C" fn() = core::mem::transmute(address);
                execute_data();
            }
        }
    }
    loop {
        unsafe { core::arch::asm!("wfi", options(nostack)) };
    }
}

#[link_section = ".module_b_code"]
extern "C" fn module_b(_: usize) -> ! {
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(MODULE_B_DATA.0[0]), 0xBBBB_2350);
    }
    loop {
        unsafe { core::arch::asm!("svc 0", options(nostack)) };
        if unsafe { core::ptr::read_volatile(core::ptr::addr_of!(MODULE_B_DATA.0[2])) } != 0 {
            loop {
                unsafe { core::arch::asm!("wfi", options(nostack)) };
            }
        }
    }
}

#[exception]
fn MemoryManagement() {
    unsafe {
        let psp: u32;
        core::arch::asm!("mrs {}, psp", out(reg) psp, options(nostack, preserves_flags));
        let stacked_pc = core::ptr::read_volatile((psp + 24) as *const u32);
        let fault = capture_active_mpu_fault(read_register(CFSR), read_register(MMFAR), stacked_pc);
        let control: u32;
        core::arch::asm!("mrs {}, control", out(reg) control, options(nostack, preserves_flags));
        let stage = STAGE.load(Ordering::Acquire);
        let expected = match stage {
            0 => core::ptr::addr_of!(MODULE_B_DATA) as u32 + 4,
            1 => TIMER1_BASE,
            2 => core::ptr::addr_of!(MODULE_A_GUARD) as u32,
            _ => core::ptr::addr_of!(MODULE_A_DATA) as u32,
        };
        let exact = fault.module_code == u32::from(MODULE_A_ID)
            && fault.context_generation == stage + 1
            && fault.isolation_faulted
            && if stage == 3 {
                fault.instruction_access && (fault.stacked_pc & !1) == expected
            } else {
                fault.data_access && fault.fault_address_valid && fault.fault_address == expected
            };
        if exact {
            FAULT_MASK.fetch_or(1 << stage, Ordering::AcqRel);
        }
        FAULTS.fetch_add(1, Ordering::AcqRel);
        LAST_MODULE.store(fault.module_code, Ordering::Release);
        LAST_ADDRESS.store(fault.fault_address, Ordering::Release);
        LAST_PC.store(fault.stacked_pc, Ordering::Release);
        FAULT_CONTROL.store(control, Ordering::Release);

        let lease = &*core::ptr::addr_of!(MODULE_A_LEASE).cast::<LeaseGuard>();
        if lease.ensure_live() == Err(LeaseError::IsolationStale) {
            LEASE_STALE.fetch_add(1, Ordering::AcqRel);
        }
        if ResourceLease::recover_owner(MODULE_A_ID as u8).released(Resource::Timer1) {
            LEASE_RECOVERIES.fetch_add(1, Ordering::AcqRel);
        }
        write_register(CFSR, 0xFF);
        if CortexMSliceSwitch::switch(&MODULE_A_CONTEXT, &MODULE_B_CONTEXT).is_err() {
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!(NOBRO_RP2350_ISOLATION_REPORT.diagnostic),
                0xDEAD_0001,
            );
        }
    }
}

#[exception]
fn SVCall() {
    unsafe {
        let stage = STAGE.load(Ordering::Acquire);
        if CortexMSliceSwitch::switch(&MODULE_B_CONTEXT, &MODULE_A_CONTEXT)
            == Err(nobro_hal::ContextSwitchError::IsolationStale)
        {
            STALE_REJECTIONS.fetch_add(1, Ordering::AcqRel);
        }
        if stage < 3 {
            let old = core::ptr::read_volatile(
                core::ptr::addr_of!(MODULE_A_RECEIPT).cast::<IsolationReceipt>(),
            );
            MODULE_A_EPOCH.begin_recovery(old).unwrap();
            let next = MODULE_A_EPOCH.restart(old, capabilities()).unwrap();
            let next_stage = stage + 1;
            prepare_module_a(next, next_stage);
            STAGE.store(next_stage, Ordering::Release);
            CortexMSliceSwitch::switch(&MODULE_B_CONTEXT, &MODULE_A_CONTEXT).unwrap();
            return;
        }

        let control: u32;
        core::arch::asm!("mrs {}, control", out(reg) control, options(nostack, preserves_flags));
        let faults = FAULTS.load(Ordering::Acquire);
        let fault_mask = FAULT_MASK.load(Ordering::Acquire);
        let stale = STALE_REJECTIONS.load(Ordering::Acquire);
        let lease_stale = LEASE_STALE.load(Ordering::Acquire);
        let recoveries = LEASE_RECOVERIES.load(Ordering::Acquire);
        let receipt = core::ptr::read_volatile(
            core::ptr::addr_of!(MODULE_A_RECEIPT).cast::<IsolationReceipt>(),
        );
        let switched = CortexMSliceSwitch::current_record_address();
        let a = core::ptr::read_volatile(core::ptr::addr_of!(MODULE_A_DATA.0[0]));
        let b = core::ptr::read_volatile(core::ptr::addr_of!(MODULE_B_DATA.0[0]));
        let denied = core::ptr::read_volatile(core::ptr::addr_of!(MODULE_B_DATA.0[1]));
        let timer = core::ptr::read_volatile(core::ptr::addr_of!(MODULE_A_DATA.0[2]));
        let stack = core::ptr::read_volatile(core::ptr::addr_of!(MODULE_A_DATA.0[3]));
        let pass = faults == 4
            && fault_mask == 0xF
            && LAST_MODULE.load(Ordering::Acquire) == u32::from(MODULE_A_ID)
            && control & 1 == 1
            && FAULT_CONTROL.load(Ordering::Acquire) & 1 == 1
            && stale == 4
            && lease_stale == 4
            && recoveries == 4
            && receipt.context_generation() == 4
            && switched == core::ptr::addr_of!(MODULE_B_CONTEXT) as u32
            && a == 0xAAAA_2350
            && b == 0xBBBB_2350
            && denied == 0
            && timer != 0
            && stack != 0;
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_RP2350_ISOLATION_REPORT),
            Report {
                magic: MAGIC,
                version: VERSION,
                completed: 1,
                all_pass: u32::from(pass),
                faults,
                fault_mask,
                last_module: LAST_MODULE.load(Ordering::Acquire),
                last_address: LAST_ADDRESS.load(Ordering::Acquire),
                last_pc: LAST_PC.load(Ordering::Acquire),
                fault_control: FAULT_CONTROL.load(Ordering::Acquire),
                final_control: control,
                stale_rejections: stale,
                lease_stale,
                lease_recoveries: recoveries,
                final_generation: receipt.context_generation(),
                switched_record: switched,
                module_a_value: a,
                module_b_value: b,
                denied_write_value: denied,
                allowed_peripheral: timer,
                allowed_stack: stack,
                diagnostic: 0,
            },
        );
        core::ptr::write_volatile(core::ptr::addr_of_mut!(MODULE_B_DATA.0[2]), 1);
    }
}

#[exception]
unsafe fn HardFault(frame: &cortex_m_rt::ExceptionFrame) -> ! {
    core::ptr::write_volatile(
        core::ptr::addr_of_mut!(NOBRO_RP2350_ISOLATION_REPORT.diagnostic),
        0xDEAD_0000 | (frame.pc() & 0xFFFF),
    );
    loop {
        core::arch::asm!("bkpt");
    }
}

#[hal::entry]
fn main() -> ! {
    unsafe {
        let plan_a = module_a_plan();
        let receipt_a = MODULE_A_EPOCH.admit(&plan_a, capabilities()).unwrap();
        prepare_module_a(receipt_a, 0);

        let plan_b = module_b_plan();
        let receipt_b = MODULE_B_EPOCH.admit(&plan_b, capabilities()).unwrap();
        core::ptr::addr_of_mut!(MODULE_B_MPU).write(MaybeUninit::new(
            ModuleMpuContext::from_isolation_plan(&plan_b, receipt_b).unwrap(),
        ));
        MODULE_B_CONTEXT
            .initialize_isolated(
                &mut *core::ptr::addr_of_mut!(MODULE_B_STACK.0),
                module_b,
                0,
                &*core::ptr::addr_of!(MODULE_B_MPU).cast::<ModuleMpuContext>(),
            )
            .unwrap();

        if !interrupts_unmasked() {
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!(NOBRO_RP2350_ISOLATION_REPORT.diagnostic),
                0xDEAD_0002,
            );
            loop {
                core::arch::asm!("wfi", options(nostack));
            }
        }
        CortexMSliceSwitch::start_raw(&MODULE_A_CONTEXT, 7, 0x80).unwrap();
    }
    loop {
        cortex_m::asm::wfi();
    }
}
