//! Phase 1 resource scheduling demo — scenes A~D with autonomous self-evaluation.
//! Target: board1 ProMicro no-SD @ 0x1000, J-Link + RAM report (no scope required).

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::asm;
use defmt_rtt as _;
use panic_probe as _;

use airon_hal::{
    board,
    bus::TwimBus,
    deadline_timer::DeadlineTimer,
    inspect,
    lease::{LeaseError, Resource, ResourceLease},
    ppi,
    pwm::{PwmServo, SERVO_PIN},
    radio_sim::RadioRxSim,
    timer::MicroTimer,
};
use airon_kernel::{
    eval::{EvalGate, EvalReport, EVAL_MAGIC, MIN_DEADLINE_TICKS},
    executor::{I2cPollTask, Poll, StatsTask, Task},
    scheduler::Scheduler,
};

const OWNER_TIMER: u8 = 1;
const OWNER_I2C: u8 = 2;
const OWNER_RADIO: u8 = 3;

static I2C_READS: AtomicU32 = AtomicU32::new(0);
static SCENE_B_PASS: AtomicU32 = AtomicU32::new(0);
static EVAL_DONE: AtomicU32 = AtomicU32::new(0);

/// Host reads this struct via J-Link (`run-phase1-eval.ps1`).
#[no_mangle]
#[used]
static mut AIRON_EVAL_REPORT: EvalReport = EvalReport::zeroed();

fn on_deadline_slot() {}

#[cortex_m_rt::entry]
fn main() -> ! {
    defmt::info!("AIRON resource_sched_demo — autonomous eval mode");

    ResourceLease::acquire(Resource::Timer0, OWNER_TIMER)
        .unwrap_or_else(|_| defmt::panic!("Timer0 lease failed"));
    ResourceLease::acquire(Resource::Radio, OWNER_RADIO)
        .unwrap_or_else(|_| defmt::panic!("Radio lease failed"));

    unsafe {
        MicroTimer::init();
        DeadlineTimer::init();
        Scheduler::set_deadline_handler(on_deadline_slot);
        RadioRxSim::init();
        ppi::led_init_output();

        let pwm = PwmServo::init_50hz(board::SERVO_PWM_PIN, board::SERVO_CENTER_US);
        defmt::info!(
            "init PWM {}Hz pin {} pulse {}us",
            pwm.frequency_hz(),
            SERVO_PIN,
            pwm.pulse_us()
        );
        core::mem::forget(pwm);
    }

    let twim0 = TwimBus::new_twim0(OWNER_I2C).unwrap_or_else(|_| defmt::panic!("TWIM0 lease"));
    scene_b_check_once();

    let now = MicroTimer::now_us();
    let mut i2c_task = I2cPollTask::new(OWNER_I2C, now);
    let mut stats = StatsTask::new(now);
    let mut i2c_buf = [0u8; 16];

    Scheduler::reset_stats();

    unsafe {
        AIRON_EVAL_REPORT.magic = EVAL_MAGIC;
        AIRON_EVAL_REPORT.version = airon_kernel::eval::EVAL_VERSION;
    }

    loop {
        let now = MicroTimer::now_us();
        poll_deadline_timer();

        if i2c_task.poll(now) == Poll::Ready {
            if twim0.read_stub(0x68, &mut i2c_buf).is_ok() {
                I2C_READS.fetch_add(1, Ordering::AcqRel);
            }
        }

        if stats.poll(now) == Poll::Ready {
            poll_deadline_timer();
            for _ in 0..4 {
                unsafe {
                    let _ = RadioRxSim::trigger_and_latency_us();
                }
            }
            poll_deadline_timer();
            try_finalize_eval();
        }

        if EVAL_DONE.load(Ordering::Acquire) != 0 {
            unsafe {
                ppi::led_toggle();
            }
            asm::delay(8_000_000);
        } else {
            asm::nop();
        }
    }
}

fn poll_deadline_timer() {
    unsafe {
        let t = nrf52840_pac::TIMER1::ptr();
        if (*t).events_compare[0].read().bits() != 0 {
            (*t).events_compare[0].reset();
            Scheduler::on_deadline_tick(MicroTimer::now_us());
        }
    }
}

