//! Generation-safe leases for the native RA4M1 composition.
//!
//! The table names physical controller instances, not driver brands. Alternate
//! programming modes that touch the same registers are rejected as conflicts.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use nobro_hal::{HalLease, LeaseClass, LeaseError, LeaseId};

const HEADER_PIN_COUNT: usize = 20;
const GPIO_BASE: usize = 16;
const IRQ_BASE: usize = GPIO_BASE + HEADER_PIN_COUNT;
const PULSE0: usize = IRQ_BASE + HEADER_PIN_COUNT;
const WATCHDOG0: usize = PULSE0 + 1;
const RTC0: usize = WATCHDOG0 + 1;
const FLASH0: usize = RTC0 + 1;
const SLOT_COUNT: usize = FLASH0 + 1;

const TIMER0: usize = 0;
const TIMER1: usize = 1;
const TIMER2: usize = 2;
const IIC0: usize = 3;
const IIC1: usize = 4;
const SPI0: usize = 5;
const PWM0: usize = 6;
const EVENT_ROUTER0: usize = 7;
const SOFTWARE_EVENT0: usize = 8;
const ADC0: usize = 9;
const UART1: usize = 10;
const UART2: usize = 11;
const UART9: usize = 12;
const USB0: usize = 13;
const DMA0: usize = 14;
const POWER0: usize = 15;

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

static SLOTS: [Slot; SLOT_COUNT] = [const { Slot::new() }; SLOT_COUNT];

/// External-IRQ channel exposed by the exact UNO R4 WiFi header pin mux.
/// Pins absent from this table remain valid GPIOs but cannot be leased as IRQs.
pub const fn header_irq_channel(pin: u8) -> Option<u8> {
    match pin {
        0 => Some(6),
        1 => Some(5),
        2 => Some(1),
        3 => Some(0),
        6 => Some(4),
        8 => Some(9),
        11 => Some(4),
        12 => Some(5),
        15 => Some(6),
        16 => Some(7),
        17 => Some(2),
        18 => Some(1),
        19 => Some(2),
        _ => None,
    }
}

fn slot_for(id: LeaseId) -> Result<usize, LeaseError> {
    match (id.class, id.instance) {
        (LeaseClass::Timer, 0) => Ok(TIMER0),
        (LeaseClass::Timer, 1) => Ok(TIMER1),
        (LeaseClass::Timer, 2) => Ok(TIMER2),
        (LeaseClass::I2c, 0) => Ok(IIC0),
        (LeaseClass::I2c, 1) => Ok(IIC1),
        (LeaseClass::Spi, 0) => Ok(SPI0),
        (LeaseClass::Pwm, 0) => Ok(PWM0),
        (LeaseClass::EventRouter, 0) => Ok(EVENT_ROUTER0),
        (LeaseClass::SoftwareEvent, 0) => Ok(SOFTWARE_EVENT0),
        (LeaseClass::Adc, 0) => Ok(ADC0),
        (LeaseClass::Uart, 1) => Ok(UART1),
        (LeaseClass::Uart, 2) => Ok(UART2),
        (LeaseClass::Uart, 9) => Ok(UART9),
        (LeaseClass::Usb, 0) => Ok(USB0),
        (LeaseClass::Dma, 0) => Ok(DMA0),
        (LeaseClass::Power, 0) => Ok(POWER0),
        (LeaseClass::Gpio, pin) if usize::from(pin) < HEADER_PIN_COUNT => {
            Ok(GPIO_BASE + usize::from(pin))
        }
        (LeaseClass::Irq, pin) if header_irq_channel(pin).is_some() => {
            Ok(IRQ_BASE + usize::from(pin))
        }
        (LeaseClass::Pulse, 0) => Ok(PULSE0),
        (LeaseClass::Watchdog, 0) => Ok(WATCHDOG0),
        (LeaseClass::Rtc, 0) => Ok(RTC0),
        (LeaseClass::Flash, 0) => Ok(FLASH0),
        _ => Err(LeaseError::Unsupported),
    }
}

