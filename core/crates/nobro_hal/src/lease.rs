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
    Pwm0,
    Egu0,
    Ppi,
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
            Self::Pwm0 => "PWM0",
            Self::Egu0 => "EGU0",
            Self::Ppi => "PPI",
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

static SLOTS: [LeaseSlot; 10] = [
    LeaseSlot::new(),
    LeaseSlot::new(),
    LeaseSlot::new(),
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
        Resource::Pwm0 => 7,
        Resource::Egu0 => 8,
        Resource::Ppi => 9,
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

#[cfg(test)]
mod invariant_tests {
    //! Property-based verification of the lease invariants (M172): thousands of random
    //! acquire/release operations are checked against a reference model, proving the
    //! lease state machine never violates mutual exclusion, ownership, or the acquire/
    //! release rules. A model checker (kani/loom) would be stronger for concurrency, but
    //! critical_section (interrupt masking, not threads) is a poor fit for loom; this
    //! exhaustive-ish randomized check is the practical formal-invariant coverage.
    use super::*;

    const RESOURCES: [Resource; 10] = [
        Resource::Timer0, Resource::Twim0, Resource::Twim1, Resource::Spim0,
        Resource::Radio, Resource::Rtc2, Resource::Timer3, Resource::Pwm0,
        Resource::Egu0, Resource::Ppi,
    ];

    fn reset_all() {
        for s in &SLOTS {
            s.taken.store(false, Ordering::Release);
            s.owner.store(0, Ordering::Release);
        }
    }

    #[test]
    fn lease_invariants_hold_over_random_op_sequences() {
        reset_all();
        // reference model: which owner (if any) holds each resource
        let mut model: [Option<u8>; 10] = [None; 10];
        let mut rng: u32 = 0x1357_9BDF;
        let mut next = |r: &mut u32| {
            *r = r.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *r
        };

        for _ in 0..30_000 {
            let ri = (next(&mut rng) % 10) as usize;
            let res = RESOURCES[ri];
            let owner = 1 + (next(&mut rng) % 4) as u8; // owners 1..=4 exercise WrongOwner
            let acquire = next(&mut rng) & 1 == 0;

            if acquire {
                let got = ResourceLease::acquire(res, owner);
                match model[ri] {
                    None => {
                        assert!(got.is_ok(), "acquire of a free resource must succeed");
                        model[ri] = Some(owner);
                    }
                    Some(_) => assert_eq!(
                        got, Err(LeaseError::AlreadyHeld),
                        "acquire of a held resource must be rejected (mutual exclusion)"
                    ),
                }
            } else {
                let got = ResourceLease::release(res, owner);
                match model[ri] {
                    None => assert_eq!(got, Err(LeaseError::NotHeld)),
                    Some(o) if o == owner => {
                        assert!(got.is_ok());
                        model[ri] = None;
                    }
                    Some(_) => assert_eq!(
                        got, Err(LeaseError::WrongOwner),
                        "only the current owner may release"
                    ),
                }
            }
            // invariant: the peripheral's held-state always matches the model
            assert_eq!(ResourceLease::is_held(res), model[ri].is_some());
        }

        // full sweep: every slot agrees with the model after the whole sequence
        for (j, res) in RESOURCES.iter().enumerate() {
            assert_eq!(ResourceLease::is_held(*res), model[j].is_some());
        }
        reset_all();
    }
}
