//! Peripheral exclusive lease (ArduinoNRF PeripheralLease equivalent).

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use crate::traits::LeaseId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Timer0,
    Twim0,
    Twim1,
    Spim0,
    Radio,
    Rtc2,
    Timer1,
    Pwm0,
    Egu0,
    Ppi,
}

impl Resource {
    pub const ALL: [Self; 10] = [
        Self::Timer0,
        Self::Twim0,
        Self::Twim1,
        Self::Spim0,
        Self::Radio,
        Self::Rtc2,
        Self::Timer1,
        Self::Pwm0,
        Self::Egu0,
        Self::Ppi,
    ];

    pub const COUNT: usize = Self::ALL.len();

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timer0 => "TIMER0",
            Self::Twim0 => "TWIM0",
            Self::Twim1 => "TWIM1",
            Self::Spim0 => "SPIM0",
            Self::Radio => "RADIO",
            Self::Rtc2 => "RTC2",
            Self::Timer1 => "TIMER1",
            Self::Pwm0 => "PWM0",
            Self::Egu0 => "EGU0",
            Self::Ppi => "PPI",
        }
    }
}

impl From<Resource> for LeaseId {
    fn from(resource: Resource) -> Self {
        match resource {
            Resource::Timer0 => Self::SYSTEM_TIMER,
            Resource::Twim0 => Self::PRIMARY_I2C,
            Resource::Twim1 => Self::SECONDARY_I2C,
            Resource::Spim0 => Self::PRIMARY_SPI,
            Resource::Radio => Self::PRIMARY_RADIO,
            Resource::Rtc2 => Self::LOW_POWER_TIMER,
            Resource::Timer1 => Self::DEADLINE_TIMER,
            Resource::Pwm0 => Self::PRIMARY_PWM,
            Resource::Egu0 => Self::SOFTWARE_EVENT,
            Resource::Ppi => Self::EVENT_ROUTER,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseError {
    AlreadyHeld,
    NotHeld,
    WrongOwner,
    Unsupported,
}

struct LeaseSlot {
    taken: AtomicBool,
    owner: AtomicU8,
    generation: AtomicU32,
}

impl LeaseSlot {
    const fn new() -> Self {
        Self {
            taken: AtomicBool::new(false),
            owner: AtomicU8::new(0),
            generation: AtomicU32::new(1),
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
        Resource::Timer1 => 6,
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
            crate::quiesce::resource(resource);
            slot.taken.store(false, Ordering::Release);
            slot.owner.store(0, Ordering::Release);
            slot.generation.store(
                slot.generation.load(Ordering::Relaxed).wrapping_add(1),
                Ordering::Release,
            );
            Ok(())
        })
    }

    pub fn is_held(resource: Resource) -> bool {
        SLOTS[idx(resource)].taken.load(Ordering::Acquire)
    }

    pub fn owner(resource: Resource) -> Option<u8> {
        critical_section::with(|_| {
            let slot = &SLOTS[idx(resource)];
            if slot.taken.load(Ordering::Acquire) {
                Some(slot.owner.load(Ordering::Acquire))
            } else {
                None
            }
        })
    }

    /// Recovery hook: release every resource owned by a faulted module.
    ///
    /// This is intentionally owner-scoped, not a global reset. A supervisor can quiesce
    /// one module and clean up its leaked leases without disturbing healthy modules.
    pub fn release_all_for_owner(owner: u8) -> usize {
        critical_section::with(|_| {
            let mut released = 0;
            for (index, slot) in SLOTS.iter().enumerate() {
                if slot.taken.load(Ordering::Acquire) && slot.owner.load(Ordering::Acquire) == owner
                {
                    crate::quiesce::resource(Resource::ALL[index]);
                    slot.taken.store(false, Ordering::Release);
                    slot.owner.store(0, Ordering::Release);
                    slot.generation.store(
                        slot.generation.load(Ordering::Relaxed).wrapping_add(1),
                        Ordering::Release,
                    );
                    released += 1;
                }
            }
            released
        })
    }

    pub fn acquire_guard(resource: Resource, owner: u8) -> Result<LeaseGuard, LeaseError> {
        critical_section::with(|_| {
            let slot = &SLOTS[idx(resource)];
            if slot.taken.load(Ordering::Acquire) {
                return Err(LeaseError::AlreadyHeld);
            }
            let generation = slot.generation.load(Ordering::Acquire);
            slot.owner.store(owner, Ordering::Release);
            slot.taken.store(true, Ordering::Release);
            Ok(LeaseGuard {
                resource,
                owner,
                generation,
                active: true,
            })
        })
    }

    fn token_is_live(resource: Resource, owner: u8, generation: u32) -> bool {
        critical_section::with(|_| {
            let slot = &SLOTS[idx(resource)];
            slot.taken.load(Ordering::Acquire)
                && slot.owner.load(Ordering::Acquire) == owner
                && slot.generation.load(Ordering::Acquire) == generation
        })
    }

    fn release_token(resource: Resource, owner: u8, generation: u32) -> Result<(), LeaseError> {
        critical_section::with(|_| {
            let slot = &SLOTS[idx(resource)];
            if !slot.taken.load(Ordering::Acquire)
                || slot.generation.load(Ordering::Acquire) != generation
            {
                return Err(LeaseError::NotHeld);
            }
            if slot.owner.load(Ordering::Acquire) != owner {
                return Err(LeaseError::WrongOwner);
            }
            crate::quiesce::resource(resource);
            slot.taken.store(false, Ordering::Release);
            slot.owner.store(0, Ordering::Release);
            slot.generation.store(
                slot.generation.load(Ordering::Relaxed).wrapping_add(1),
                Ordering::Release,
            );
            Ok(())
        })
    }
}

/// An acquisition-generation proof; fields are private and the token is not clonable.
///
/// ```compile_fail
/// use nobro_hal::{LeaseGuard, Resource};
/// let forged = LeaseGuard { resource: Resource::Twim0, owner: 1, generation: 1, active: true };
/// ```
///
/// ```compile_fail
/// # use nobro_hal::{Resource, ResourceLease};
/// let guard = ResourceLease::acquire_guard(Resource::Twim0, 1).unwrap();
/// let duplicate = guard.clone();
/// ```
pub struct LeaseGuard {
    resource: Resource,
    owner: u8,
    generation: u32,
    active: bool,
}

impl LeaseGuard {
    pub const fn resource(&self) -> Resource {
        self.resource
    }

    pub const fn owner(&self) -> u8 {
        self.owner
    }

    /// Prove this exact acquisition is still live. Recovery invalidates all extant
    /// guards by advancing the slot generation, even if the same owner reacquires it.
    pub fn ensure_live(&self) -> Result<(), LeaseError> {
        ResourceLease::token_is_live(self.resource, self.owner, self.generation)
            .then_some(())
            .ok_or(LeaseError::NotHeld)
    }

    pub fn release(mut self) -> Result<(), LeaseError> {
        ResourceLease::release_token(self.resource, self.owner, self.generation)?;
        self.active = false;
        Ok(())
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = ResourceLease::release_token(self.resource, self.owner, self.generation);
            self.active = false;
        }
    }
}

#[cfg(test)]
mod invariant_tests {
    //! Property-based verification of the lease invariants: thousands of random
    //! acquire/release operations are checked against a reference model, proving the
    //! lease state machine never violates mutual exclusion, ownership, or the acquire/
    //! release rules. A model checker (kani/loom) would be stronger for concurrency, but
    //! critical_section (interrupt masking, not threads) is a poor fit for loom; this
    //! exhaustive-ish randomized check is the practical formal-invariant coverage.
    use super::*;
    extern crate std;
    use core::hint::spin_loop;
    use core::sync::atomic::AtomicBool;

