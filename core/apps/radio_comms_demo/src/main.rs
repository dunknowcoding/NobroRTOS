//! Radio as a MANAGED hardware resource on board1: verify the Resource::Radio exclusive
//! lease (acquire, conflict rejected, wrong-owner release), Capability::Radio
//! authorization, and StreamSal frame TX via the radio-comms adapter. Self-certifies via
//! NOBRO_RADIO_COMMS_REPORT (J-Link mem32) - proof NobroRTOS distributes/manages the
//! radio peripheral, closing the M26 radio's integration into the kernel.
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
use nobro_sal::StreamSal;

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

    // StreamSal frame TX through the managed radio (takes + releases the lease).
    let mut frames_sent: u32 = 0;
    if let Ok(mut comms) = RadioComms::acquire(7) {
        for i in 0..20u32 {
            let pkt = [0xC2u8, (i & 0xFF) as u8, ((i >> 8) & 0xFF) as u8];
            if comms.write_frame(&pkt).is_ok() {
                frames_sent = frames_sent.wrapping_add(1);
            }
            for _ in 0..100_000u32 {
                cortex_m::asm::nop();
            }
        }
        let _ = comms.release();
    }

    let pass = lease_ok && capability_ok && frames_sent >= 10;
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
