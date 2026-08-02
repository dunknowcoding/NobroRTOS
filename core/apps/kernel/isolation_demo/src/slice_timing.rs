//! nRF52840 high-resolution deadline plus timer-forced P-SLICE self-test.
//!
//! TIMER0 proves a qualified 1 us deadline/wake path. TIMER1 then interrupts a
//! non-yielding PSP context twice, with PendSV switching into and resuming a
//! recovery context at the admitted S140 application priorities. The S140
//! image is resident but dormant in this composition; active SoftDevice
//! integration remains a separate admission boundary.

#![no_main]
#![no_std]

use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::peripheral::NVIC;
use cortex_m_rt::entry;
use defmt_rtt as _;
use nobro_hal::{ContextRecord, CortexMSliceSwitch, NrfTimerPower, PriorityCeiling};
use nobro_power::{DeadlineTimingProfile, DeadlineTimingRequest, PowerMode, PowerPlatform};
use nrf52840_pac::{Interrupt, TIMER1};
use panic_halt as _;

const MAGIC: u32 = 0x4e53_4c34; // "NSL4"
const VERSION: u32 = 1;
const DEADLINE_DELAY_US: u32 = 1_000;
const SLICE_DELAY_US: u32 = 1_000;
const TIMER1_PRIORITY_RAW: u8 = 5 << 5;

#[repr(C)]
struct SliceTimingReport {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    deadline_releases: u32,
    deadline_lateness_us: u32,
    admitted_overhead_us: u32,
    slice_interrupts: u32,
    switch_errors: u32,
    offender_spins: u32,
    recovery_resumes: u32,
    current_record: u32,
    diagnostic_checksum: u32,
}

#[no_mangle]
#[used]
static mut NOBRO_NRF_SLICE_TIMING_REPORT: SliceTimingReport = SliceTimingReport {
    magic: MAGIC,
    version: VERSION,
    completed: 0,
    all_pass: 0,
    deadline_releases: 0,
    deadline_lateness_us: 0,
    admitted_overhead_us: 0,
    slice_interrupts: 0,
    switch_errors: 0,
    offender_spins: 0,
    recovery_resumes: 0,
    current_record: 0,
    diagnostic_checksum: 0,
};

#[repr(align(8))]
struct Stack([u8; 512]);

static mut OFFENDER_STACK: Stack = Stack([0; 512]);
static mut RECOVERY_STACK: Stack = Stack([0; 512]);
static OFFENDER_CONTEXT: ContextRecord = ContextRecord::empty();
static RECOVERY_CONTEXT: ContextRecord = ContextRecord::empty();
static PHASE: AtomicU32 = AtomicU32::new(0);
static SLICE_DEADLINE: AtomicU32 = AtomicU32::new(0);
static SLICE_INTERRUPTS: AtomicU32 = AtomicU32::new(0);
static SWITCH_ERRORS: AtomicU32 = AtomicU32::new(0);
static OFFENDER_SPINS: AtomicU32 = AtomicU32::new(0);
static DEADLINE_RELEASES: AtomicU32 = AtomicU32::new(0);
static DEADLINE_LATENESS: AtomicU32 = AtomicU32::new(0);
static ADMITTED_OVERHEAD: AtomicU32 = AtomicU32::new(0);

fn timer1_now_us() -> u32 {
    unsafe {
        let timer = TIMER1::ptr();
        (*timer).tasks_capture[0].write(|w| w.bits(1));
        (*timer).cc[0].read().bits()
    }
}

fn arm_slice(delay_us: u32) {
    unsafe {
        let timer = TIMER1::ptr();
        let deadline = timer1_now_us().wrapping_add(delay_us);
        SLICE_DEADLINE.store(deadline, Ordering::Release);
        (*timer).events_compare[3].reset();
        (*timer).cc[3].write(|w| w.bits(deadline));
        (*timer).intenset.write(|w| w.compare3().set_bit());
    }
}

extern "C" fn offender(_: usize) -> ! {
    loop {
        OFFENDER_SPINS.fetch_add(1, Ordering::Relaxed);
        core::hint::spin_loop();
    }
}

