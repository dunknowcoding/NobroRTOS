//! Peripheral exclusive lease (ArduinoNRF PeripheralLease equivalent).

use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU8, Ordering};

use crate::isolation::IsolationReceipt;
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
    Gpio,
    Gpiote,
    Uarte0,
    Saadc,
    Nvmc,
    Timer2,
}

impl Resource {
    pub const ALL: [Self; 16] = [
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
        Self::Gpio,
        Self::Gpiote,
        Self::Uarte0,
        Self::Saadc,
        Self::Nvmc,
        Self::Timer2,
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
            Self::Gpio => "GPIO",
            Self::Gpiote => "GPIOTE",
            Self::Uarte0 => "UARTE0",
            Self::Saadc => "SAADC",
            Self::Nvmc => "NVMC",
            Self::Timer2 => "TIMER2",
        }
    }

    /// Stable id used by the module-isolation peripheral allowlist.
    pub const fn isolation_id(self) -> u8 {
        idx(self) as u8 + 1
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
            Resource::Gpio => Self::PRIMARY_GPIO,
            Resource::Gpiote => Self::PRIMARY_IRQ,
            Resource::Uarte0 => Self::PRIMARY_UART,
            Resource::Saadc => Self::PRIMARY_ADC,
            Resource::Nvmc => Self::APPLICATION_FLASH,
            Resource::Timer2 => Self::PRIMARY_PULSE,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseError {
    AlreadyHeld,
    NotHeld,
    WrongOwner,
    GenerationExhausted,
    Unsupported,
    IsolationDenied,
    IsolationStale,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LeaseRecoveryReceipt {
    pub owner: u8,
    /// One bit per [`Resource::ALL`] entry. Every reported resource was
    /// quiesced before its generation advanced and ownership became free.
    pub released_mask: u16,
}

impl LeaseRecoveryReceipt {
    pub const fn released_count(self) -> usize {
        self.released_mask.count_ones() as usize
    }

    pub const fn released(self, resource: Resource) -> bool {
        self.released_mask & (1u16 << idx(resource)) != 0
    }
}

struct LeaseSlot {
    taken: AtomicBool,
    owner: AtomicU8,
    generation: AtomicU32,
    // Fits the slot's former padding on 32-bit targets.
    epoch: AtomicU16,
}

impl LeaseSlot {
    const fn new() -> Self {
        Self {
            taken: AtomicBool::new(false),
            owner: AtomicU8::new(0),
            generation: AtomicU32::new(1),
            epoch: AtomicU16::new(0),
        }
    }
}

#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<LeaseSlot>() == 8);

static SLOTS: [LeaseSlot; 16] = [const { LeaseSlot::new() }; 16];

const fn idx(r: Resource) -> usize {
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
        Resource::Gpio => 10,
        Resource::Gpiote => 11,
        Resource::Uarte0 => 12,
        Resource::Saadc => 13,
        Resource::Nvmc => 14,
        Resource::Timer2 => 15,
    }
}

/// Resources that are different programming modes of one physical nRF block.
///
/// Physical-block and pin-mux conflicts between otherwise distinct portable
/// identities. GPIO is currently a deliberately conservative whole-bank lease;
/// every provider that writes an exposed PIN_CNF/PSEL therefore excludes it.
const fn is_pin_owner(resource: Resource) -> bool {
    matches!(
        resource,
        Resource::Twim0
            | Resource::Twim1
            | Resource::Spim0
            | Resource::Pwm0
            | Resource::Gpiote
            | Resource::Uarte0
            | Resource::Saadc
    )
}

const fn resources_conflict(left: Resource, right: Resource) -> bool {
    if idx(left) == idx(right) {
        return true;
    }
    if matches!(
        (left, right),
        (Resource::Twim0, Resource::Spim0) | (Resource::Spim0, Resource::Twim0)
    ) {
        return true;
    }
    (idx(left) == idx(Resource::Gpio) && is_pin_owner(right))
        || (idx(right) == idx(Resource::Gpio) && is_pin_owner(left))
}

fn acquisition_conflicts(resource: Resource) -> bool {
    Resource::ALL.iter().any(|candidate| {
        resources_conflict(resource, *candidate)
            && SLOTS[idx(*candidate)].taken.load(Ordering::Acquire)
    })
}

#[inline(always)]
fn prepare_generation_epoch(slot: &LeaseSlot) -> Result<(), LeaseError> {
    if slot.generation.load(Ordering::Acquire) < u32::MAX {
        return Ok(());
    }
    let epoch = slot.epoch.load(Ordering::Acquire);
    if epoch == u16::MAX {
        return Err(LeaseError::GenerationExhausted);
    }
    slot.epoch.store(epoch + 1, Ordering::Release);
    slot.generation.store(1, Ordering::Release);
    Ok(())
}

fn advance_generation(slot: &LeaseSlot) {
    let generation = slot.generation.load(Ordering::Acquire);
    slot.generation
        .store(generation.saturating_add(1), Ordering::Release);
}

pub struct ResourceLease;

impl ResourceLease {
    pub fn acquire(resource: Resource, owner: u8) -> Result<(), LeaseError> {
        critical_section::with(|_| {
            let slot = &SLOTS[idx(resource)];
            if acquisition_conflicts(resource) {
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
            advance_generation(slot);
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
        Self::recover_owner(owner).released_count()
    }

    /// Atomically quiesce and revoke every peripheral lease held by `owner`.
    ///
    /// Interrupt masking spans hardware shutdown, owner clearing, and
    /// generation advancement, so a completion ISR or concurrent fault cannot
    /// publish through an old DMA pointer after the resource is reassigned.
    pub fn recover_owner(owner: u8) -> LeaseRecoveryReceipt {
        critical_section::with(|_| {
            let mut released_mask = 0u16;
            for (index, slot) in SLOTS.iter().enumerate() {
                if slot.taken.load(Ordering::Acquire) && slot.owner.load(Ordering::Acquire) == owner
                {
                    crate::quiesce::resource(Resource::ALL[index]);
                    slot.taken.store(false, Ordering::Release);
                    slot.owner.store(0, Ordering::Release);
                    advance_generation(slot);
                    released_mask |= 1u16 << index;
                }
            }
            LeaseRecoveryReceipt {
                owner,
                released_mask,
            }
        })
    }

    pub fn acquire_guard(resource: Resource, owner: u8) -> Result<LeaseGuard, LeaseError> {
        Self::acquire_guard_with_isolation(resource, owner, None)
    }

    pub fn acquire_guard_isolated(
        resource: Resource,
        isolation: IsolationReceipt,
    ) -> Result<LeaseGuard, LeaseError> {
        isolation
            .permits_peripheral(resource.isolation_id())
            .map_err(|error| match error {
                crate::IsolationError::PeripheralDenied(_) => LeaseError::IsolationDenied,
                _ => LeaseError::IsolationStale,
            })?;
        let owner =
            u8::try_from(isolation.lease_owner()).map_err(|_| LeaseError::IsolationDenied)?;
        Self::acquire_guard_with_isolation(resource, owner, Some(isolation))
    }

    fn acquire_guard_with_isolation(
        resource: Resource,
        owner: u8,
        isolation: Option<IsolationReceipt>,
    ) -> Result<LeaseGuard, LeaseError> {
        #[cfg(not(any(feature = "pmsa-v7", feature = "pmsa-v8", test)))]
        if isolation.is_some() {
            return Err(LeaseError::Unsupported);
        }
        critical_section::with(|_| {
            if let Some(receipt) = isolation {
                receipt
                    .permits_peripheral(resource.isolation_id())
                    .map_err(|error| match error {
                        crate::IsolationError::PeripheralDenied(_) => LeaseError::IsolationDenied,
                        _ => LeaseError::IsolationStale,
                    })?;
            }
            let slot = &SLOTS[idx(resource)];
            if acquisition_conflicts(resource) {
                return Err(LeaseError::AlreadyHeld);
            }
            prepare_generation_epoch(slot)?;
            let epoch = slot.epoch.load(Ordering::Acquire);
            let generation = slot.generation.load(Ordering::Acquire);
            slot.owner.store(owner, Ordering::Release);
            slot.taken.store(true, Ordering::Release);
            Ok(LeaseGuard {
                resource,
                owner,
                epoch,
                generation,
                active: true,
                #[cfg(any(feature = "pmsa-v7", feature = "pmsa-v8", test))]
                isolation,
            })
        })
    }

    fn token_is_live(
        resource: Resource,
        owner: u8,
        expected_epoch: u16,
        expected_generation: u32,
    ) -> bool {
        critical_section::with(|_| {
            let slot = &SLOTS[idx(resource)];
            slot.epoch.load(Ordering::Acquire) == expected_epoch
                && slot.taken.load(Ordering::Acquire)
                && slot.owner.load(Ordering::Acquire) == owner
                && slot.generation.load(Ordering::Acquire) == expected_generation
        })
    }

    fn release_token(
        resource: Resource,
        owner: u8,
        expected_epoch: u16,
        expected_generation: u32,
    ) -> Result<(), LeaseError> {
        critical_section::with(|_| {
            let slot = &SLOTS[idx(resource)];
            if slot.epoch.load(Ordering::Acquire) != expected_epoch
                || !slot.taken.load(Ordering::Acquire)
                || slot.generation.load(Ordering::Acquire) != expected_generation
            {
                return Err(LeaseError::NotHeld);
            }
            if slot.owner.load(Ordering::Acquire) != owner {
                return Err(LeaseError::WrongOwner);
            }
            crate::quiesce::resource(resource);
            slot.taken.store(false, Ordering::Release);
            slot.owner.store(0, Ordering::Release);
            advance_generation(slot);
            Ok(())
        })
    }
}

/// An acquisition-generation proof; fields are private and the token is not clonable.
///
/// ```compile_fail
/// use nobro_hal::{LeaseGuard, Resource};
/// let forged = LeaseGuard {
///     resource: Resource::Twim0,
///     owner: 1,
///     epoch: 0,
///     generation: 1,
///     active: true,
/// };
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
    epoch: u16,
    generation: u32,
    active: bool,
    #[cfg(any(feature = "pmsa-v7", feature = "pmsa-v8", test))]
    isolation: Option<IsolationReceipt>,
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
        if !ResourceLease::token_is_live(self.resource, self.owner, self.epoch, self.generation) {
            return Err(LeaseError::NotHeld);
        }
        #[cfg(any(feature = "pmsa-v7", feature = "pmsa-v8", test))]
        {
            if self
                .isolation
                .is_some_and(|receipt| receipt.ensure_usable().is_err())
            {
                return Err(LeaseError::IsolationStale);
            }
        }
        Ok(())
    }

    pub fn release(mut self) -> Result<(), LeaseError> {
        ResourceLease::release_token(self.resource, self.owner, self.epoch, self.generation)?;
        self.active = false;
        Ok(())
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = ResourceLease::release_token(
                self.resource,
                self.owner,
                self.epoch,
                self.generation,
            );
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
            s.epoch.store(0, Ordering::Release);
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
                let conflicting_peer_is_held =
                    Resource::ALL
                        .iter()
                        .enumerate()
                        .any(|(candidate_index, candidate)| {
                            candidate_index != ri
                                && resources_conflict(res, *candidate)
                                && model[candidate_index].is_some()
                        });
                match (model[ri], conflicting_peer_is_held) {
                    (None, false) => {
                        assert!(got.is_ok(), "acquire of a free resource must succeed");
                        model[ri] = Some(owner);
                    }
                    _ => assert_eq!(
                        got,
                        Err(LeaseError::AlreadyHeld),
                        "acquire of a held or physically aliased resource must be rejected"
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
        assert_eq!(ResourceLease::acquire(Resource::Twim1, 7), Ok(()));
        assert_eq!(ResourceLease::acquire(Resource::Radio, 8), Ok(()));
        assert_eq!(ResourceLease::owner(Resource::Twim0), Some(7));

        let twim_before = crate::quiesce::count(Resource::Twim0);
        let twim1_before = crate::quiesce::count(Resource::Twim1);
        let receipt = ResourceLease::recover_owner(7);
        assert_eq!(receipt.released_count(), 2);
        assert!(receipt.released(Resource::Twim0));
        assert!(receipt.released(Resource::Twim1));
        assert!(!receipt.released(Resource::Radio));
        assert_eq!(crate::quiesce::count(Resource::Twim0), twim_before + 1);
        assert_eq!(crate::quiesce::count(Resource::Twim1), twim1_before + 1);
        assert!(!ResourceLease::is_held(Resource::Twim0));
        assert!(!ResourceLease::is_held(Resource::Twim1));
        assert_eq!(ResourceLease::owner(Resource::Radio), Some(8));
        assert_eq!(ResourceLease::release(Resource::Radio, 8), Ok(()));
        reset_all();
    }

    #[test]
    fn shared_twim0_spim0_block_has_one_physical_owner() {
        let _lock = test_lock();
        reset_all();

        let twim = ResourceLease::acquire_guard(Resource::Twim0, 7).unwrap();
        assert!(matches!(
            ResourceLease::acquire_guard(Resource::Spim0, 8),
            Err(LeaseError::AlreadyHeld)
        ));
        drop(twim);

        let spim = ResourceLease::acquire_guard(Resource::Spim0, 8).unwrap();
        assert_eq!(
            ResourceLease::acquire(Resource::Twim0, 7),
            Err(LeaseError::AlreadyHeld)
        );
        drop(spim);

        assert_eq!(ResourceLease::acquire(Resource::Twim0, 7), Ok(()));
        assert_eq!(ResourceLease::release(Resource::Twim0, 7), Ok(()));
        reset_all();
    }

    #[test]
    fn coarse_gpio_bank_excludes_every_pin_mux_owner() {
        let _lock = test_lock();
        for peripheral in [
            Resource::Twim0,
            Resource::Twim1,
            Resource::Spim0,
            Resource::Pwm0,
            Resource::Gpiote,
            Resource::Uarte0,
            Resource::Saadc,
        ] {
            reset_all();
            let gpio = ResourceLease::acquire_guard(Resource::Gpio, 4).unwrap();
            assert!(matches!(
                ResourceLease::acquire_guard(peripheral, 5),
                Err(LeaseError::AlreadyHeld)
            ));
            drop(gpio);
            let owner = ResourceLease::acquire_guard(peripheral, 5).unwrap();
            assert!(matches!(
                ResourceLease::acquire_guard(Resource::Gpio, 4),
                Err(LeaseError::AlreadyHeld)
            ));
            drop(owner);
        }
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
    fn generation_rollover_advances_epoch_instead_of_reviving_old_guard() {
        let _lock = test_lock();
        reset_all();
        let slot = &SLOTS[idx(Resource::Pwm0)];
        slot.generation.store(u32::MAX - 1, Ordering::Release);

        let stale = ResourceLease::acquire_guard(Resource::Pwm0, 3).unwrap();
        assert_eq!(stale.ensure_live(), Ok(()));
        assert_eq!(stale.release(), Ok(()));
        let current = ResourceLease::acquire_guard(Resource::Pwm0, 3).unwrap();
        assert_eq!(slot.epoch.load(Ordering::Acquire), 1);
        assert_eq!(current.ensure_live(), Ok(()));
        assert_eq!(
            ResourceLease::release_token(Resource::Pwm0, 3, 0, u32::MAX),
            Err(LeaseError::NotHeld)
        );
        assert_eq!(current.release(), Ok(()));

        slot.epoch.store(u16::MAX, Ordering::Release);
        slot.generation.store(u32::MAX, Ordering::Release);
        assert!(matches!(
            ResourceLease::acquire_guard(Resource::Pwm0, 3),
            Err(LeaseError::GenerationExhausted)
        ));
        assert!(!ResourceLease::is_held(Resource::Pwm0));
        reset_all();
    }

    #[test]
    #[cfg(feature = "platform-nrf52840")]
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

    #[test]
    fn concurrent_fault_cleanup_is_owner_scoped_and_generation_safe() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let _lock = test_lock();
        reset_all();
        let stale_a = ResourceLease::acquire_guard(Resource::Spim0, 21).unwrap();
        let stale_b = ResourceLease::acquire_guard(Resource::Radio, 22).unwrap();
        let healthy = ResourceLease::acquire_guard(Resource::Pwm0, 23).unwrap();
        let start = Arc::new(Barrier::new(3));
        let start_a = Arc::clone(&start);
        let start_b = Arc::clone(&start);
        let a = thread::spawn(move || {
            start_a.wait();
            ResourceLease::recover_owner(21)
        });
        let b = thread::spawn(move || {
            start_b.wait();
            ResourceLease::recover_owner(22)
        });
        start.wait();
        let receipt_a = a.join().unwrap();
        let receipt_b = b.join().unwrap();
        assert!(receipt_a.released(Resource::Spim0));
        assert!(receipt_b.released(Resource::Radio));
        assert_eq!(stale_a.ensure_live(), Err(LeaseError::NotHeld));
        assert_eq!(stale_b.ensure_live(), Err(LeaseError::NotHeld));
        assert_eq!(healthy.ensure_live(), Ok(()));
        drop(stale_a);
        drop(stale_b);
        drop(healthy);
        reset_all();
    }

    #[test]
    fn isolated_peripheral_guard_binds_allowlist_and_fault_generation() {
        use crate::{
            IsolationArchitecture, IsolationCapabilities, IsolationEpoch, IsolationPlan,
            IsolationRegion,
        };

        let _lock = test_lock();
        reset_all();
        static EPOCH: IsolationEpoch = IsolationEpoch::new();
        let mut plan = IsolationPlan::<4>::new(31, 31);
        plan.add(IsolationRegion::code(0, 1024 * 1024)).unwrap();
        plan.add(IsolationRegion::data(0x2000_0000, 256)).unwrap();
        plan.add(IsolationRegion::stack(0x2000_0100, 256)).unwrap();
        plan.add(IsolationRegion::peripheral(
            0x4000_3000,
            4096,
            Resource::Twim1.isolation_id(),
        ))
        .unwrap();
        let capabilities = IsolationCapabilities::pmsa(1, 1, IsolationArchitecture::PmsaV7M, 8);
        let receipt = EPOCH.admit(&plan, capabilities).unwrap();
        assert!(matches!(
            ResourceLease::acquire_guard_isolated(Resource::Pwm0, receipt),
            Err(LeaseError::IsolationDenied)
        ));
        let guard = ResourceLease::acquire_guard_isolated(Resource::Twim1, receipt).unwrap();
        EPOCH.activate(receipt).unwrap();
        assert_eq!(guard.ensure_live(), Ok(()));
        EPOCH.fault(receipt).unwrap();
        assert_eq!(guard.ensure_live(), Err(LeaseError::IsolationStale));
        drop(guard);
        reset_all();
    }
}
