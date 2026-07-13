//! Bare-metal floor for the COMPLEX workload (Wave 59): the same five-stage
//! pipeline as `nobro-graph-complex`, hand-rolled. Deadline scheduling, the
//! fan-out to radio + storage, the bounded ring, and backpressure counting are
//! all written by hand — no framework. Report: [control_ticks, fusion, radio_tx, drops].
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

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

fn micros() -> u32 {
    unsafe {
        wr(TIMER0 + 0x040, 1);
        rd(TIMER0 + 0x540)
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

/// A hand-rolled one-slot channel with a full-flag, so backpressure is counted
/// exactly like the framework mailboxes.
struct Slot(Option<u32>);
impl Slot {
    fn push(&mut self, v: u32) -> bool {
        if self.0.is_some() {
            return false;
        }
        self.0 = Some(v);
        true
    }
    fn pop(&mut self) -> Option<u32> {
        self.0.take()
    }
}

#[entry]
fn main() -> ! {
    unsafe {
        wr(GPIO_P0 + 0x700 + 4 * PIN, 1);
    }
    timer_init();

    let mut report = [0u32; 4]; // control_ticks, fusion, radio_tx, drops
    let mut fused: u32 = 0;
    let mut ring = [0u32; 8];
    let mut ring_head = 0usize;

    // Hand-rolled deadline schedule for five stages.
    let (mut nf, mut nc, mut nr, mut ns, mut nd) = (0u32, 0u32, 0u32, 0u32, 0u32);
    let mut fusion_out = Slot(None);
    let mut radio_in = Slot(None);
    let mut storage_in = Slot(None);

    loop {
        let now = micros();
        // fusion @ 100 Hz
        if now.wrapping_sub(nf) < 0x8000_0000 {
            // BENCH_INSTRUMENTATION_BEGIN
            #[cfg(nobro_ram_run)]
            let runtime_start = runtime_cycles();
            // BENCH_INSTRUMENTATION_END
            nf = nf.wrapping_add(10_000);
            report[1] = report[1].wrapping_add(1);
            let a = report[1].wrapping_mul(3).wrapping_add(7);
            let b = report[1].wrapping_mul(5).wrapping_add(11);
            fused = fused - (fused >> 3) + ((a ^ b) >> 3);
            if !fusion_out.push(fused) {
                report[3] = report[3].wrapping_add(1);
            }
            // BENCH_INSTRUMENTATION_BEGIN
            #[cfg(nobro_ram_run)]
            account_runtime(runtime_start);
            // BENCH_INSTRUMENTATION_END
        }
        // control @ 50 Hz: consume fusion, toggle GPIO, fan out
        if now.wrapping_sub(nc) < 0x8000_0000 {
            // BENCH_INSTRUMENTATION_BEGIN
            #[cfg(nobro_ram_run)]
            let runtime_start = runtime_cycles();
            // BENCH_INSTRUMENTATION_END
            nc = nc.wrapping_add(20_000);
            report[0] = report[0].wrapping_add(1);
            unsafe {
                if report[0] & 1 == 0 {
                    wr(GPIO_P0 + 0x508, 1 << PIN);
                } else {
                    wr(GPIO_P0 + 0x50C, 1 << PIN);
                }
            }
            if let Some(cmd) = fusion_out.pop() {
                if !radio_in.push(cmd) {
                    report[3] = report[3].wrapping_add(1);
                }
                if !storage_in.push(cmd) {
                    report[3] = report[3].wrapping_add(1);
                }
            }
            // BENCH_INSTRUMENTATION_BEGIN
            #[cfg(nobro_ram_run)]
            account_runtime(runtime_start);
            // BENCH_INSTRUMENTATION_END
        }
        // radio @ 20 Hz
        if now.wrapping_sub(nr) < 0x8000_0000 {
            // BENCH_INSTRUMENTATION_BEGIN
            #[cfg(nobro_ram_run)]
            let runtime_start = runtime_cycles();
            // BENCH_INSTRUMENTATION_END
            nr = nr.wrapping_add(50_000);
            if radio_in.pop().is_some() {
                report[2] = report[2].wrapping_add(1);
            }
            // BENCH_INSTRUMENTATION_BEGIN
            #[cfg(nobro_ram_run)]
            account_runtime(runtime_start);
            // BENCH_INSTRUMENTATION_END
        }
        // storage @ 10 Hz
        if now.wrapping_sub(ns) < 0x8000_0000 {
            // BENCH_INSTRUMENTATION_BEGIN
            #[cfg(nobro_ram_run)]
            let runtime_start = runtime_cycles();
            // BENCH_INSTRUMENTATION_END
            ns = ns.wrapping_add(100_000);
            if let Some(v) = storage_in.pop() {
                ring[ring_head] = v;
                ring_head = (ring_head + 1) % ring.len();
            }
            // BENCH_INSTRUMENTATION_BEGIN
            #[cfg(nobro_ram_run)]
            account_runtime(runtime_start);
            // BENCH_INSTRUMENTATION_END
        }
        // diagnostics @ 5 Hz
        if now.wrapping_sub(nd) < 0x8000_0000 {
            // BENCH_INSTRUMENTATION_BEGIN
            #[cfg(nobro_ram_run)]
            let runtime_start = runtime_cycles();
            // BENCH_INSTRUMENTATION_END
            nd = nd.wrapping_add(200_000);
            // BENCH_INSTRUMENTATION_BEGIN
            #[cfg(nobro_ram_run)]
            account_runtime(runtime_start);
            // BENCH_INSTRUMENTATION_END
        }
        unsafe {
            core::ptr::write_volatile(core::ptr::addr_of_mut!(BASELINE_REPORT), report);
        }
    }
}
