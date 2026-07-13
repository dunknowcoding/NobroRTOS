//! Bounded async executor hardware example: runs the same checks as the host unit tests
//! on the MCU and records the outcome in NOBRO_ASYNC_EXEC_REPORT. No HAL - pure kernel.
#![no_std]
#![no_main]

use core::future::Future;
use core::pin::{pin, Pin};
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::{Context, Poll};

use cortex_m::asm;
use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_kernel::async_exec::{AsyncError, BoundedExecutor};

const AE_MAGIC: u32 = 0x4E42_4153; // "NBAS"

#[repr(C)]
#[derive(Clone, Copy)]
struct AsyncExecReport {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    spawn_pass: u32,
    capacity_pass: u32,
    stall_pass: u32,
    rounds_used: u32,
    tasks_completed: u32,
    checksum: u32,
}

#[no_mangle]
#[used]
static mut NOBRO_ASYNC_EXEC_REPORT: AsyncExecReport = AsyncExecReport {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    spawn_pass: 0,
    capacity_pass: 0,
    stall_pass: 0,
    rounds_used: 0,
    tasks_completed: 0,
    checksum: 0,
};

struct Countdown<'c> {
    left: u32,
    done: &'c AtomicU32,
}

impl<'c> Future for Countdown<'c> {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        if self.left == 0 {
            self.done.fetch_add(1, Ordering::Relaxed);
            Poll::Ready(())
        } else {
            self.left -= 1;
            Poll::Pending
        }
    }
}

struct Never;
impl Future for Never {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        Poll::Pending
    }
}

fn test_spawn_complete() -> (bool, u32, u32) {
    let done = AtomicU32::new(0);
    let f1 = pin!(Countdown {
        left: 3,
        done: &done
    });
    let f2 = pin!(Countdown {
        left: 1,
        done: &done
    });
    let mut exec = BoundedExecutor::<4>::new();
    if exec.spawn(f1).is_err() || exec.spawn(f2).is_err() {
        return (false, 0, 0);
    }
    match exec.run_to_idle(16) {
        Ok(rounds) => (done.load(Ordering::Relaxed) == 2, rounds, 2),
        Err(_) => (false, 0, 0),
    }
}

fn test_capacity() -> bool {
    let done = AtomicU32::new(0);
    let f1 = pin!(Countdown {
        left: 0,
        done: &done
    });
    let f2 = pin!(Countdown {
        left: 0,
        done: &done
    });
    let mut exec = BoundedExecutor::<1>::new();
    exec.spawn(f1).is_ok() && exec.spawn(f2) == Err(AsyncError::Full)
}

fn test_stall() -> bool {
    let n = pin!(Never);
    let mut exec = BoundedExecutor::<1>::new();
    exec.spawn(n).is_ok() && exec.run_to_idle(8) == Err(AsyncError::Stalled)
}

#[entry]
fn main() -> ! {
    let (spawn_ok, rounds, completed) = test_spawn_complete();
    let cap_ok = test_capacity();
    let stall_ok = test_stall();
    let all = spawn_ok && cap_ok && stall_ok;

    let sp = u32::from(spawn_ok);
    let cp = u32::from(cap_ok);
    let st = u32::from(stall_ok);
    let ap = u32::from(all);
    let cs = AE_MAGIC ^ 1 ^ ap ^ sp ^ cp ^ st ^ rounds ^ completed;

    unsafe {
        NOBRO_ASYNC_EXEC_REPORT = AsyncExecReport {
            magic: AE_MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            spawn_pass: sp,
            capacity_pass: cp,
            stall_pass: st,
            rounds_used: rounds,
            tasks_completed: completed,
            checksum: cs,
        };
    }

    loop {
        asm::delay(16_000_000);
    }
}