fn scene_b_check_once() {
    match TwimBus::new_twim0(99) {
        Err(LeaseError::AlreadyHeld) => {
            SCENE_B_PASS.store(1, Ordering::Release);
            defmt::info!("scene B: TWIM0 AlreadyHeld — pass");
        }
        Ok(bus) => {
            defmt::warn!("scene B: unexpected second acquire");
            drop(bus);
        }
        Err(LeaseError::NotHeld) => defmt::warn!("scene B: NotHeld"),
        Err(LeaseError::WrongOwner) => defmt::warn!("scene B: WrongOwner"),
    }
}

fn try_finalize_eval() {
    if EVAL_DONE.load(Ordering::Acquire) != 0 {
        return;
    }

    let ticks = Scheduler::tick_count();
    let jitter = Scheduler::max_jitter_us();
    let misses = Scheduler::deadline_misses();
    let i2c_reads = I2C_READS.load(Ordering::Acquire);
    let (radio_max, radio_samples) = RadioRxSim::latency_stats();
    let (scene_d, pwm_snap, parity) =
        unsafe { inspect::scene_d_pass(board::SERVO_CENTER_US) };

    let scene_a = EvalGate::scene_a_pass(jitter, misses, ticks, i2c_reads);
    let scene_b = SCENE_B_PASS.load(Ordering::Acquire) != 0;
    let scene_c = EvalGate::scene_c_pass(radio_max, radio_samples);

    write_progress_report(
        scene_a,
        scene_b,
        scene_c,
        scene_d,
        jitter,
        misses,
        ticks,
        i2c_reads,
        radio_max,
        radio_samples,
        &pwm_snap,
        &parity,
    );

    if ticks < MIN_DEADLINE_TICKS {
        return;
    }

    defmt::info!(
        "eval progress A={} B={} C={} D={} jitter={} radio_lat={} i2c={}",
        scene_a,
        scene_b,
        scene_c,
        scene_d,
        jitter,
        radio_max,
        i2c_reads
    );

    if !(scene_a && scene_b && scene_c && scene_d) {
        return;
    }

    let mut report = EvalReport {
        scene_a_pass: 1,
        scene_a_max_jitter_us: jitter,
        scene_a_ticks: ticks,
        scene_a_misses: misses,
        scene_a_i2c_reads: i2c_reads,
        scene_b_pass: 1,
        scene_c_pass: 1,
        scene_c_max_latency_us: radio_max,
        scene_c_samples: radio_samples,
        scene_d_pass: 1,
        scene_d_pwm_hz: pwm_snap.frequency_hz,
        scene_d_pin: u32::from(pwm_snap.out_pin),
        scene_d_flash_start: parity.flash_start,
        ..EvalReport::zeroed()
    };
    report.seal();

    unsafe {
        AIRON_EVAL_REPORT = report;
    }
    EVAL_DONE.store(1, Ordering::Release);

    defmt::info!(
        "AIRON_EVAL_FINAL magic=0x{:08X} ALL=1 A=1 B=1 C=1 D=1 jitter={} radio_lat={} ticks={}",
        EVAL_MAGIC,
        jitter,
        radio_max,
        ticks
    );
}

fn write_progress_report(
    scene_a: bool,
    scene_b: bool,
    scene_c: bool,
    scene_d: bool,
    jitter: u32,
    misses: u32,
    ticks: u32,
    i2c_reads: u32,
    radio_max: u32,
    radio_samples: u32,
    pwm_snap: &airon_hal::PwmSnapshot,
    parity: &airon_hal::BoardParity,
) {
    let mut report = EvalReport {
        scene_a_pass: u32::from(scene_a),
        scene_a_max_jitter_us: jitter,
        scene_a_ticks: ticks,
        scene_a_misses: misses,
        scene_a_i2c_reads: i2c_reads,
        scene_b_pass: u32::from(scene_b),
        scene_c_pass: u32::from(scene_c),
        scene_c_max_latency_us: radio_max,
        scene_c_samples: radio_samples,
        scene_d_pass: u32::from(scene_d),
        scene_d_pwm_hz: pwm_snap.frequency_hz,
        scene_d_pin: u32::from(pwm_snap.out_pin),
        scene_d_flash_start: parity.flash_start,
        ..EvalReport::zeroed()
    };
    report.magic = EVAL_MAGIC;
    report.version = airon_kernel::eval::EVAL_VERSION;
    unsafe {
        AIRON_EVAL_REPORT = report;
    }
}
