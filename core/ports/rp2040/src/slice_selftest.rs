//! RP2040 Cortex-M0+ forced-suspension and recovery self-test.
//!
//! TIMER alarm 0 interrupts a non-yielding PSP task twice. Each interrupt
//! requests PendSV at the admitted lowest priority: first into a recovery
//! context, then back into that saved recovery context after a deliberate
//! return to the offender. A debugger reads the fixed report; no serial or
//! heap path participates in the proof.

#![no_main]
#![no_std]

use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::peripheral::NVIC;
use fugit::ExtU32;
use nobro_hal::{CortexM0ContextRecord, CortexM0SliceSwitch};
use nobro_kernel::{
    admit_scheduling, SchedulingCapabilities, SchedulingProfile, SchedulingRequest,
};
use panic_halt as _;
use rp2040_hal as hal;

#[link_section = ".boot2"]
#[used]
pub static BOOT2: [u8; 256] = rp2040_boot2::BOOT_LOADER_W25Q080;

const MAGIC: u32 = 0x4e53_4c30; // "NSL0"
const VERSION: u32 = 2;
const ALARM_DELAY_US: u32 = 1_000;
const TIMER_PRIORITY_RAW: u8 = 1 << 6;
const PENDSV_LOGICAL_PRIORITY: u8 = 3;

#[repr(C)]
pub struct SliceSelftestReport {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    alarm_count: u32,
    switch_errors: u32,
    first_lateness_us: u32,
    second_lateness_us: u32,
    offender_spins: u32,
    recovery_resumes: u32,
    current_record: u32,
    profile_admitted: u32,
    diagnostic_checksum: u32,
}

#[no_mangle]
#[used]
pub static mut NOBRO_RP2040_SLICE_REPORT: SliceSelftestReport = SliceSelftestReport {
    magic: MAGIC,
    version: VERSION,
    completed: 0,
    all_pass: 0,
    alarm_count: 0,
    switch_errors: 0,
    first_lateness_us: 0,
    second_lateness_us: 0,
    offender_spins: 0,
    recovery_resumes: 0,
    current_record: 0,
    profile_admitted: 0,
    diagnostic_checksum: 0,
};

#[repr(align(8))]
struct Stack([u8; 256]);

static mut OFFENDER_STACK: Stack = Stack([0; 256]);
static mut RECOVERY_STACK: Stack = Stack([0; 256]);
static OFFENDER_CONTEXT: CortexM0ContextRecord = CortexM0ContextRecord::empty();
static RECOVERY_CONTEXT: CortexM0ContextRecord = CortexM0ContextRecord::empty();
static PHASE: AtomicU32 = AtomicU32::new(0);
static ALARM_DEADLINE: AtomicU32 = AtomicU32::new(0);
static ALARM_COUNT: AtomicU32 = AtomicU32::new(0);
static SWITCH_ERRORS: AtomicU32 = AtomicU32::new(0);
static FIRST_LATENESS_US: AtomicU32 = AtomicU32::new(0);
static SECOND_LATENESS_US: AtomicU32 = AtomicU32::new(0);
static OFFENDER_SPINS: AtomicU32 = AtomicU32::new(0);
static PROFILE_ADMITTED: AtomicU32 = AtomicU32::new(0);

fn timer() -> &'static hal::pac::timer::RegisterBlock {
    unsafe { &*hal::pac::TIMER::ptr() }
}

fn now_us() -> u32 {
    timer().timerawl().read().bits()
}

fn arm_alarm(delay_us: u32) {
    let deadline = now_us().wrapping_add(delay_us);
    ALARM_DEADLINE.store(deadline, Ordering::Release);
    timer().intr().write(|w| w.alarm_0().clear_bit_by_one());
    timer().alarm0().write(|w| unsafe { w.bits(deadline) });
}

extern "C" fn offender(_: usize) -> ! {
    loop {
        OFFENDER_SPINS.store(
            OFFENDER_SPINS.load(Ordering::Relaxed).wrapping_add(1),
            Ordering::Relaxed,
        );
        core::hint::spin_loop();
    }
}

