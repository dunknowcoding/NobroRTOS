//! More managed resources (M63): verify the kernel arbitrates the newly-leased PWM, EGU,
//! and PPI peripherals exactly like the bus/radio resources - exclusive acquire, conflict
//! rejection, wrong-owner protection, release, and re-acquire. Self-certifies via
//! NOBRO_LEASE_REPORT (J-Link mem32).
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_hal::{
    lease::{LeaseError, Resource},
    traits::HalLease,
    ActivePlatform as Hal,
};

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    tested: u32,
    passed: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E4C_4553; // "NLES"

#[no_mangle]
#[used]
static mut NOBRO_LEASE_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    tested: 0,
    passed: 0,
    checksum: 0,
};

/// Full arbitration cycle for one resource: exclusive acquire, conflict rejected,
/// wrong-owner release rejected, release, re-acquire by another owner, free.
fn test_resource(r: Resource) -> bool {
    const A: u8 = 11;
    const B: u8 = 12;
    let _ = Hal::release(r, A);
    let _ = Hal::release(r, B);
    let acquired = Hal::acquire(r, A).is_ok();
    let conflict = matches!(Hal::acquire(r, B), Err(LeaseError::AlreadyHeld));
    let wrong = matches!(Hal::release(r, B), Err(LeaseError::WrongOwner));
    let released = Hal::release(r, A).is_ok();
    let reacquired = Hal::acquire(r, B).is_ok();
    let freed = Hal::release(r, B).is_ok();
    acquired && conflict && wrong && released && reacquired && freed
}

#[entry]
fn main() -> ! {
    let resources = [Resource::Pwm0, Resource::Egu0, Resource::Ppi];
    let tested = resources.len() as u32;
    let mut passed = 0u32;
    for r in resources {
        if test_resource(r) {
            passed += 1;
        }
    }

    let pass = passed == tested;
    let ap = u32::from(pass);
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ tested ^ passed;
    unsafe {
        NOBRO_LEASE_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            tested,
            passed,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