fn conflict_mask(slot: usize) -> u64 {
    let own = 1u64 << slot;
    let gpio_irq_pair = |pin: usize| (1u64 << (GPIO_BASE + pin)) | (1u64 << (IRQ_BASE + pin));
    match slot {
        // The event-DMA composition owns GPT0 as its pacer and GPT1 as its
        // timeout counter, so it cannot overlap the ordinary GPT0 PWM provider.
        PWM0 => own | (1u64 << DMA0) | gpio_irq_pair(5),
        DMA0 => own | (1u64 << PWM0) | (1u64 << EVENT_ROUTER0),
        EVENT_ROUTER0 => own | (1u64 << DMA0),
        ADC0 => own | gpio_irq_pair(14),
        SPI0 => {
            own | gpio_irq_pair(10)
                | gpio_irq_pair(11)
                | gpio_irq_pair(12)
                | gpio_irq_pair(13)
                | (1u64 << PULSE0)
        }
        IIC0 => own | gpio_irq_pair(18) | gpio_irq_pair(19),
        UART2 => own | gpio_irq_pair(0) | gpio_irq_pair(1),
        PULSE0 => own | gpio_irq_pair(13) | (1u64 << SPI0),
        slot if (GPIO_BASE..GPIO_BASE + HEADER_PIN_COUNT).contains(&slot) => {
            let pin = slot - GPIO_BASE;
            own | (1u64 << (IRQ_BASE + pin)) | peripheral_pin_conflicts(pin)
        }
        slot if (IRQ_BASE..IRQ_BASE + HEADER_PIN_COUNT).contains(&slot) => {
            let pin = slot - IRQ_BASE;
            own | (1u64 << (GPIO_BASE + pin)) | peripheral_pin_conflicts(pin)
        }
        _ => own,
    }
}

fn peripheral_pin_conflicts(pin: usize) -> u64 {
    match pin {
        0 | 1 => 1u64 << UART2,
        5 => 1u64 << PWM0,
        10..=12 => 1u64 << SPI0,
        13 => (1u64 << SPI0) | (1u64 << PULSE0),
        14 => 1u64 << ADC0,
        18 | 19 => 1u64 << IIC0,
        _ => 0,
    }
}

fn has_conflict(slot: usize) -> bool {
    let mask = conflict_mask(slot);
    SLOTS.iter().enumerate().any(|(index, candidate)| {
        mask & (1u64 << index) != 0 && candidate.held.load(Ordering::Acquire)
    })
}

fn advance_generation(slot: &Slot) {
    let current = slot.generation.load(Ordering::Acquire);
    slot.generation
        .store(current.saturating_add(1), Ordering::Release);
}

pub struct Ra4m1Leases;

