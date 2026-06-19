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

impl Resource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timer0 => "TIMER0",
            Self::Twim0 => "TWIM0",
            Self::Twim1 => "TWIM1",
            Self::Spim0 => "SPIM0",
            Self::Radio => "RADIO",
            Self::Rtc2 => "RTC2",
            Self::Timer3 => "TIMER3",
        }
    }
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

    pub fn acquire_guard(resource: Resource, owner: u8) -> Result<LeaseGuard, LeaseError> {
        Self::acquire(resource, owner)?;
        Ok(LeaseGuard {
            resource,
            owner,
            active: true,
        })
    }
}

pub struct LeaseGuard {
    resource: Resource,
    owner: u8,
    active: bool,
}

impl LeaseGuard {
    pub const fn resource(&self) -> Resource {
        self.resource
    }

    pub const fn owner(&self) -> u8 {
        self.owner
    }

    pub fn release(mut self) -> Result<(), LeaseError> {
        ResourceLease::release(self.resource, self.owner)?;
        self.active = false;
        Ok(())
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = ResourceLease::release(self.resource, self.owner);
            self.active = false;
        }
    }
}