extern "C" fn recovery(_: usize) -> ! {
    // First forced entry. Re-arm the exact 1 us-resolution provider, return to
    // the saved offender once, and require the next alarm to resume us here.
    PHASE.store(3, Ordering::Release);
    arm_alarm(ALARM_DELAY_US);
    if unsafe { CortexM0SliceSwitch::switch(&RECOVERY_CONTEXT, &OFFENDER_CONTEXT) }.is_err() {
        SWITCH_ERRORS.store(
            SWITCH_ERRORS.load(Ordering::Relaxed).wrapping_add(1),
            Ordering::Relaxed,
        );
    }

    let alarms = ALARM_COUNT.load(Ordering::Acquire);
    let errors = SWITCH_ERRORS.load(Ordering::Acquire);
    let spins = OFFENDER_SPINS.load(Ordering::Acquire);
    let current = CortexM0SliceSwitch::current_record_address();
    let profile_admitted = PROFILE_ADMITTED.load(Ordering::Acquire);
    let resumes = u32::from(PHASE.load(Ordering::Acquire) == 4);
    let first = FIRST_LATENESS_US.load(Ordering::Acquire);
    let second = SECOND_LATENESS_US.load(Ordering::Acquire);
    let pass = u32::from(
        alarms == 2
            && errors == 0
            && spins != 0
            && resumes == 1
            && profile_admitted == 1
            && current == core::ptr::addr_of!(RECOVERY_CONTEXT) as u32,
    );
    let diagnostic_checksum = MAGIC
        ^ VERSION
        ^ 1
        ^ pass
        ^ alarms
        ^ errors
        ^ first
        ^ second
        ^ spins
        ^ resumes
        ^ current
        ^ profile_admitted;
    // The architectural switch completed and the report no longer needs the
    // independent fallback. Disable it before parking this diagnostic image.
    unsafe {
        (*hal::pac::WATCHDOG::ptr())
            .ctrl()
            .write(|writer| writer.enable().clear_bit());
    }
    unsafe {
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_RP2040_SLICE_REPORT),
            SliceSelftestReport {
                magic: MAGIC,
                version: VERSION,
                completed: 1,
                all_pass: pass,
                alarm_count: alarms,
                switch_errors: errors,
                first_lateness_us: first,
                second_lateness_us: second,
                offender_spins: spins,
                recovery_resumes: resumes,
                current_record: current,
                profile_admitted,
                diagnostic_checksum,
            },
        );
    }
    loop {
        cortex_m::asm::wfe();
    }
}

#[no_mangle]
#[allow(non_snake_case)]
unsafe extern "C" fn TIMER_IRQ_0() {
    timer().intr().write(|w| w.alarm_0().clear_bit_by_one());
    let deadline = ALARM_DEADLINE.load(Ordering::Acquire);
    let lateness = now_us().wrapping_sub(deadline);
    let count = ALARM_COUNT.load(Ordering::Acquire).wrapping_add(1);
    ALARM_COUNT.store(count, Ordering::Release);
    if count == 1 {
        FIRST_LATENESS_US.store(lateness, Ordering::Release);
        PHASE.store(2, Ordering::Release);
    } else {
        SECOND_LATENESS_US.store(lateness, Ordering::Release);
        PHASE.store(4, Ordering::Release);
    }
    if CortexM0SliceSwitch::switch(&OFFENDER_CONTEXT, &RECOVERY_CONTEXT).is_err() {
        SWITCH_ERRORS.store(
            SWITCH_ERRORS.load(Ordering::Relaxed).wrapping_add(1),
            Ordering::Relaxed,
        );
    }
}

#[hal::entry]
fn main() -> ! {
    let mut pac = hal::pac::Peripherals::take().unwrap();
    let mut watchdog = hal::Watchdog::new(pac.WATCHDOG);
    let _clocks = hal::clocks::init_clocks_and_plls(
        12_000_000,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    )
    .unwrap();
    let scheduling = SchedulingRequest::cooperative()
        .profile(SchedulingProfile::ForcedPreemption)
        .priorities(4)
        .force_suspend(ALARM_DELAY_US, 50, 10_000);
    let capabilities = SchedulingCapabilities::cooperative(0x5250_4d30, 1, 4)
        .deadline_observation(1, 50)
        .async_priority()
        .forced_preemption(10, 10_000, false);
    PROFILE_ADMITTED.store(
        u32::from(admit_scheduling(scheduling, capabilities).is_ok()),
        Ordering::Release,
    );
    // If PendSV cannot complete, this independent peripheral resets the target
    // within the admitted containment bound. Recovery disables it after proof.
    watchdog.start(10_000.micros());

    unsafe {
        OFFENDER_CONTEXT
            .initialize(&mut *core::ptr::addr_of_mut!(OFFENDER_STACK.0), offender, 0)
            .unwrap();
        RECOVERY_CONTEXT
            .initialize(&mut *core::ptr::addr_of_mut!(RECOVERY_STACK.0), recovery, 0)
            .unwrap();
        let mut core = cortex_m::Peripherals::steal();
        core.NVIC
            .set_priority(hal::pac::Interrupt::TIMER_IRQ_0, TIMER_PRIORITY_RAW);
        NVIC::unpend(hal::pac::Interrupt::TIMER_IRQ_0);
        NVIC::unmask(hal::pac::Interrupt::TIMER_IRQ_0);
    }
    timer().inte().write(|w| w.alarm_0().set_bit());
    PHASE.store(1, Ordering::Release);
    arm_alarm(ALARM_DELAY_US);
    unsafe {
        CortexM0SliceSwitch::start(
            &OFFENDER_CONTEXT,
            PENDSV_LOGICAL_PRIORITY,
            PENDSV_LOGICAL_PRIORITY,
        )
        .unwrap();
    }
    loop {
        cortex_m::asm::wfe();
    }
}
