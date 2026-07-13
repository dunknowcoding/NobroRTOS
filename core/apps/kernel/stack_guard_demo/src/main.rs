//! Stack-overflow detection as a kernel service:
//! the portable `nobro_kernel::StackGuardTable` guards a region planted below
//! the live stack pointer, attributed to a module identity. Shallow recursion
//! must leave the canary intact (no false positive); deep recursion that
//! reaches the guard must trip it AND the sweep must attribute the fault to
//! the registered module; repainting must re-arm the guard. Runs on real RAM
//! on a development board (nRF52840 app RAM is 256 KB with the stack at the
//! top, so probing a few KB below MSP is safe).
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_kernel::{module_code, ModuleId, StackGuardTable, StackRegion};

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    intact_after_shallow: u32,
    tripped_after_deep: u32,
    attributed_module: u32,
    rearmed_ok: u32,
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
    attributed_module: 0,
    rearmed_ok: 0,
    guard_addr: 0,
    checksum: 0,
};

const GUARD_BYTES: usize = 64;

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
    // Guard region 3 KB below MSP, attributed to the Sensor module identity.
    // Shallow recursion (~12 levels * ~64 B < 1 KB) must not reach it; deep
    // recursion (~96 levels * ~64 B > 5 KB) must cross and trip it.
    let sp = cortex_m::register::msp::read();
    let base = ((sp as usize - 3 * 1024) & !3) - GUARD_BYTES;
    let region = StackRegion {
        base,
        len: GUARD_BYTES,
        canary_bytes: GUARD_BYTES / 2,
    };
    let mut guards = StackGuardTable::<2>::new();
    // SAFETY: the region lies 3 KB below the live MSP in app RAM — valid for
    // volatile access for the program's lifetime and below every live frame.
    unsafe {
        guards.register(ModuleId::Sensor, region).unwrap();
    }
    let guard_addr = base as u32;

    let _ = burn(12);
    let intact_after_shallow = u32::from(guards.sweep().is_none());

    let _ = burn(96);
    let fault = guards.sweep();
    let tripped_after_deep = u32::from(fault.is_some());
    let attributed_module = fault.map(|f| module_code(f.module)).unwrap_or(0);

    // Recovery re-arms the guard: repaint, then the sweep is clean again.
    let rearmed_ok =
        u32::from(unsafe { guards.repaint(ModuleId::Sensor) } && guards.sweep().is_none());

    let pass = intact_after_shallow == 1
        && tripped_after_deep == 1
        && attributed_module == module_code(ModuleId::Sensor)
        && rearmed_ok == 1;
    let ap = u32::from(pass);
    let cs = MAGIC
        ^ 2
        ^ 1
        ^ ap
        ^ intact_after_shallow
        ^ tripped_after_deep
        ^ attributed_module
        ^ rearmed_ok
        ^ guard_addr;
    unsafe {
        NOBRO_STACK_REPORT = Report {
            magic: MAGIC,
            version: 2,
            completed: 1,
            all_pass: ap,
            intact_after_shallow,
            tripped_after_deep,
            attributed_module,
            rearmed_ok,
            guard_addr,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
