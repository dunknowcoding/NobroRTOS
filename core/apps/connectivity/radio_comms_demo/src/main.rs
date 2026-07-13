//! Radio as a managed hardware resource: verify the Resource::Radio exclusive
//! lease (acquire, conflict rejected, wrong-owner release), Capability::Radio
//! authorization, and deadline/budget-accounted frame TX via the wireless domain. Publishes
//! NOBRO_RADIO_COMMS_REPORT, showing the runtime manages the
//! radio peripheral, closing the radio's integration into the kernel.
#![no_std]
#![no_main]

use cortex_m_rt::entry;
use defmt_rtt as _;
use panic_halt as _;

use nobro_adapter_radio_comms::RadioComms;
use nobro_hal::{
    lease::{LeaseError, Resource},
    traits::HalLease,
    ActivePlatform as Hal,
};
use nobro_kernel::{Capability, CapabilityGrantTable, CapabilitySet, ModuleId};
use nobro_wireless::{LinkBudget, ManagedLink, TxContract};

#[repr(C)]
#[derive(Clone, Copy)]
struct Report {
    magic: u32,
    version: u32,
    completed: u32,
    all_pass: u32,
    lease_ok: u32,
    capability_ok: u32,
    frames_sent: u32,
    checksum: u32,
}
const MAGIC: u32 = 0x4E52_4332; // "NRC2"

#[no_mangle]
#[used]
static mut NOBRO_RADIO_COMMS_REPORT: Report = Report {
    magic: 0,
    version: 0,
    completed: 0,
    all_pass: 0,
    lease_ok: 0,
    capability_ok: 0,
    frames_sent: 0,
    checksum: 0,
};

fn start_hfxo() {
    unsafe {
        core::ptr::write_volatile(0x4000_0000 as *mut u32, 1);
        while core::ptr::read_volatile(0x4000_0100 as *const u32) == 0 {}
    }
}

/// The radio is exclusive: a second owner is rejected, and only the holder may release.
fn test_lease() -> bool {
    const A: u8 = 7;
    const B: u8 = 8;
    let _ = Hal::release(Resource::Radio, A);
    let _ = Hal::release(Resource::Radio, B);
    let acquired = Hal::acquire(Resource::Radio, A).is_ok();
    let conflict_rejected = matches!(
        Hal::acquire(Resource::Radio, B),
        Err(LeaseError::AlreadyHeld)
    );
    let wrong_owner = matches!(
        Hal::release(Resource::Radio, B),
        Err(LeaseError::WrongOwner)
    );
    let released = Hal::release(Resource::Radio, A).is_ok();
    let reacquired = Hal::acquire(Resource::Radio, B).is_ok();
    let freed = Hal::release(Resource::Radio, B).is_ok();
    acquired && conflict_rejected && wrong_owner && released && reacquired && freed
}

/// Capability::Radio gates radio use; an unrelated capability is denied.
fn test_capability() -> bool {
    let mut table = CapabilityGrantTable::<2>::new();
    let granted = CapabilitySet::empty().with(Capability::Radio);
    if table.register(ModuleId::Radio, granted).is_err() {
        return false;
    }
    table.authorize(ModuleId::Radio, Capability::Radio).is_ok()
        && table.authorize(ModuleId::Radio, Capability::Bus0).is_err()
}

#[entry]
fn main() -> ! {
    start_hfxo();

    let lease_ok = test_lease();
    let capability_ok = test_capability();

    // Domain-accounted frame TX through the managed radio (takes + releases the lease).
    let mut frames_sent: u32 = 0;
    let mut release_ok = false;
    if let Ok(comms) = RadioComms::acquire(7) {
        let mut link = ManagedLink::new(comms, LinkBudget::new(32, 20, 60));
        for i in 0..20u32 {
            let pkt = [0xC2u8, (i & 0xFF) as u8, ((i >> 8) & 0xFF) as u8];
            if link.send_at(i as u64, TxContract::by(100), &pkt).is_ok() {
                frames_sent = frames_sent.wrapping_add(1);
            }
            for _ in 0..100_000u32 {
                cortex_m::asm::nop();
            }
        }
        release_ok = link.into_backend().release().is_ok();
    }

    let pass = lease_ok && capability_ok && frames_sent >= 10 && release_ok;
    let lo = u32::from(lease_ok);
    let co = u32::from(capability_ok);
    let ap = u32::from(pass);
    let cs = MAGIC ^ 1 ^ 1 ^ ap ^ lo ^ co ^ frames_sent;
    unsafe {
        NOBRO_RADIO_COMMS_REPORT = Report {
            magic: MAGIC,
            version: 1,
            completed: 1,
            all_pass: ap,
            lease_ok: lo,
            capability_ok: co,
            frames_sent,
            checksum: cs,
        };
    }

    loop {
        cortex_m::asm::delay(16_000_000);
    }
}
