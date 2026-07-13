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
    lease::{LeaseError, Resource, ResourceLease},
    traits::HalLease,
    ActivePlatform as Hal, BusError, TwimBus,
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

fn test_generation_recovery() -> bool {
    const OWNER: u8 = 21;
    let stale = ResourceLease::acquire_guard(Resource::Twim0, OWNER).unwrap();
    let released = ResourceLease::release_all_for_owner(OWNER) == 1;
    let current = ResourceLease::acquire_guard(Resource::Twim0, OWNER).unwrap();
    let stale_denied = stale.ensure_live() == Err(LeaseError::NotHeld);
    drop(stale); // must not release `current`, despite the same numeric owner
    let current_alive = current.ensure_live().is_ok();
    drop(current);
    released && stale_denied && current_alive
}

fn test_safe_bus_denial() -> bool {
    const OWNER: u8 = 22;
    let bus = TwimBus::new_twim0(OWNER).unwrap();
    let released = ResourceLease::release_all_for_owner(OWNER) == 1;
    let mut bytes = [0u8; 2];
    released && bus.read_stub(0x52, &mut bytes) == Err(BusError::LeaseDenied)
}

#[entry]
fn main() -> ! {
    let resources = [Resource::Pwm0, Resource::Egu0, Resource::Ppi];
    let tested = resources.len() as u32 + 2;
    let mut passed = 0u32;
    for r in resources {
        if test_resource(r) {
            passed += 1;
        }
    }
    passed += u32::from(test_generation_recovery());
    passed += u32::from(test_safe_bus_denial());

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
