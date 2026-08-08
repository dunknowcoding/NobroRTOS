//! Generation-safe ownership for the exact SAMD21 composition.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use nobro_hal::{HalLease, LeaseClass, LeaseError, LeaseId};

const SLOT_COUNT: usize = 17;
const RTC0: usize = 0;
const TC4: usize = 1;
const EIC7: usize = 2;
const DMAC0: usize = 3;
const TCC1: usize = 4;
const SERCOM0: usize = 5;
const SERCOM3: usize = 6;
const SERCOM4: usize = 7;
const USB0: usize = 8;
const POWER0: usize = 9;
const ADC0: usize = 10;
const GPIO_D8: usize = 11;
const GPIO_D9: usize = 12;
const EVSYS0: usize = 13;
const PULSE0: usize = 14;
const WATCHDOG0: usize = 15;
const FLASH0: usize = 16;

struct Slot {
    held: AtomicBool,
    owner: AtomicU8,
    generation: AtomicU32,
}

impl Slot {
    const fn new() -> Self {
        Self {
            held: AtomicBool::new(false),
            owner: AtomicU8::new(0),
            generation: AtomicU32::new(1),
        }
    }
}

static SLOTS: [Slot; SLOT_COUNT] = [
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
    Slot::new(),
];

pub const RTC_LEASE: LeaseId = LeaseId::new(LeaseClass::Timer, 0);
pub const TC4_DEADLINE_LEASE: LeaseId = LeaseId::new(LeaseClass::Timer, 4);
pub const EIC7_LEASE: LeaseId = LeaseId::new(LeaseClass::EventRouter, 7);
pub const EVSYS0_LEASE: LeaseId = LeaseId::new(LeaseClass::SoftwareEvent, 0);
pub const DMAC0_LEASE: LeaseId = LeaseId::new(LeaseClass::Dma, 0);
pub const TCC1_PWM_LEASE: LeaseId = LeaseId::new(LeaseClass::Pwm, 1);
pub const SERCOM0_UART_LEASE: LeaseId = LeaseId::new(LeaseClass::Uart, 0);
pub const SERCOM3_I2C_LEASE: LeaseId = LeaseId::new(LeaseClass::I2c, 3);
pub const SERCOM4_SPI_LEASE: LeaseId = LeaseId::new(LeaseClass::Spi, 4);
pub const USB_LEASE: LeaseId = LeaseId::new(LeaseClass::Usb, 0);
pub const POWER_LEASE: LeaseId = LeaseId::new(LeaseClass::Power, 0);
pub const ADC0_LEASE: LeaseId = LeaseId::new(LeaseClass::Adc, 0);
pub const D8_GPIO_LEASE: LeaseId = LeaseId::new(LeaseClass::SoftwareEvent, 8);
pub const D9_GPIO_LEASE: LeaseId = LeaseId::new(LeaseClass::SoftwareEvent, 9);
pub const PULSE_LEASE: LeaseId = LeaseId::PRIMARY_PULSE;
pub const WATCHDOG_LEASE: LeaseId = LeaseId::SYSTEM_WATCHDOG;
pub const FLASH_LEASE: LeaseId = LeaseId::APPLICATION_FLASH;

