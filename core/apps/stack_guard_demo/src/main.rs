//! Stack-overflow detection (M168): a StackGuard canary region planted below the live
//! stack pointer. Shallow recursion must leave it intact (no false positive); deep
//! recursion that reaches the guard must trip it (true positive). Runs on real RAM on a
//! development board; nRF52840 app RAM is 256 KB with the stack at the top, so probing a
//! few KB below MSP is safe.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    intact_after_shallow: u32,
    tripped_after_deep: u32,
    guard_addr: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E53_474B; // "NSGK"

#[no_mangle]
#[used]
static mut NOBRO_STACK_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    intact_after_shallow: 0,
    tripped_after_deep: 0,
    guard_addr: 0,
    checksum: 0,
};

const CANARY: u32 = 0x5743_414E; // "NCAW"
const GUARD_WORDS: usize = 16;

struct StackGuard {
    base: *mut u32,
}

impl StackGuard {
    /// Plant a canary block `offset_bytes` below the current stack pointer.
    unsafe fn plant(offset_bytes: u32) -> StackGuard {
        let sp = cortex_m::register::msp::read();
        let base = ((sp - offset_bytes) & !3) as *mut u32;
        for i in 0..GUARD_WORDS {
            core::ptr::write_volatile(base.add(i), CANARY);
        }
        StackGuard { base }
    }

    unsafe fn intact(&self) -> bool {
        (0..GUARD_WORDS).all(|i| core::ptr::read_volatile(self.base.add(i)) == CANARY)
    }
}

/// Recurse consuming ~64 bytes of stack per level; the volatile writes stop the
/// compiler from collapsing the frames.
fn burn(depth: u32) -> u32 {
    let mut pad = [0u32; 12];
    for (i, p) in pad.iter_mut().enumerate() {
        unsafe { core::ptr::write_volatile(p, depth.wrapping_add(i as u32)) };
    }
    if depth == 0 {
        unsafe { core::ptr::read_volatile(&pad[11]) }
    } else {
        burn(depth - 1).wrapping_add(unsafe { core::ptr::read_volatile(&pad[0]) })
    }
}

#[entry]
fn main() -> ! {
    // Guard 3 KB below MSP. Shallow recursion (~12 levels * ~64 B < 1 KB) must not
    // reach it; deep recursion (~96 levels * ~64 B > 5 KB) must cross and trip it.
    let guard = unsafe { StackGuard::plant(3 * 1024) };
    let guard_addr = guard.base as u32;

    let _ = burn(12);
    let intact_after_shallow = unsafe { u32::from(guard.intact()) };

    let _ = burn(96);
    let tripped_after_deep = unsafe { u32::from(!guard.intact()) };

    let pass = intact_after_shallow == 1 && tripped_after_deep == 1;
    let ap = u32::from(pass);
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ intact_after_shallow ^ tripped_after_deep ^ guard_addr;
    unsafe {
        NOBRO_STACK_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            intact_after_shallow,
            tripped_after_deep,
            guard_addr,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