    static TEST_LOCK: AtomicBool = AtomicBool::new(false);

    struct TestLock;

    impl Drop for TestLock {
        fn drop(&mut self) {
            TEST_LOCK.store(false, Ordering::Release);
        }
    }

    fn test_lock() -> TestLock {
        while TEST_LOCK
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            spin_loop();
        }
        TestLock
    }

    fn reset_all() {
        for s in &SLOTS {
            s.taken.store(false, Ordering::Release);
            s.owner.store(0, Ordering::Release);
            s.generation.store(1, Ordering::Release);
        }
    }

    #[test]
    fn lease_invariants_hold_over_random_op_sequences() {
        let _lock = test_lock();
        reset_all();
        // reference model: which owner (if any) holds each resource
        let mut model: [Option<u8>; Resource::COUNT] = [None; Resource::COUNT];
        let mut rng: u32 = 0x1357_9BDF;
        let next = |r: &mut u32| {
            *r = r.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *r
        };

        // Miri exhaustively interprets each atomic/critical-section operation; keep a
        // substantial deterministic sequence there while retaining the 30k native gate.
        let operations = if cfg!(miri) { 2_000 } else { 30_000 };
        for _ in 0..operations {
            let ri = (next(&mut rng) % Resource::COUNT as u32) as usize;
            let res = Resource::ALL[ri];
            let owner = 1 + (next(&mut rng) % 4) as u8; // owners 1..=4 exercise WrongOwner
            let op = next(&mut rng) % 6;

            if op <= 2 {
                let got = ResourceLease::acquire(res, owner);
                match model[ri] {
                    None => {
                        assert!(got.is_ok(), "acquire of a free resource must succeed");
                        model[ri] = Some(owner);
                    }
                    Some(_) => assert_eq!(
                        got,
                        Err(LeaseError::AlreadyHeld),
                        "acquire of a held resource must be rejected (mutual exclusion)"
                    ),
                }
            } else if op <= 4 {
                let got = ResourceLease::release(res, owner);
                match model[ri] {
                    None => assert_eq!(got, Err(LeaseError::NotHeld)),
                    Some(o) if o == owner => {
                        assert!(got.is_ok());
                        model[ri] = None;
                    }
                    Some(_) => assert_eq!(
                        got,
                        Err(LeaseError::WrongOwner),
                        "only the current owner may release"
                    ),
                }
            } else {
                let expected = model.iter().filter(|&&o| o == Some(owner)).count();
                let released = ResourceLease::release_all_for_owner(owner);
                assert_eq!(released, expected);
                for slot in &mut model {
                    if *slot == Some(owner) {
                        *slot = None;
                    }
                }
            }
            // invariant: the peripheral's held-state always matches the model
            assert_eq!(ResourceLease::is_held(res), model[ri].is_some());
            assert_eq!(ResourceLease::owner(res), model[ri]);
        }

        // full sweep: every slot agrees with the model after the whole sequence
        for (j, res) in Resource::ALL.iter().enumerate() {
            assert_eq!(ResourceLease::is_held(*res), model[j].is_some());
            assert_eq!(ResourceLease::owner(*res), model[j]);
        }
        reset_all();
    }