fn slot_for(id: LeaseId) -> Result<usize, LeaseError> {
    match (id.class, id.instance) {
        (LeaseClass::Timer, 0) => Ok(RTC0),
        (LeaseClass::Timer, 4) => Ok(TC4),
        (LeaseClass::EventRouter, 7) => Ok(EIC7),
        (LeaseClass::SoftwareEvent, 0) => Ok(EVSYS0),
        (LeaseClass::Dma, 0) => Ok(DMAC0),
        (LeaseClass::Pwm, 1) => Ok(TCC1),
        (LeaseClass::Uart, 0) => Ok(SERCOM0),
        (LeaseClass::I2c, 3) => Ok(SERCOM3),
        (LeaseClass::Spi, 4) => Ok(SERCOM4),
        (LeaseClass::Usb, 0) => Ok(USB0),
        (LeaseClass::Power, 0) => Ok(POWER0),
        (LeaseClass::Adc, 0) => Ok(ADC0),
        (LeaseClass::SoftwareEvent, 8) => Ok(GPIO_D8),
        (LeaseClass::SoftwareEvent, 9) => Ok(GPIO_D9),
        (LeaseClass::Pulse, 0) => Ok(PULSE0),
        (LeaseClass::Watchdog, 0) => Ok(WATCHDOG0),
        (LeaseClass::Flash, 0) => Ok(FLASH0),
        _ => Err(LeaseError::Unsupported),
    }
}

fn conflict_mask(slot: usize) -> u32 {
    let own = 1u32 << slot;
    match slot {
        // D9 is both the PN532 IRQ input and TCC1/WO1.  A composition must
        // select one role; silently muxing it underneath an owner is forbidden.
        EIC7 => own | (1u32 << GPIO_D9) | (1u32 << TCC1) | (1u32 << PULSE0),
        GPIO_D9 => own | (1u32 << EIC7) | (1u32 << TCC1) | (1u32 << PULSE0),
        TCC1 => own | (1u32 << EIC7) | (1u32 << GPIO_D9) | (1u32 << PULSE0),
        PULSE0 => own | (1u32 << EIC7) | (1u32 << GPIO_D9) | (1u32 << TCC1),
        _ => own,
    }
}

fn has_conflict(slot: usize) -> bool {
    let mask = conflict_mask(slot);
    SLOTS.iter().enumerate().any(|(index, candidate)| {
        mask & (1u32 << index) != 0 && candidate.held.load(Ordering::Acquire)
    })
}

fn advance_generation(slot: &Slot) {
    let current = slot.generation.load(Ordering::Acquire);
    slot.generation
        .store(current.saturating_add(1), Ordering::Release);
}

pub struct Samd21Leases;

impl Samd21Leases {
    pub fn acquire_guard(id: LeaseId, owner: u8) -> Result<Samd21LeaseGuard, LeaseError> {
        critical_section::with(|_| {
            let index = slot_for(id)?;
            if has_conflict(index) {
                return Err(LeaseError::AlreadyHeld);
            }
            let slot = &SLOTS[index];
            let generation = slot.generation.load(Ordering::Acquire);
            if generation == u32::MAX {
                return Err(LeaseError::GenerationExhausted);
            }
            slot.owner.store(owner, Ordering::Release);
            slot.held.store(true, Ordering::Release);
            Ok(Samd21LeaseGuard {
                id,
                owner,
                generation,
                active: true,
            })
        })
    }

    fn token_is_live(id: LeaseId, owner: u8, generation: u32) -> bool {
        critical_section::with(|_| {
            let Ok(index) = slot_for(id) else {
                return false;
            };
            let slot = &SLOTS[index];
            slot.held.load(Ordering::Acquire)
                && slot.owner.load(Ordering::Acquire) == owner
                && slot.generation.load(Ordering::Acquire) == generation
        })
    }

    fn release_token(id: LeaseId, owner: u8, generation: u32) -> Result<(), LeaseError> {
        critical_section::with(|_| {
            let index = slot_for(id)?;
            let slot = &SLOTS[index];
            if !slot.held.load(Ordering::Acquire)
                || slot.generation.load(Ordering::Acquire) != generation
            {
                return Err(LeaseError::NotHeld);
            }
            if slot.owner.load(Ordering::Acquire) != owner {
                return Err(LeaseError::WrongOwner);
            }
            slot.held.store(false, Ordering::Release);
            slot.owner.store(0, Ordering::Release);
            advance_generation(slot);
            Ok(())
        })
    }
}