extern "C" fn recovery(_: usize) -> ! {
    PHASE.store(3, Ordering::Release);
    arm_slice(SLICE_DELAY_US);
    if unsafe { CortexMSliceSwitch::switch(&RECOVERY_CONTEXT, &OFFENDER_CONTEXT) }.is_err() {
        SWITCH_ERRORS.fetch_add(1, Ordering::Relaxed);
    }

    let deadline_releases = DEADLINE_RELEASES.load(Ordering::Acquire);
    let deadline_lateness = DEADLINE_LATENESS.load(Ordering::Acquire);
    let admitted_overhead = ADMITTED_OVERHEAD.load(Ordering::Acquire);
    let slice_interrupts = SLICE_INTERRUPTS.load(Ordering::Acquire);
    let switch_errors = SWITCH_ERRORS.load(Ordering::Acquire);
    let spins = OFFENDER_SPINS.load(Ordering::Acquire);
    let recovery_resumes = u32::from(PHASE.load(Ordering::Acquire) == 4);
    let current = CortexMSliceSwitch::current_record_address();
    let pass = u32::from(
        deadline_releases == 1
            && deadline_lateness <= 50
            && admitted_overhead <= 54
            && slice_interrupts == 2
            && switch_errors == 0
            && spins != 0
            && recovery_resumes == 1
            && current == core::ptr::addr_of!(RECOVERY_CONTEXT) as u32,
    );
    let diagnostic_checksum = MAGIC
        ^ VERSION
        ^ 1
        ^ pass
        ^ deadline_releases
        ^ deadline_lateness
        ^ admitted_overhead
        ^ slice_interrupts
        ^ switch_errors
        ^ spins
        ^ recovery_resumes
        ^ current;
    unsafe {
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(NOBRO_NRF_SLICE_TIMING_REPORT),
            SliceTimingReport {
                magic: MAGIC,
                version: VERSION,
                completed: 1,
                all_pass: pass,
                deadline_releases,
                deadline_lateness_us: deadline_lateness,
                admitted_overhead_us: admitted_overhead,
                slice_interrupts,
                switch_errors,
                offender_spins: spins,
                recovery_resumes,
                current_record: current,
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
unsafe extern "C" fn TIMER1() {
    (*TIMER1::ptr()).events_compare[3].reset();
    let count = SLICE_INTERRUPTS.fetch_add(1, Ordering::AcqRel) + 1;
    if count == 1 {
        PHASE.store(2, Ordering::Release);
    } else {
        PHASE.store(4, Ordering::Release);
    }
    let _lateness = timer1_now_us().wrapping_sub(SLICE_DEADLINE.load(Ordering::Acquire));
    if CortexMSliceSwitch::switch(&OFFENDER_CONTEXT, &RECOVERY_CONTEXT).is_err() {
        SWITCH_ERRORS.fetch_add(1, Ordering::Relaxed);
    }
}

#[entry]
fn main() -> ! {
    let profile = DeadlineTimingProfile {
        provider_id: NrfTimerPower::DEADLINE_PROVIDER_ID,
        generation: 1,
        minimum_period_us: 2,
        resolution_us: 1,
        programming_overhead_us: 2,
        interrupt_overhead_us: 2,
        wake_latency_us: 50,
    };
    let admission = profile
        .admit(DeadlineTimingRequest::exact(DEADLINE_DELAY_US, 54))
        .unwrap();
    let mut power = unsafe { NrfTimerPower::init() };
    power.qualify_deadline_timing(profile).unwrap();
    let deadline = NrfTimerPower::now_us().wrapping_add(u64::from(DEADLINE_DELAY_US));
    power.program_deadline_release(Some(deadline), 1).unwrap();
    power.enter(PowerMode::Idle).unwrap();
    let now = NrfTimerPower::now_us();
    let releases = power.take_deadline_releases(now);
    DEADLINE_RELEASES.store(releases, Ordering::Release);
    DEADLINE_LATENESS.store(now.wrapping_sub(deadline) as u32, Ordering::Release);
    ADMITTED_OVERHEAD.store(admission.total_overhead_us, Ordering::Release);

    unsafe {
        let timer = TIMER1::ptr();
        (*timer).tasks_stop.write(|w| w.bits(1));
        (*timer).tasks_clear.write(|w| w.bits(1));
        (*timer).mode.write(|w| w.mode().timer());
        (*timer).bitmode.write(|w| w.bitmode()._32bit());
        (*timer).prescaler.write(|w| w.prescaler().bits(4));
        (*timer).events_compare[3].reset();
        (*timer).tasks_start.write(|w| w.bits(1));
        let mut core = cortex_m::Peripherals::steal();
        core.NVIC
            .set_priority(Interrupt::TIMER1, TIMER1_PRIORITY_RAW);
        NVIC::unpend(Interrupt::TIMER1);
        NVIC::unmask(Interrupt::TIMER1);
        OFFENDER_CONTEXT
            .initialize(&mut *core::ptr::addr_of_mut!(OFFENDER_STACK.0), offender, 0)
            .unwrap();
        RECOVERY_CONTEXT
            .initialize(&mut *core::ptr::addr_of_mut!(RECOVERY_STACK.0), recovery, 0)
            .unwrap();
    }
    PHASE.store(1, Ordering::Release);
    arm_slice(SLICE_DELAY_US);
    unsafe {
        CortexMSliceSwitch::start(&OFFENDER_CONTEXT, 7, PriorityCeiling::NRF52840_S140).unwrap();
    }
    loop {
        cortex_m::asm::wfe();
    }
}
