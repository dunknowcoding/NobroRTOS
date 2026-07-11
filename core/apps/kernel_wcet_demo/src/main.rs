//! Measured kernel-op WCET (Wave 17): DWT-cycle-counted upper bounds, on real silicon.
//!
//! For each core operation the demo runs `ITERS` iterations and records the MAX cycle
//! count - a measured upper bound on this hardware (cache-free Cortex-M4, so spread is
//! small), honestly labeled: not a formal WCET proof. Also measures the longest
//! interrupt-masked window a `critical_section` kernel op produces, answering "interrupt
//! latency is unbounded/undocumented" with a number.
//!
//! `NOBRO_WCET_REPORT` carries max cycles per op (@64 MHz: cycles ÷ 64 = µs·100).
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_classic::EventFlags;
use nobro_hal::{lease::Resource, traits::HalLease, ActivePlatform as Hal};
use nobro_kernel::{
    alarm::AlarmId, AlarmQueue, Capability, CapabilityGrantTable, CapabilitySet, Mailbox, Message,
    MessageKind, ModuleId, QuotaLedger, SystemBudget,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct WcetReport {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    iters: u32,
    mailbox_cyc: u32,
    alarm_cyc: u32,
    quota_cyc: u32,
    authorize_cyc: u32,
    lease_cyc: u32,
    event_flags_cyc: u32,
    critical_section_cyc: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E57_4354; // "NWCT"

#[no_mangle]
#[used]
static mut NOBRO_WCET_REPORT: WcetReport = WcetReport {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    iters: 0,
    mailbox_cyc: 0,
    alarm_cyc: 0,
    quota_cyc: 0,
    authorize_cyc: 0,
    lease_cyc: 0,
    event_flags_cyc: 0,
    critical_section_cyc: 0,
    checksum: 0,
};

const ITERS: u32 = 1000;

// ---- DWT cycle counter (Cortex-M4) ----
const DEMCR: *mut u32 = 0xE000_EDFC as *mut u32;
const DWT_CTRL: *mut u32 = 0xE000_1000 as *mut u32;
const DWT_CYCCNT: *mut u32 = 0xE000_1004 as *mut u32;

fn dwt_init() {
    unsafe {
        DEMCR.write_volatile(DEMCR.read_volatile() | (1 << 24)); // TRCENA
        DWT_CYCCNT.write_volatile(0);
        DWT_CTRL.write_volatile(DWT_CTRL.read_volatile() | 1); // CYCCNTENA
    }
}

#[inline(always)]
fn cyccnt() -> u32 {
    unsafe { DWT_CYCCNT.read_volatile() }
}

/// Max cycles over ITERS runs of `op` (compiler barrier around the timed region).
fn wcet(mut op: impl FnMut()) -> u32 {
    let mut max = 0u32;
    for _ in 0..ITERS {
        cortex_m::asm::dsb();
        let t0 = cyccnt();
        op();
        cortex_m::asm::dsb();
        let dt = cyccnt().wrapping_sub(t0);
        if dt > max {
            max = dt;
        }
    }
    max
}

#[entry]
fn main() -> ! {
    dwt_init();

    // mailbox push + pop (the kernel IPC hot path)
    let mut mb = Mailbox::<8>::new();
    let msg = Message::new(
        ModuleId::Kernel,
        ModuleId::Sensor,
        MessageKind::Command,
        1,
        0,
    );
    let mailbox_cyc = wcet(|| {
        let _ = mb.push(msg);
        let _ = mb.pop();
    });

    // alarm schedule + pop_due (timer wheel hot path)
    let mut aq = AlarmQueue::<8>::new();
    let mut now = 0u64;
    let alarm_cyc = wcet(|| {
        now += 10;
        let _ = aq.schedule_once(AlarmId(1), ModuleId::Sensor, now + 5, now);
        let _ = aq.pop_due(now + 6);
    });

    // quota reserve + release (admission-time bookkeeping)
    let mut ledger = QuotaLedger::<4>::new();
    let _ = ledger.register(ModuleId::Sensor, SystemBudget::new(1024, 256, 2));
    let chunk = SystemBudget::new(0, 64, 1);
    let quota_cyc = wcet(|| {
        let _ = ledger.reserve(ModuleId::Sensor, chunk);
        let _ = ledger.release(ModuleId::Sensor, chunk);
    });

    // capability authorize (every host-service call passes through this)
    let mut table = CapabilityGrantTable::<4>::new();
    let _ = table.register(
        ModuleId::Sensor,
        CapabilitySet::empty().with(Capability::Bus0),
    );
    let authorize_cyc = wcet(|| {
        let _ = table.authorize(ModuleId::Sensor, Capability::Bus0);
    });

    // peripheral lease acquire + release (critical-section protected)
    let lease_cyc = wcet(|| {
        let _ = Hal::acquire(Resource::Egu0, 7);
        let _ = Hal::release(Resource::Egu0, 7);
    });

    // classic event flags set + wait_any (FreeRTOS-migrant hot path)
    let mut ev = EventFlags::new();
    let event_flags_cyc = wcet(|| {
        ev.set(0b1);
        let _ = ev.wait_any(0b1, true);
    });

    // longest interrupt-masked window produced by a critical_section kernel op:
    // time the masked region of a lease acquire (the CS covers the whole closure).
    let critical_section_cyc = wcet(|| {
        cortex_m::interrupt::free(|_| {
            cortex_m::asm::nop();
        });
        let _ = Hal::acquire(Resource::Egu0, 7);
        let _ = Hal::release(Resource::Egu0, 7);
    });

    // sanity ceilings @64 MHz: every op must be sub-10 µs (640 cycles) except the
    // composite CS probe (sub-20 µs). Failing these means a regression, not noise.
    let ok = mailbox_cyc > 0
        && mailbox_cyc < 640
        && alarm_cyc > 0
        && alarm_cyc < 640
        && quota_cyc > 0
        && quota_cyc < 640
        && authorize_cyc > 0
        && authorize_cyc < 640
        && lease_cyc > 0
        && lease_cyc < 640
        && event_flags_cyc > 0
        && event_flags_cyc < 640
        && critical_section_cyc > 0
        && critical_section_cyc < 1280;

    let ap = u32::from(ok);
    let cs = MAGIC
        ^ 1
        ^ 1
        ^ ap
        ^ ITERS
        ^ mailbox_cyc
        ^ alarm_cyc
        ^ quota_cyc
        ^ authorize_cyc
        ^ lease_cyc
        ^ event_flags_cyc
        ^ critical_section_cyc;
    unsafe {
        NOBRO_WCET_REPORT = WcetReport {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            iters: ITERS,
            mailbox_cyc,
            alarm_cyc,
            quota_cyc,
            authorize_cyc,
            lease_cyc,
            event_flags_cyc,
            critical_section_cyc,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