impl HalLease for Samd21Leases {
    fn acquire(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        let id = resource.into();
        let guard = Self::acquire_guard(id, owner)?;
        core::mem::forget(guard);
        Ok(())
    }

    fn release(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        let id = resource.into();
        critical_section::with(|_| {
            let index = slot_for(id)?;
            let slot = &SLOTS[index];
            if !slot.held.load(Ordering::Acquire) {
                return Err(LeaseError::NotHeld);
            }
            if slot.owner.load(Ordering::Acquire) != owner {
                return Err(LeaseError::WrongOwner);
            }
            slot.held.store(false, Ordering::Release);
            slot.owner.store(0, Ordering::Release);
            advance_generation(slot);
            Ok(())
        })
    }

    fn is_held(resource: impl Into<LeaseId>) -> bool {
        slot_for(resource.into())
            .ok()
            .is_some_and(|index| SLOTS[index].held.load(Ordering::Acquire))
    }

    fn owner(resource: impl Into<LeaseId>) -> Option<u8> {
        slot_for(resource.into()).ok().and_then(|index| {
            let slot = &SLOTS[index];
            slot.held
                .load(Ordering::Acquire)
                .then(|| slot.owner.load(Ordering::Acquire))
        })
    }

    fn release_all_for_owner(owner: u8) -> usize {
        critical_section::with(|_| {
            let mut released = 0;
            for slot in &SLOTS {
                if slot.held.load(Ordering::Acquire) && slot.owner.load(Ordering::Acquire) == owner
                {
                    slot.held.store(false, Ordering::Release);
                    slot.owner.store(0, Ordering::Release);
                    advance_generation(slot);
                    released += 1;
                }
            }
            released
        })
    }
}

pub struct Samd21LeaseGuard {
    id: LeaseId,
    owner: u8,
    generation: u32,
    active: bool,
}

impl Samd21LeaseGuard {
    pub const fn id(&self) -> LeaseId {
        self.id
    }

    pub fn ensure_live(&self) -> Result<(), LeaseError> {
        Samd21Leases::token_is_live(self.id, self.owner, self.generation)
            .then_some(())
            .ok_or(LeaseError::NotHeld)
    }
}

impl Drop for Samd21LeaseGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = Samd21Leases::release_token(self.id, self.owner, self.generation);
            self.active = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn independent_pn532_buses_can_be_held_together() {
        let _lock = lock();
        let i2c = Samd21Leases::acquire_guard(SERCOM3_I2C_LEASE, 1).unwrap();
        let spi = Samd21Leases::acquire_guard(SERCOM4_SPI_LEASE, 2).unwrap();
        assert!(i2c.ensure_live().is_ok());
        assert!(spi.ensure_live().is_ok());
    }

    #[test]
    fn d9_irq_and_pwm_mux_conflict_fails_closed() {
        let _lock = lock();
        let irq = Samd21Leases::acquire_guard(EIC7_LEASE, 1).unwrap();
        assert!(matches!(
            Samd21Leases::acquire_guard(TCC1_PWM_LEASE, 2),
            Err(LeaseError::AlreadyHeld)
        ));
        drop(irq);
    }

    #[test]
    fn stale_generation_is_invalidated_by_owner_recovery() {
        let _lock = lock();
        let stale = Samd21Leases::acquire_guard(SERCOM4_SPI_LEASE, 9).unwrap();
        assert_eq!(Samd21Leases::release_all_for_owner(9), 1);
        assert_eq!(stale.ensure_live(), Err(LeaseError::NotHeld));
    }

    #[test]
    fn event_dma_composition_can_own_channel_and_route() {
        let _lock = lock();
        let dma = Samd21Leases::acquire_guard(DMAC0_LEASE, 6).unwrap();
        let route = Samd21Leases::acquire_guard(EVSYS0_LEASE, 6).unwrap();
        assert!(dma.ensure_live().is_ok());
        assert!(route.ensure_live().is_ok());
    }
}