    #[test]
    fn recovery_can_release_all_leases_for_one_owner() {
        let _lock = test_lock();
        reset_all();
        assert_eq!(ResourceLease::acquire(Resource::Twim0, 7), Ok(()));
        assert_eq!(ResourceLease::acquire(Resource::Spim0, 7), Ok(()));
        assert_eq!(ResourceLease::acquire(Resource::Radio, 8), Ok(()));
        assert_eq!(ResourceLease::owner(Resource::Twim0), Some(7));

        let twim_before = crate::quiesce::count(Resource::Twim0);
        let spim_before = crate::quiesce::count(Resource::Spim0);
        assert_eq!(ResourceLease::release_all_for_owner(7), 2);
        assert_eq!(crate::quiesce::count(Resource::Twim0), twim_before + 1);
        assert_eq!(crate::quiesce::count(Resource::Spim0), spim_before + 1);
        assert!(!ResourceLease::is_held(Resource::Twim0));
        assert!(!ResourceLease::is_held(Resource::Spim0));
        assert_eq!(ResourceLease::owner(Resource::Radio), Some(8));
        assert_eq!(ResourceLease::release(Resource::Radio, 8), Ok(()));
        reset_all();
    }

    #[test]
    fn guard_drop_auto_releases_the_resource() {
        let _lock = test_lock();
        reset_all();
        {
            let guard = ResourceLease::acquire_guard(Resource::Pwm0, 3).unwrap();
            assert_eq!(guard.resource(), Resource::Pwm0);
            assert_eq!(ResourceLease::owner(Resource::Pwm0), Some(3));
        }
        assert_eq!(ResourceLease::owner(Resource::Pwm0), None);
        reset_all();
    }

    #[test]
    fn recovery_invalidates_stale_guard_even_after_same_owner_reacquires() {
        let _lock = test_lock();
        reset_all();
        let stale = ResourceLease::acquire_guard(Resource::Twim0, 7).unwrap();
        assert_eq!(stale.ensure_live(), Ok(()));
        assert_eq!(ResourceLease::release_all_for_owner(7), 1);
        assert_eq!(stale.ensure_live(), Err(LeaseError::NotHeld));
        let current = ResourceLease::acquire_guard(Resource::Twim0, 7).unwrap();
        assert_eq!(current.ensure_live(), Ok(()));
        assert_eq!(stale.ensure_live(), Err(LeaseError::NotHeld));
        drop(current);
        drop(stale);
        reset_all();
    }

    #[test]
    fn recovery_denies_safe_bus_use_before_touching_hardware() {
        let _lock = test_lock();
        reset_all();
        let bus = crate::bus::TwimBus::new_twim0(41).unwrap();
        let mut bytes = [0u8; 2];
        assert_eq!(bus.read_stub(0x52, &mut bytes), Ok(()));
        assert_eq!(ResourceLease::release_all_for_owner(41), 1);
        assert_eq!(
            bus.read_stub(0x52, &mut bytes),
            Err(crate::bus::BusError::LeaseDenied)
        );
        assert_eq!(bytes, [0x52, 0x53]);
        drop(bus);
        reset_all();
    }

    #[test]
    fn concurrent_recovery_and_reacquire_cannot_revive_old_authority() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let _lock = test_lock();
        reset_all();
        let stale = ResourceLease::acquire_guard(Resource::Spim0, 12).unwrap();
        let ready = Arc::new(Barrier::new(2));
        let released = Arc::new(Barrier::new(2));
        let worker_ready = Arc::clone(&ready);
        let worker_released = Arc::clone(&released);
        let worker = thread::spawn(move || {
            worker_ready.wait();
            worker_released.wait();
            stale.ensure_live()
        });
        ready.wait();
        assert_eq!(ResourceLease::release_all_for_owner(12), 1);
        let current = ResourceLease::acquire_guard(Resource::Spim0, 12).unwrap();
        released.wait();
        assert_eq!(worker.join().unwrap(), Err(LeaseError::NotHeld));
        assert_eq!(current.ensure_live(), Ok(()));
        drop(current);
        reset_all();
    }
}