impl Ra4m1Leases {
    pub fn acquire_guard(id: LeaseId, owner: u8) -> Result<Ra4m1LeaseGuard, LeaseError> {
        critical_section::with(|_| {
            let index = slot_for(id)?;
            if has_conflict(index) {
                return Err(LeaseError::AlreadyHeld);
            }
            let slot = &SLOTS[index];
            if slot.generation.load(Ordering::Acquire) == u32::MAX {
                return Err(LeaseError::GenerationExhausted);
            }
            let generation = slot.generation.load(Ordering::Acquire);
            slot.owner.store(owner, Ordering::Release);
            slot.held.store(true, Ordering::Release);
            Ok(Ra4m1LeaseGuard {
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
            quiesce(id);
            slot.held.store(false, Ordering::Release);
            slot.owner.store(0, Ordering::Release);
            advance_generation(slot);
            Ok(())
        })
    }
}

impl HalLease for Ra4m1Leases {
    fn acquire(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        let id = resource.into();
        critical_section::with(|_| {
            let index = slot_for(id)?;
            if has_conflict(index) {
                return Err(LeaseError::AlreadyHeld);
            }
            let slot = &SLOTS[index];
            slot.owner.store(owner, Ordering::Release);
            slot.held.store(true, Ordering::Release);
            Ok(())
        })
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
            quiesce(id);
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
            for (index, slot) in SLOTS.iter().enumerate() {
                if slot.held.load(Ordering::Acquire) && slot.owner.load(Ordering::Acquire) == owner
                {
                    quiesce(id_for_slot(index));
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

pub struct Ra4m1LeaseGuard {
    id: LeaseId,
    owner: u8,
    generation: u32,
    active: bool,
}

impl Ra4m1LeaseGuard {
    pub const fn id(&self) -> LeaseId {
        self.id
    }

    pub const fn owner(&self) -> u8 {
        self.owner
    }

    pub fn ensure_live(&self) -> Result<(), LeaseError> {
        Ra4m1Leases::token_is_live(self.id, self.owner, self.generation)
            .then_some(())
            .ok_or(LeaseError::NotHeld)
    }

    pub fn release(mut self) -> Result<(), LeaseError> {
        Ra4m1Leases::release_token(self.id, self.owner, self.generation)?;
        self.active = false;
        Ok(())
    }
}

impl Drop for Ra4m1LeaseGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = Ra4m1Leases::release_token(self.id, self.owner, self.generation);
            self.active = false;
        }
    }
}

fn id_for_slot(slot: usize) -> LeaseId {
    match slot {
        TIMER0 => LeaseId::SYSTEM_TIMER,
        TIMER1 => LeaseId::DEADLINE_TIMER,
        TIMER2 => LeaseId::EVENT_CAPTURE_TIMER,
        IIC0 => LeaseId::PRIMARY_I2C,
        IIC1 => LeaseId::SECONDARY_I2C,
        SPI0 => LeaseId::PRIMARY_SPI,
        PWM0 => LeaseId::PRIMARY_PWM,
        EVENT_ROUTER0 => LeaseId::EVENT_ROUTER,
        SOFTWARE_EVENT0 => LeaseId::SOFTWARE_EVENT,
        ADC0 => LeaseId::PRIMARY_ADC,
        UART1 => LeaseId::new(LeaseClass::Uart, 1),
        UART2 => LeaseId::new(LeaseClass::Uart, 2),
        UART9 => LeaseId::new(LeaseClass::Uart, 9),
        USB0 => LeaseId::USB_DEVICE,
        DMA0 => LeaseId::PRIMARY_DMA,
        POWER0 => LeaseId::SYSTEM_POWER,
        GPIO_BASE..=35 => LeaseId::new(LeaseClass::Gpio, (slot - GPIO_BASE) as u8),
        IRQ_BASE..=55 => LeaseId::new(LeaseClass::Irq, (slot - IRQ_BASE) as u8),
        PULSE0 => LeaseId::PRIMARY_PULSE,
        WATCHDOG0 => LeaseId::SYSTEM_WATCHDOG,
        RTC0 => LeaseId::SYSTEM_RTC,
        FLASH0 => LeaseId::APPLICATION_FLASH,
        _ => unreachable!(),
    }
}

#[cfg(target_arch = "arm")]
fn quiesce(id: LeaseId) {
    unsafe {
        match (id.class, id.instance) {
            (LeaseClass::Timer, 0) => write8(0x4008_4008, 0xf4),
            (LeaseClass::Timer, 1) => write8(0x4008_4108, 0xf4),
            (LeaseClass::Timer, 2) => write32(0x4007_8208, 1 << 2),
            (LeaseClass::I2c, 0) => write8(0x4005_3000, 0),
            (LeaseClass::I2c, 1) => write8(0x4005_3100, 0),
            (LeaseClass::Spi, 0) => {
                let value = read8(0x4007_2000) & !(1 << 6);
                write8(0x4007_2000, value);
            }
            (LeaseClass::Pwm, 0) => write32(0x4007_8008, 1),
            (LeaseClass::Uart, 1) => write8(0x4007_0022, 0),
            (LeaseClass::Uart, 2) => write8(0x4007_0042, 0),
            (LeaseClass::Uart, 9) => write8(0x4007_0122, 0),
            (LeaseClass::Adc, 0) => {
                let value = read16(0x4005_C000) & !(1 << 15);
                write16(0x4005_C000, value);
            }
            (LeaseClass::Dma, 0) => write8(0x4000_501c, 0),
            // USB has a stack-owned disconnect path; silently rewriting its
            // controller from a generic lease callback would violate that owner.
            _ => {}
        }
    }
}

#[cfg(not(target_arch = "arm"))]
fn quiesce(_: LeaseId) {}

#[cfg(target_arch = "arm")]
unsafe fn read8(address: usize) -> u8 {
    (address as *const u8).read_volatile()
}

#[cfg(target_arch = "arm")]
unsafe fn write8(address: usize, value: u8) {
    (address as *mut u8).write_volatile(value);
}

#[cfg(target_arch = "arm")]
unsafe fn read16(address: usize) -> u16 {
    (address as *const u16).read_volatile()
}

#[cfg(target_arch = "arm")]
unsafe fn write16(address: usize, value: u16) {
    (address as *mut u16).write_volatile(value);
}

#[cfg(target_arch = "arm")]
unsafe fn write32(address: usize, value: u32) {
    (address as *mut u32).write_volatile(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn leases_enforce_owner_and_drop_release() {
        let _lock = test_lock();
        let guard = Ra4m1Leases::acquire_guard(LeaseId::PRIMARY_I2C, 7).unwrap();
        assert_eq!(guard.ensure_live(), Ok(()));
        assert_eq!(
            Ra4m1Leases::acquire(LeaseId::PRIMARY_I2C, 8),
            Err(LeaseError::AlreadyHeld)
        );
        assert_eq!(
            Ra4m1Leases::release(LeaseId::PRIMARY_I2C, 8),
            Err(LeaseError::WrongOwner)
        );
        drop(guard);
        assert!(!Ra4m1Leases::is_held(LeaseId::PRIMARY_I2C));
    }

    #[test]
    fn owner_recovery_invalidates_stale_generation() {
        let _lock = test_lock();
        let stale = Ra4m1Leases::acquire_guard(LeaseId::PRIMARY_SPI, 4).unwrap();
        assert_eq!(Ra4m1Leases::release_all_for_owner(4), 1);
        assert_eq!(stale.ensure_live(), Err(LeaseError::NotHeld));
        let current = Ra4m1Leases::acquire_guard(LeaseId::PRIMARY_SPI, 4).unwrap();
        assert_eq!(current.ensure_live(), Ok(()));
        drop(current);
        drop(stale);
    }

    #[test]
    fn event_dma_and_pwm_cannot_alias_gpt0() {
        let _lock = test_lock();
        let pwm = Ra4m1Leases::acquire_guard(LeaseId::PRIMARY_PWM, 1).unwrap();
        assert!(matches!(
            Ra4m1Leases::acquire_guard(LeaseId::PRIMARY_DMA, 2),
            Err(LeaseError::AlreadyHeld)
        ));
        drop(pwm);
        let dma = Ra4m1Leases::acquire_guard(LeaseId::PRIMARY_DMA, 2).unwrap();
        assert!(matches!(
            Ra4m1Leases::acquire_guard(LeaseId::EVENT_ROUTER, 3),
            Err(LeaseError::AlreadyHeld)
        ));
        drop(dma);
    }

    #[test]
    fn unsupported_instances_fail_closed() {
        let _lock = test_lock();
        assert_eq!(
            Ra4m1Leases::acquire(LeaseId::new(LeaseClass::Spi, 3), 1),
            Err(LeaseError::Unsupported)
        );
        assert_eq!(header_irq_channel(12), Some(5));
        assert_eq!(header_irq_channel(13), None);
        assert_eq!(
            Ra4m1Leases::acquire(LeaseId::new(LeaseClass::Irq, 13), 1),
            Err(LeaseError::Unsupported)
        );
    }
}
