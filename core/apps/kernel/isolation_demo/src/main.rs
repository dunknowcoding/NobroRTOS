//! Deep-target module isolation demonstration.
//!
//! Two PSP contexts have separate writable stack/data regions and share only
//! executable flash. Module A deliberately writes Module B's data, producing
//! an attributed MemManage fault. The handler switches to Module B; PendSV
//! installs B's MPU bank and returns unprivileged. B proves its own data is
//! writable and reports completion through SVC.
#![no_main]
#![no_std]

use core::mem::MaybeUninit;

use cortex_m_rt::{entry, exception};
use defmt_rtt as _;
use nobro_hal::{
    mpu::{KernelMpuPlan, ModuleMpuContext, MpuFaultRecord, MpuRegionSpec},
    ContextRecord, CortexMSliceSwitch, PriorityCeiling,
};
use nobro_kernel::{module_code, ModuleId};
use panic_halt as _;

const MAGIC: u32 = 0x4E49_534F; // "NISO"
const VERSION: u32 = 1;
const CFSR: u32 = 0xE000_ED28;
const MMFAR: u32 = 0xE000_ED34;
const VTOR: u32 = 0xE000_ED08;
const APP_BASE: u32 = 0x1000;

#[repr(C)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    faults: u32,
    fault_module: u32,
    fault_address: u32,
    fault_control: u32,
    final_control: u32,
    switched_record: u32,
    module_a_value: u32,
    module_b_value: u32,
    diagnostic_checksum: u32,
}

#[no_mangle]
#[used]
static mut NOBRO_ISOLATION_REPORT: Report = Report {
    magic: MAGIC,
    version: VERSION,
    completed: 0,
    all_pass: 0,
    faults: 0,
    fault_module: 0,
    fault_address: 0,
    fault_control: 0,
    final_control: 0,
    switched_record: 0,
    module_a_value: 0,
    module_b_value: 0,
    diagnostic_checksum: 0,
};

#[repr(align(256))]
struct Region([u32; 64]);

#[repr(align(256))]
struct Stack([u8; 256]);

static mut MODULE_A_DATA: Region = Region([0; 64]);
static mut MODULE_B_DATA: Region = Region([0; 64]);
static mut MODULE_A_STACK: Stack = Stack([0; 256]);
static mut MODULE_B_STACK: Stack = Stack([0; 256]);
static MODULE_A_CONTEXT: ContextRecord = ContextRecord::empty();
static MODULE_B_CONTEXT: ContextRecord = ContextRecord::empty();
static mut MODULE_A_MPU: MaybeUninit<ModuleMpuContext> = MaybeUninit::uninit();
static mut MODULE_B_MPU: MaybeUninit<ModuleMpuContext> = MaybeUninit::uninit();

unsafe fn read_register(address: u32) -> u32 {
    core::ptr::read_volatile(address as *const u32)
}

unsafe fn write_register(address: u32, value: u32) {
    core::ptr::write_volatile(address as *mut u32, value);
}

extern "C" fn module_a(_: usize) -> ! {
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(MODULE_A_DATA.0[0]), 0xAAAA_0001);
        // Not present in A's MPU bank: this must fault before the write lands.
        core::ptr::write_volatile(core::ptr::addr_of_mut!(MODULE_B_DATA.0[1]), 0xBAD0_0001);
    }
    loop {
        cortex_m::asm::wfi();
    }
}

extern "C" fn module_b(_: usize) -> ! {
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(MODULE_B_DATA.0[0]), 0xBBBB_0002);
        core::arch::asm!("svc 0", options(nostack));
    }
    loop {
        cortex_m::asm::wfi();
    }
}

#[exception]
fn MemoryManagement() {
    unsafe {
        let cfsr = read_register(CFSR);
        let mmfar = read_register(MMFAR);
        let control: u32;
        core::arch::asm!("mrs {}, control", out(reg) control, options(nostack, preserves_flags));
        let psp: u32;
        core::arch::asm!("mrs {}, psp", out(reg) psp, options(nostack, preserves_flags));
        let stacked_pc = core::ptr::read_volatile((psp + 24) as *const u32);
        let fault = MpuFaultRecord::decode_mem_manage(
            cfsr,
            mmfar,
            stacked_pc,
            module_code(ModuleId::Sensor),
        );
        core::ptr::write_volatile(core::ptr::addr_of_mut!(NOBRO_ISOLATION_REPORT.faults), 1);
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_ISOLATION_REPORT.fault_module),
            fault.module_code,
        );
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_ISOLATION_REPORT.fault_address),
            fault.fault_address,
        );
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_ISOLATION_REPORT.fault_control),
            control,
        );
        write_register(CFSR, 0xFF);
        if CortexMSliceSwitch::switch(&MODULE_A_CONTEXT, &MODULE_B_CONTEXT).is_err() {
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!(NOBRO_ISOLATION_REPORT.diagnostic_checksum),
                0xDEAD_0001,
            );
        }
    }
}

