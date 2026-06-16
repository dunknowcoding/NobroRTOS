//! Peripheral exclusive lease (ArduinoNRF PeripheralLease equivalent).

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Timer0,
    Twim0,
    Twim1,
    Spim0,
    Radio,
    Rtc2,
    Timer3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseError {
    AlreadyHeld,
    NotHeld,
    WrongOwner,
}

struct LeaseSlot {
    taken: AtomicBool,
    owner: AtomicU8,
}

impl LeaseSlot {
    const fn new() -> Self {
        Self {
            taken: AtomicBool::new(false),
            owner: AtomicU8::new(0),
        }
    }
}

static SLOTS: [LeaseSlot; 7] = [
    LeaseSlot::new(),
    LeaseSlot::new(),
    LeaseSlot::new(),
    LeaseSlot::new(),
    LeaseSlot::new(),
    LeaseSlot::new(),
    LeaseSlot::new(),
];

fn idx(r: Resource) -> usize {
    match r {
        Resource::Timer0 => 0,
        Resource::Twim0 => 1,
        Resource::Twim1 => 2,
        Resource::Spim0 => 3,
        Resource::Radio => 4,
        Resource::Rtc2 => 5,
        Resource::Timer3 => 6,
    }
}

pub struct ResourceLease;

impl ResourceLease {
    pub fn acquire(resource: Resource, owner: u8) -> Result<(), LeaseError> {
        critical_section::with(|_| {
            let slot = &SLOTS[idx(resource)];
            if slot.taken.load(Ordering::Acquire) {
                return Err(LeaseError::AlreadyHeld);
            }
            slot.taken.store(true, Ordering::Release);
            slot.owner.store(owner, Ordering::Release);
            Ok(())
        })
    }

    pub fn release(resource: Resource, owner: u8) -> Result<(), LeaseError> {
        critical_section::with(|_| {
            let slot = &SLOTS[idx(resource)];
            if !slot.taken.load(Ordering::Acquire) {
                return Err(LeaseError::NotHeld);
            }
            if slot.owner.load(Ordering::Acquire) != owner {
                return Err(LeaseError::WrongOwner);
            }
            slot.taken.store(false, Ordering::Release);
            slot.owner.store(0, Ordering::Release);
            Ok(())
        })
    }

    pub fn is_held(resource: Resource) -> bool {
        SLOTS[idx(resource)].taken.load(Ordering::Acquire)
    }
}