#[exception]
fn SVCall() {
    unsafe {
        let control: u32;
        core::arch::asm!("mrs {}, control", out(reg) control, options(nostack, preserves_flags));
        let fault_address =
            core::ptr::read_volatile(core::ptr::addr_of!(NOBRO_ISOLATION_REPORT.fault_address));
        let module_a_value = core::ptr::read_volatile(core::ptr::addr_of!(MODULE_A_DATA.0[0]));
        let module_b_value = core::ptr::read_volatile(core::ptr::addr_of!(MODULE_B_DATA.0[0]));
        let switched_record = CortexMSliceSwitch::current_record_address();
        let expected_fault = core::ptr::addr_of!(MODULE_B_DATA) as u32 + 4;
        let pass = core::ptr::read_volatile(core::ptr::addr_of!(NOBRO_ISOLATION_REPORT.faults))
            == 1
            && core::ptr::read_volatile(core::ptr::addr_of!(NOBRO_ISOLATION_REPORT.fault_module))
                == module_code(ModuleId::Sensor)
            && fault_address == expected_fault
            && control & 1 == 1
            && switched_record == core::ptr::addr_of!(MODULE_B_CONTEXT) as u32
            && module_a_value == 0xAAAA_0001
            && module_b_value == 0xBBBB_0002
            && core::ptr::read_volatile(core::ptr::addr_of!(MODULE_B_DATA.0[1])) == 0;
        let all_pass = u32::from(pass);
        let diagnostic_checksum = MAGIC
            ^ VERSION
            ^ 1
            ^ all_pass
            ^ 1
            ^ module_code(ModuleId::Sensor)
            ^ fault_address
            ^ core::ptr::read_volatile(core::ptr::addr_of!(NOBRO_ISOLATION_REPORT.fault_control))
            ^ control
            ^ switched_record
            ^ module_a_value
            ^ module_b_value;
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_ISOLATION_REPORT),
            Report {
                magic: MAGIC,
                version: VERSION,
                completed: 1,
                all_pass,
                faults: 1,
                fault_module: module_code(ModuleId::Sensor),
                fault_address,
                fault_control: core::ptr::read_volatile(core::ptr::addr_of!(
                    NOBRO_ISOLATION_REPORT.fault_control
                )),
                final_control: control,
                switched_record,
                module_a_value,
                module_b_value,
                diagnostic_checksum,
            },
        );
    }
}

#[exception]
unsafe fn HardFault(frame: &cortex_m_rt::ExceptionFrame) -> ! {
    core::ptr::write_volatile(
        core::ptr::addr_of_mut!(NOBRO_ISOLATION_REPORT.diagnostic_checksum),
        0xDEAD_0000 | (frame.pc() & 0xFFFF),
    );
    loop {
        cortex_m::asm::bkpt();
    }
}

#[entry]
fn main() -> ! {
    unsafe {
        write_register(VTOR, APP_BASE);
        core::arch::asm!("dsb", "isb", options(nostack, preserves_flags));

        let mut plan_a = KernelMpuPlan::<3>::new(false);
        plan_a.add(MpuRegionSpec::code(0, 1024 * 1024)).unwrap();
        plan_a
            .add(MpuRegionSpec::ram(
                core::ptr::addr_of!(MODULE_A_STACK) as u32,
                256,
            ))
            .unwrap();
        plan_a
            .add(MpuRegionSpec::ram(
                core::ptr::addr_of!(MODULE_A_DATA) as u32,
                256,
            ))
            .unwrap();
        core::ptr::addr_of_mut!(MODULE_A_MPU).write(MaybeUninit::new(
            ModuleMpuContext::from_plan(&plan_a).unwrap(),
        ));

        let mut plan_b = KernelMpuPlan::<3>::new(false);
        plan_b.add(MpuRegionSpec::code(0, 1024 * 1024)).unwrap();
        plan_b
            .add(MpuRegionSpec::ram(
                core::ptr::addr_of!(MODULE_B_STACK) as u32,
                256,
            ))
            .unwrap();
        plan_b
            .add(MpuRegionSpec::ram(
                core::ptr::addr_of!(MODULE_B_DATA) as u32,
                256,
            ))
            .unwrap();
        core::ptr::addr_of_mut!(MODULE_B_MPU).write(MaybeUninit::new(
            ModuleMpuContext::from_plan(&plan_b).unwrap(),
        ));

        MODULE_A_CONTEXT
            .initialize_isolated(
                &mut *core::ptr::addr_of_mut!(MODULE_A_STACK.0),
                module_a,
                0,
                &*core::ptr::addr_of!(MODULE_A_MPU).cast::<ModuleMpuContext>(),
            )
            .unwrap();
        MODULE_B_CONTEXT
            .initialize_isolated(
                &mut *core::ptr::addr_of_mut!(MODULE_B_STACK.0),
                module_b,
                0,
                &*core::ptr::addr_of!(MODULE_B_MPU).cast::<ModuleMpuContext>(),
            )
            .unwrap();
        // Establish the application's interrupt state explicitly. This also
        // makes a debugger vector launch independent of a bootloader that
        // halted with PRIMASK set.
        cortex_m::interrupt::enable();
        CortexMSliceSwitch::start(&MODULE_A_CONTEXT, 7, PriorityCeiling::NRF52840_BARE).unwrap();
    }

    loop {
        cortex_m::asm::wfi();
    }
}
