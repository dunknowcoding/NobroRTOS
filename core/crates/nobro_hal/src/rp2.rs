//! Shared RP2040/RP2350 ownership and provider contracts.
//!
//! This module deliberately contains no PAC register addresses.  The two
//! silicon ports share bounded ownership, cancellation, and backend adapters
//! here, while their port-local modules retain clock, PIO, DMA, reset, and
//! interrupt details that differ between RP2040 and RP2350.

use core::{
    marker::PhantomData,
    sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering},
};

use embedded_hal::{i2c::I2c, spi::SpiBus};

use crate::{
    HalAdcChannel, HalAlarm, HalByteIo, HalClock, HalI2c, HalLease, HalPower, HalPwmChannel,
    HalReset, HalSpi, IdleMode, LeaseClass, LeaseError, LeaseId, TransferMode,
};

/// RP-series silicon selected by an exact board composition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rp2Silicon {
    Rp2040,
    Rp2350,
}

/// Hardware limits which are intentionally not flattened across the two chips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rp2RuntimeContract {
    pub silicon: Rp2Silicon,
    pub cores: u8,
    pub pio_blocks: u8,
    pub pio_state_machines: u8,
    pub dma_channels: u8,
    pub pwm_slices: u8,
    pub adc_external_inputs: u8,
    pub gpio_count: u8,
    pub usb_device: bool,
    pub xip_cache: bool,
}

pub const RP2040_RUNTIME: Rp2RuntimeContract = Rp2RuntimeContract {
    silicon: Rp2Silicon::Rp2040,
    cores: 2,
    pio_blocks: 2,
    pio_state_machines: 8,
    dma_channels: 12,
    pwm_slices: 8,
    adc_external_inputs: 4,
    gpio_count: 30,
    usb_device: true,
    xip_cache: true,
};

pub const RP2350_RUNTIME: Rp2RuntimeContract = Rp2RuntimeContract {
    silicon: Rp2Silicon::Rp2350,
    cores: 2,
    pio_blocks: 3,
    pio_state_machines: 12,
    dma_channels: 16,
    pwm_slices: 12,
    adc_external_inputs: 4,
    // The exact Pico 2 W composition uses the 30-GPIO RP2350A package.
    gpio_count: 30,
    usb_device: true,
    xip_cache: true,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rp2ContractError {
    InvalidChannel,
    EmptyTransfer,
    TransferTooLong,
    InvalidDreq,
    InvalidPioBlock,
    InvalidStateMachine,
    ProgramTooLong,
    InvalidPinWindow,
}

/// Silicon-checked DMA admission shared by the two concrete DMA engines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rp2DmaPlan {
    pub silicon: Rp2Silicon,
    pub channel: u8,
    pub words: u16,
    pub dreq: Option<u8>,
}

impl Rp2DmaPlan {
    pub const MAX_STAGED_WORDS: u16 = 1_024;

    pub const fn new(
        runtime: Rp2RuntimeContract,
        channel: u8,
        words: u16,
        dreq: Option<u8>,
    ) -> Result<Self, Rp2ContractError> {
        if channel >= runtime.dma_channels {
            return Err(Rp2ContractError::InvalidChannel);
        }
        if words == 0 {
            return Err(Rp2ContractError::EmptyTransfer);
        }
        if words > Self::MAX_STAGED_WORDS {
            return Err(Rp2ContractError::TransferTooLong);
        }
        if matches!(dreq, Some(value) if value > 63) {
            return Err(Rp2ContractError::InvalidDreq);
        }
        Ok(Self {
            silicon: runtime.silicon,
            channel,
            words,
            dreq,
        })
    }
}

/// PIO program/pin admission. RP2350's extra block count remains explicit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rp2PioPlan {
    pub silicon: Rp2Silicon,
    pub block: u8,
    pub state_machine: u8,
    pub program_words: u8,
    pub first_pin: u8,
    pub pin_count: u8,
}

impl Rp2PioPlan {
    pub const fn new(
        runtime: Rp2RuntimeContract,
        block: u8,
        state_machine: u8,
        program_words: u8,
        first_pin: u8,
        pin_count: u8,
    ) -> Result<Self, Rp2ContractError> {
        if block >= runtime.pio_blocks {
            return Err(Rp2ContractError::InvalidPioBlock);
        }
        if state_machine >= 4 {
            return Err(Rp2ContractError::InvalidStateMachine);
        }
        if program_words == 0 || program_words > 32 {
            return Err(Rp2ContractError::ProgramTooLong);
        }
        if pin_count == 0
            || first_pin >= runtime.gpio_count
            || first_pin.saturating_add(pin_count) > runtime.gpio_count
        {
            return Err(Rp2ContractError::InvalidPinWindow);
        }
        Ok(Self {
            silicon: runtime.silicon,
            block,
            state_machine,
            program_words,
            first_pin,
            pin_count,
        })
    }
}

/// Shared logical resources.  Instance counts remain bounded by the selected
/// [`Rp2RuntimeContract`]; a port must not expose a non-existent channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Rp2Resource {
    SystemTimer,
    DeadlineAlarm,
    Pio0,
    Dma0,
    Pwm0,
    Adc,
    I2c0,
    I2c1,
    Spi0,
    Spi1,
    Uart0,
    Uart1,
    Usb,
    Power,
    Reset,
    Core1,
    Cyw43,
}

impl Rp2Resource {
    pub const ALL: [Self; 17] = [
        Self::SystemTimer,
        Self::DeadlineAlarm,
        Self::Pio0,
        Self::Dma0,
        Self::Pwm0,
        Self::Adc,
        Self::I2c0,
        Self::I2c1,
        Self::Spi0,
        Self::Spi1,
        Self::Uart0,
        Self::Uart1,
        Self::Usb,
        Self::Power,
        Self::Reset,
        Self::Core1,
        Self::Cyw43,
    ];

    pub const fn lease_id(self) -> LeaseId {
        match self {
            Self::SystemTimer => LeaseId::SYSTEM_TIMER,
            Self::DeadlineAlarm => LeaseId::DEADLINE_TIMER,
            Self::Pio0 => LeaseId::PRIMARY_PIO,
            Self::Dma0 => LeaseId::PRIMARY_DMA,
            Self::Pwm0 => LeaseId::PRIMARY_PWM,
            Self::Adc => LeaseId::PRIMARY_ADC,
            Self::I2c0 => LeaseId::PRIMARY_I2C,
            Self::I2c1 => LeaseId::SECONDARY_I2C,
            Self::Spi0 => LeaseId::PRIMARY_SPI,
            Self::Spi1 => LeaseId::new(LeaseClass::Spi, 1),
            Self::Uart0 => LeaseId::PRIMARY_UART,
            Self::Uart1 => LeaseId::new(LeaseClass::Uart, 1),
            Self::Usb => LeaseId::USB_DEVICE,
            Self::Power => LeaseId::SYSTEM_POWER,
            Self::Reset => LeaseId::SYSTEM_RESET,
            Self::Core1 => LeaseId::SECONDARY_CORE,
            Self::Cyw43 => LeaseId::PRIMARY_RADIO,
        }
    }
}

const RP2_LEASE_SLOT_COUNT: usize = 45;

/// Map every resource present on either RP2040 or RP2350 into one fixed slot.
///
/// Runtime-aware constructors below reject the RP2350-only tail instances
/// when an RP2040 composition is selected.
const fn lease_slot_index(id: LeaseId) -> Option<usize> {
    match id.class {
        LeaseClass::Timer if id.instance < 2 => Some(id.instance as usize),
        LeaseClass::Pio if id.instance < 3 => Some(2 + id.instance as usize),
        LeaseClass::Dma if id.instance < 16 => Some(5 + id.instance as usize),
        LeaseClass::Pwm if id.instance < 12 => Some(21 + id.instance as usize),
        LeaseClass::Adc if id.instance == 0 => Some(33),
        LeaseClass::I2c if id.instance < 2 => Some(34 + id.instance as usize),
        LeaseClass::Spi if id.instance < 2 => Some(36 + id.instance as usize),
        LeaseClass::Uart if id.instance < 2 => Some(38 + id.instance as usize),
        LeaseClass::Usb if id.instance == 0 => Some(40),
        LeaseClass::Power if id.instance == 0 => Some(41),
        LeaseClass::Reset if id.instance == 0 => Some(42),
        LeaseClass::Core if id.instance == 1 => Some(43),
        LeaseClass::Radio if id.instance == 0 => Some(44),
        _ => None,
    }
}

struct LeaseSlot {
    held: AtomicBool,
    owner: AtomicU8,
    generation: AtomicU32,
}

impl LeaseSlot {
    const fn new() -> Self {
        Self {
            held: AtomicBool::new(false),
            owner: AtomicU8::new(0),
            generation: AtomicU32::new(1),
        }
    }
}

static RP2_LEASE_SLOTS: [LeaseSlot; RP2_LEASE_SLOT_COUNT] =
    [const { LeaseSlot::new() }; RP2_LEASE_SLOT_COUNT];

/// Process-wide lease authority for one exact RP-series firmware image.
pub struct Rp2Leases;

impl Rp2Leases {
    pub fn acquire_guard(resource: Rp2Resource, owner: u8) -> Result<Rp2LeaseGuard, LeaseError> {
        Self::acquire_id(resource.lease_id(), owner)
    }

    fn acquire_id(resource: LeaseId, owner: u8) -> Result<Rp2LeaseGuard, LeaseError> {
        let index = lease_slot_index(resource).ok_or(LeaseError::Unsupported)?;
        critical_section::with(|_| {
            let slot = &RP2_LEASE_SLOTS[index];
            if slot.held.load(Ordering::Acquire) {
                return Err(LeaseError::AlreadyHeld);
            }
            if slot.generation.load(Ordering::Acquire) == u32::MAX {
                return Err(LeaseError::GenerationExhausted);
            }
            slot.owner.store(owner, Ordering::Release);
            slot.held.store(true, Ordering::Release);
            Ok(Rp2LeaseGuard {
                resource,
                owner,
                generation: slot.generation.load(Ordering::Acquire),
                live: true,
            })
        })
    }

    pub fn acquire_pio(
        runtime: Rp2RuntimeContract,
        block: u8,
        owner: u8,
    ) -> Result<Rp2LeaseGuard, LeaseError> {
        if block >= runtime.pio_blocks {
            return Err(LeaseError::Unsupported);
        }
        Self::acquire_id(LeaseId::new(LeaseClass::Pio, block), owner)
    }

    pub fn acquire_dma(
        runtime: Rp2RuntimeContract,
        channel: u8,
        owner: u8,
    ) -> Result<Rp2LeaseGuard, LeaseError> {
        if channel >= runtime.dma_channels {
            return Err(LeaseError::Unsupported);
        }
        Self::acquire_id(LeaseId::new(LeaseClass::Dma, channel), owner)
    }

    pub fn acquire_pwm(
        runtime: Rp2RuntimeContract,
        slice: u8,
        owner: u8,
    ) -> Result<Rp2LeaseGuard, LeaseError> {
        if slice >= runtime.pwm_slices {
            return Err(LeaseError::Unsupported);
        }
        Self::acquire_id(LeaseId::new(LeaseClass::Pwm, slice), owner)
    }

    fn release_exact(
        resource: LeaseId,
        owner: u8,
        generation: Option<u32>,
    ) -> Result<(), LeaseError> {
        let index = lease_slot_index(resource).ok_or(LeaseError::Unsupported)?;
        critical_section::with(|_| {
            let slot = &RP2_LEASE_SLOTS[index];
            if !slot.held.load(Ordering::Acquire) {
                return Err(LeaseError::NotHeld);
            }
            if slot.owner.load(Ordering::Acquire) != owner {
                return Err(LeaseError::WrongOwner);
            }
            if generation
                .is_some_and(|expected| slot.generation.load(Ordering::Acquire) != expected)
            {
                return Err(LeaseError::NotHeld);
            }
            slot.held.store(false, Ordering::Release);
            slot.owner.store(0, Ordering::Release);
            let next = slot.generation.load(Ordering::Acquire).saturating_add(1);
            slot.generation.store(next, Ordering::Release);
            Ok(())
        })
    }

    pub fn recover_owner(owner: u8) -> usize {
        critical_section::with(|_| {
            let mut released = 0;
            for slot in &RP2_LEASE_SLOTS {
                if slot.held.load(Ordering::Acquire) && slot.owner.load(Ordering::Acquire) == owner
                {
                    slot.held.store(false, Ordering::Release);
                    slot.owner.store(0, Ordering::Release);
                    let next = slot.generation.load(Ordering::Acquire).saturating_add(1);
                    slot.generation.store(next, Ordering::Release);
                    released += 1;
                }
            }
            released
        })
    }
}

impl HalLease for Rp2Leases {
    fn acquire(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        let guard = Self::acquire_id(resource.into(), owner)?;
        core::mem::forget(guard);
        Ok(())
    }

    fn release(resource: impl Into<LeaseId>, owner: u8) -> Result<(), LeaseError> {
        Self::release_exact(resource.into(), owner, None)
    }

    fn is_held(resource: impl Into<LeaseId>) -> bool {
        lease_slot_index(resource.into())
            .is_some_and(|index| RP2_LEASE_SLOTS[index].held.load(Ordering::Acquire))
    }

    fn owner(resource: impl Into<LeaseId>) -> Option<u8> {
        lease_slot_index(resource.into()).and_then(|index| {
            let slot = &RP2_LEASE_SLOTS[index];
            slot.held
                .load(Ordering::Acquire)
                .then(|| slot.owner.load(Ordering::Acquire))
        })
    }

    fn release_all_for_owner(owner: u8) -> usize {
        Self::recover_owner(owner)
    }
}

/// Generation-checked resource guard.  Owner recovery invalidates stale guards.
pub struct Rp2LeaseGuard {
    resource: LeaseId,
    owner: u8,
    generation: u32,
    live: bool,
}

impl Rp2LeaseGuard {
    pub fn ensure_live(&self) -> Result<(), LeaseError> {
        let index = lease_slot_index(self.resource).ok_or(LeaseError::NotHeld)?;
        let slot = &RP2_LEASE_SLOTS[index];
        if self.live
            && slot.held.load(Ordering::Acquire)
            && slot.owner.load(Ordering::Acquire) == self.owner
            && slot.generation.load(Ordering::Acquire) == self.generation
        {
            Ok(())
        } else {
            Err(LeaseError::NotHeld)
        }
    }
}

impl Drop for Rp2LeaseGuard {
    fn drop(&mut self) {
        if self.live {
            let _ = Rp2Leases::release_exact(self.resource, self.owner, Some(self.generation));
            self.live = false;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rp2ProviderError<E> {
    Lease(LeaseError),
    Backend(E),
    InvalidConfig,
    LengthMismatch,
    Timeout,
}

pub struct Rp2I2c<B> {
    backend: B,
    lease: Rp2LeaseGuard,
}

impl<B> Rp2I2c<B> {
    pub fn try_new(backend: B, instance: u8, owner: u8) -> Result<Self, LeaseError> {
        let resource = match instance {
            0 => Rp2Resource::I2c0,
            1 => Rp2Resource::I2c1,
            _ => return Err(LeaseError::Unsupported),
        };
        Ok(Self {
            backend,
            lease: Rp2Leases::acquire_guard(resource, owner)?,
        })
    }

    pub fn into_inner(self) -> B {
        self.backend
    }
}

impl<B: I2c> HalI2c for Rp2I2c<B> {
    type Error = Rp2ProviderError<B::Error>;
    const TRANSFER_MODE: TransferMode = TransferMode::Polling;

    fn write(&mut self, address: u8, bytes: &[u8]) -> Result<(), Self::Error> {
        if address >= 0x80 || bytes.is_empty() {
            return Err(Rp2ProviderError::InvalidConfig);
        }
        self.lease.ensure_live().map_err(Rp2ProviderError::Lease)?;
        self.backend
            .write(address, bytes)
            .map_err(Rp2ProviderError::Backend)
    }

    fn read(&mut self, address: u8, bytes: &mut [u8]) -> Result<(), Self::Error> {
        if address >= 0x80 || bytes.is_empty() {
            return Err(Rp2ProviderError::InvalidConfig);
        }
        self.lease.ensure_live().map_err(Rp2ProviderError::Lease)?;
        self.backend
            .read(address, bytes)
            .map_err(Rp2ProviderError::Backend)
    }

    fn write_read(
        &mut self,
        address: u8,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<(), Self::Error> {
        if address >= 0x80 || write.is_empty() || read.is_empty() {
            return Err(Rp2ProviderError::InvalidConfig);
        }
        self.lease.ensure_live().map_err(Rp2ProviderError::Lease)?;
        self.backend
            .write_read(address, write, read)
            .map_err(Rp2ProviderError::Backend)
    }
}

pub struct Rp2Spi<B> {
    backend: B,
    lease: Rp2LeaseGuard,
}

impl<B> Rp2Spi<B> {
    pub fn try_new(backend: B, instance: u8, owner: u8) -> Result<Self, LeaseError> {
        let resource = match instance {
            0 => Rp2Resource::Spi0,
            1 => Rp2Resource::Spi1,
            _ => return Err(LeaseError::Unsupported),
        };
        Ok(Self {
            backend,
            lease: Rp2Leases::acquire_guard(resource, owner)?,
        })
    }
}

impl<B: SpiBus<u8>> HalSpi for Rp2Spi<B> {
    type Error = Rp2ProviderError<B::Error>;
    const TRANSFER_MODE: TransferMode = TransferMode::Polling;

    fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> Result<(), Self::Error> {
        if write.len() != read.len() {
            return Err(Rp2ProviderError::LengthMismatch);
        }
        if write.is_empty() {
            return Ok(());
        }
        self.lease.ensure_live().map_err(Rp2ProviderError::Lease)?;
        self.backend
            .transfer(read, write)
            .map_err(Rp2ProviderError::Backend)
    }
}

pub trait Rp2PwmBackend {
    type Error;
    fn max_duty(&self) -> u16;
    fn set_duty(&mut self, duty: u16) -> Result<(), Self::Error>;
}

pub struct Rp2Pwm<B> {
    backend: B,
    lease: Rp2LeaseGuard,
}

impl<B> Rp2Pwm<B> {
    pub fn try_new(
        backend: B,
        runtime: Rp2RuntimeContract,
        slice: u8,
        owner: u8,
    ) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            lease: Rp2Leases::acquire_pwm(runtime, slice, owner)?,
        })
    }
}

impl<B: Rp2PwmBackend> HalPwmChannel for Rp2Pwm<B> {
    type Error = Rp2ProviderError<B::Error>;

    fn max_duty(&self) -> u16 {
        self.backend.max_duty()
    }

    fn set_duty(&mut self, duty: u16) -> Result<(), Self::Error> {
        self.lease.ensure_live().map_err(Rp2ProviderError::Lease)?;
        self.backend
            .set_duty(duty)
            .map_err(Rp2ProviderError::Backend)
    }
}

pub trait Rp2AdcBackend {
    type Error;
    fn max_sample(&self) -> u16;
    fn read(&mut self) -> Result<u16, Self::Error>;
}

pub struct Rp2Adc<B> {
    backend: B,
    lease: Rp2LeaseGuard,
}

impl<B> Rp2Adc<B> {
    pub fn try_new(backend: B, owner: u8) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            lease: Rp2Leases::acquire_guard(Rp2Resource::Adc, owner)?,
        })
    }
}

impl<B: Rp2AdcBackend> HalAdcChannel for Rp2Adc<B> {
    type Error = Rp2ProviderError<B::Error>;

    fn max_sample(&self) -> u16 {
        self.backend.max_sample()
    }

    fn read(&mut self) -> Result<u16, Self::Error> {
        self.lease.ensure_live().map_err(Rp2ProviderError::Lease)?;
        self.backend.read().map_err(Rp2ProviderError::Backend)
    }
}

pub trait Rp2ByteIoBackend {
    type Error;
    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error>;
    fn write_some(&mut self, bytes: &[u8]) -> Result<usize, Self::Error>;
    fn flush(&mut self) -> Result<(), Self::Error>;
}

pub struct Rp2ByteIo<B> {
    backend: B,
    lease: Rp2LeaseGuard,
}

impl<B> Rp2ByteIo<B> {
    pub fn try_new(backend: B, resource: Rp2Resource, owner: u8) -> Result<Self, LeaseError> {
        if !matches!(
            resource,
            Rp2Resource::Uart0 | Rp2Resource::Uart1 | Rp2Resource::Usb
        ) {
            return Err(LeaseError::Unsupported);
        }
        Ok(Self {
            backend,
            lease: Rp2Leases::acquire_guard(resource, owner)?,
        })
    }
}

impl<B: Rp2ByteIoBackend> HalByteIo for Rp2ByteIo<B> {
    type Error = Rp2ProviderError<B::Error>;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        self.lease.ensure_live().map_err(Rp2ProviderError::Lease)?;
        self.backend
            .read_available(bytes)
            .map_err(Rp2ProviderError::Backend)
    }

    fn write_all(&mut self, mut bytes: &[u8]) -> Result<(), Self::Error> {
        self.lease.ensure_live().map_err(Rp2ProviderError::Lease)?;
        let mut no_progress = 0u32;
        while !bytes.is_empty() {
            let count = self
                .backend
                .write_some(bytes)
                .map_err(Rp2ProviderError::Backend)?;
            if count == 0 {
                no_progress += 1;
                if no_progress == 100_000 {
                    return Err(Rp2ProviderError::Timeout);
                }
            } else {
                if count > bytes.len() {
                    return Err(Rp2ProviderError::InvalidConfig);
                }
                no_progress = 0;
                bytes = &bytes[count..];
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.lease.ensure_live().map_err(Rp2ProviderError::Lease)?;
        self.backend.flush().map_err(Rp2ProviderError::Backend)
    }
}

pub trait Rp2AlarmBackend {
    type Error;
    fn arm_after_us(&mut self, delay_us: u64) -> Result<(), Self::Error>;
    fn cancel(&mut self);
    fn elapsed(&mut self) -> bool;
}

pub struct Rp2Alarm<B: Rp2AlarmBackend, C> {
    backend: B,
    clock: PhantomData<C>,
    lease: Rp2LeaseGuard,
    deadline_us: Option<u64>,
}

impl<B: Rp2AlarmBackend, C> Rp2Alarm<B, C> {
    pub fn try_new(backend: B, _clock: C, owner: u8) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            clock: PhantomData,
            lease: Rp2Leases::acquire_guard(Rp2Resource::DeadlineAlarm, owner)?,
            deadline_us: None,
        })
    }
}

impl<B: Rp2AlarmBackend, C: HalClock> HalAlarm for Rp2Alarm<B, C> {
    type Error = Rp2ProviderError<B::Error>;

    fn arm_after_us(&mut self, delay_us: u64) -> Result<u64, Self::Error> {
        self.lease.ensure_live().map_err(Rp2ProviderError::Lease)?;
        if delay_us == 0 {
            return Err(Rp2ProviderError::InvalidConfig);
        }
        let deadline = C::now_us()
            .checked_add(delay_us)
            .ok_or(Rp2ProviderError::InvalidConfig)?;
        self.backend
            .arm_after_us(delay_us)
            .map_err(Rp2ProviderError::Backend)?;
        self.deadline_us = Some(deadline);
        Ok(deadline)
    }

    fn cancel(&mut self) {
        self.backend.cancel();
        self.deadline_us = None;
    }

    fn deadline_us(&self) -> Option<u64> {
        self.deadline_us
    }

    fn poll_due(&mut self, now_us: u64) -> bool {
        if self.deadline_us.is_some_and(|deadline| now_us >= deadline) || self.backend.elapsed() {
            self.cancel();
            true
        } else {
            false
        }
    }
}

impl<B: Rp2AlarmBackend, C> Drop for Rp2Alarm<B, C> {
    fn drop(&mut self) {
        self.backend.cancel();
        self.deadline_us = None;
    }
}

pub trait Rp2PowerBackend {
    type Error;
    fn cpu_sleep(&mut self) -> Result<(), Self::Error>;
}

pub struct Rp2Power<B> {
    backend: B,
    lease: Rp2LeaseGuard,
}

impl<B> Rp2Power<B> {
    pub fn try_new(backend: B, owner: u8) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            lease: Rp2Leases::acquire_guard(Rp2Resource::Power, owner)?,
        })
    }
}

impl<B: Rp2PowerBackend> HalPower for Rp2Power<B> {
    type Error = Rp2ProviderError<B::Error>;

    fn idle(&mut self, mode: IdleMode) -> Result<(), Self::Error> {
        self.lease.ensure_live().map_err(Rp2ProviderError::Lease)?;
        match mode {
            IdleMode::CpuSleep => self.backend.cpu_sleep().map_err(Rp2ProviderError::Backend),
        }
    }
}

pub trait Rp2ResetBackend {
    type Cause: Copy + Eq;
    fn reset_cause() -> Self::Cause;
    fn system_reset() -> !;
}

pub struct Rp2Reset<B>(PhantomData<B>);

impl<B: Rp2ResetBackend> HalReset for Rp2Reset<B> {
    type Cause = B::Cause;

    fn reset_cause() -> Self::Cause {
        B::reset_cause()
    }

    fn system_reset() -> ! {
        B::system_reset()
    }
}

/// Exclusive core-1 ownership with an explicit recovery generation.
pub struct Rp2MulticoreContract {
    lease: Rp2LeaseGuard,
    generation: u32,
}

impl Rp2MulticoreContract {
    pub fn try_acquire(owner: u8) -> Result<Self, LeaseError> {
        let lease = Rp2Leases::acquire_guard(Rp2Resource::Core1, owner)?;
        let generation = lease.generation;
        Ok(Self { lease, generation })
    }

    pub fn generation(&self) -> Result<u32, LeaseError> {
        self.lease.ensure_live()?;
        Ok(self.generation)
    }
}

/// Backend selected for the Pico W/Pico 2 W CYW43439 logical stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rp2Cyw43Backend {
    PioSpi,
    Vendor,
}

/// Exact-one wireless backend plus its backend-specific lease set.
pub struct Rp2Cyw43Contract {
    backend: Rp2Cyw43Backend,
    radio: Rp2LeaseGuard,
    pio: Option<Rp2LeaseGuard>,
}

impl Rp2Cyw43Contract {
    pub fn try_mount(backend: Rp2Cyw43Backend, owner: u8) -> Result<Self, LeaseError> {
        let radio = Rp2Leases::acquire_guard(Rp2Resource::Cyw43, owner)?;
        let pio = match backend {
            Rp2Cyw43Backend::PioSpi => match Rp2Leases::acquire_guard(Rp2Resource::Pio0, owner) {
                Ok(pio) => Some(pio),
                Err(error) => {
                    drop(radio);
                    return Err(error);
                }
            },
            Rp2Cyw43Backend::Vendor => None,
        };
        Ok(Self {
            backend,
            radio,
            pio,
        })
    }

    pub fn backend(&self) -> Result<Rp2Cyw43Backend, LeaseError> {
        self.radio.ensure_live()?;
        if let Some(pio) = &self.pio {
            pio.ensure_live()?;
        }
        Ok(self.backend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::convert::Infallible;
    use embedded_hal::{i2c, spi};

    #[derive(Default)]
    struct MockI2c {
        transactions: usize,
    }

    impl i2c::ErrorType for MockI2c {
        type Error = Infallible;
    }

    impl I2c for MockI2c {
        fn transaction(
            &mut self,
            _address: u8,
            operations: &mut [i2c::Operation<'_>],
        ) -> Result<(), Self::Error> {
            self.transactions += 1;
            for operation in operations {
                if let i2c::Operation::Read(bytes) = operation {
                    bytes.fill(0x52);
                }
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockSpi {
        transfers: usize,
    }

    impl spi::ErrorType for MockSpi {
        type Error = Infallible;
    }

    impl SpiBus<u8> for MockSpi {
        fn read(&mut self, words: &mut [u8]) -> Result<(), Self::Error> {
            words.fill(0xa5);
            Ok(())
        }

        fn write(&mut self, _words: &[u8]) -> Result<(), Self::Error> {
            Ok(())
        }

        fn transfer(&mut self, read: &mut [u8], write: &[u8]) -> Result<(), Self::Error> {
            self.transfers += 1;
            read.copy_from_slice(write);
            Ok(())
        }

        fn transfer_in_place(&mut self, words: &mut [u8]) -> Result<(), Self::Error> {
            words.reverse();
            Ok(())
        }

        fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    struct MockPwm(u16);

    impl Rp2PwmBackend for MockPwm {
        type Error = Infallible;

        fn max_duty(&self) -> u16 {
            1_000
        }

        fn set_duty(&mut self, duty: u16) -> Result<(), Self::Error> {
            self.0 = duty;
            Ok(())
        }
    }

    struct MockAdc;

    impl Rp2AdcBackend for MockAdc {
        type Error = Infallible;

        fn max_sample(&self) -> u16 {
            4_095
        }

        fn read(&mut self) -> Result<u16, Self::Error> {
            Ok(2_048)
        }
    }

    #[derive(Default)]
    struct MockByteIo {
        written: usize,
    }

    impl Rp2ByteIoBackend for MockByteIo {
        type Error = Infallible;

        fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
            if let Some(first) = bytes.first_mut() {
                *first = 0x42;
                Ok(1)
            } else {
                Ok(0)
            }
        }

        fn write_some(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
            let count = bytes.len().min(2);
            self.written += count;
            Ok(count)
        }

        fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockAlarm {
        armed: bool,
        cancelled: bool,
    }

    impl Rp2AlarmBackend for MockAlarm {
        type Error = Infallible;

        fn arm_after_us(&mut self, _delay_us: u64) -> Result<(), Self::Error> {
            self.armed = true;
            Ok(())
        }

        fn cancel(&mut self) {
            self.cancelled = true;
        }

        fn elapsed(&mut self) -> bool {
            false
        }
    }

    struct MockClock;

    impl HalClock for MockClock {
        fn now_us() -> u64 {
            100
        }
    }

    #[derive(Default)]
    struct MockPower {
        sleeps: usize,
    }

    impl Rp2PowerBackend for MockPower {
        type Error = Infallible;

        fn cpu_sleep(&mut self) -> Result<(), Self::Error> {
            self.sleeps += 1;
            Ok(())
        }
    }

    #[test]
    fn silicon_differences_are_not_flattened() {
        assert_eq!(RP2040_RUNTIME.cores, RP2350_RUNTIME.cores);
        const {
            assert!(RP2350_RUNTIME.pio_blocks > RP2040_RUNTIME.pio_blocks);
            assert!(RP2350_RUNTIME.dma_channels > RP2040_RUNTIME.dma_channels);
            assert!(RP2350_RUNTIME.pwm_slices > RP2040_RUNTIME.pwm_slices);
        };
    }

    #[test]
    fn dma_and_pio_admission_use_the_selected_silicon_limits() {
        assert!(Rp2DmaPlan::new(RP2040_RUNTIME, 11, 64, Some(0x3f)).is_ok());
        assert_eq!(
            Rp2DmaPlan::new(RP2040_RUNTIME, 12, 64, None),
            Err(Rp2ContractError::InvalidChannel)
        );
        assert!(Rp2DmaPlan::new(RP2350_RUNTIME, 15, 64, None).is_ok());
        assert_eq!(
            Rp2PioPlan::new(RP2040_RUNTIME, 2, 0, 4, 0, 1),
            Err(Rp2ContractError::InvalidPioBlock)
        );
        assert!(Rp2PioPlan::new(RP2350_RUNTIME, 2, 3, 32, 16, 8).is_ok());
        assert_eq!(
            Rp2PioPlan::new(RP2350_RUNTIME, 0, 4, 1, 0, 1),
            Err(Rp2ContractError::InvalidStateMachine)
        );
    }

    #[test]
    fn channel_leases_preserve_rp2040_and_rp2350_instance_limits() {
        let owner = 206;
        Rp2Leases::recover_owner(owner);
        let rp2040_dma11 = Rp2Leases::acquire_dma(RP2040_RUNTIME, 11, owner).unwrap();
        let rp2350_dma15 = Rp2Leases::acquire_dma(RP2350_RUNTIME, 15, owner).unwrap();
        assert_eq!(
            Rp2Leases::acquire_dma(RP2040_RUNTIME, 12, owner).err(),
            Some(LeaseError::Unsupported)
        );
        let rp2350_pio2 = Rp2Leases::acquire_pio(RP2350_RUNTIME, 2, owner).unwrap();
        assert_eq!(
            Rp2Leases::acquire_pio(RP2040_RUNTIME, 2, owner).err(),
            Some(LeaseError::Unsupported)
        );
        let rp2350_pwm8 = Rp2Leases::acquire_pwm(RP2350_RUNTIME, 8, owner).unwrap();
        assert_eq!(
            Rp2Leases::acquire_pwm(RP2040_RUNTIME, 8, owner).err(),
            Some(LeaseError::Unsupported)
        );
        assert!(rp2040_dma11.ensure_live().is_ok());
        assert!(rp2350_dma15.ensure_live().is_ok());
        assert!(rp2350_pio2.ensure_live().is_ok());
        assert!(rp2350_pwm8.ensure_live().is_ok());
    }

    #[test]
    fn lease_contention_recovery_and_stale_guard_are_bounded() {
        let owner = 201;
        Rp2Leases::recover_owner(owner);
        let guard = Rp2Leases::acquire_guard(Rp2Resource::Dma0, owner).unwrap();
        assert_eq!(
            Rp2Leases::acquire_guard(Rp2Resource::Dma0, owner + 1).err(),
            Some(LeaseError::AlreadyHeld)
        );
        assert_eq!(Rp2Leases::recover_owner(owner), 1);
        assert_eq!(guard.ensure_live(), Err(LeaseError::NotHeld));
        drop(guard);
        let next = Rp2Leases::acquire_guard(Rp2Resource::Dma0, owner + 1).unwrap();
        assert!(next.ensure_live().is_ok());
    }

    #[test]
    fn cyw43_mount_owns_only_the_selected_backends_resources() {
        let owner = 202;
        Rp2Leases::recover_owner(owner);
        {
            let stack = Rp2Cyw43Contract::try_mount(Rp2Cyw43Backend::PioSpi, owner).unwrap();
            assert_eq!(stack.backend(), Ok(Rp2Cyw43Backend::PioSpi));
            assert!(Rp2Leases::is_held(LeaseId::PRIMARY_RADIO));
            assert!(Rp2Leases::is_held(LeaseId::PRIMARY_PIO));
        }
        assert!(!Rp2Leases::is_held(LeaseId::PRIMARY_RADIO));
        assert!(!Rp2Leases::is_held(LeaseId::PRIMARY_PIO));

        let pio_owner = 205;
        Rp2Leases::recover_owner(pio_owner);
        let pio = Rp2Leases::acquire_guard(Rp2Resource::Pio0, pio_owner).unwrap();
        {
            let stack = Rp2Cyw43Contract::try_mount(Rp2Cyw43Backend::Vendor, owner).unwrap();
            assert_eq!(stack.backend(), Ok(Rp2Cyw43Backend::Vendor));
            assert!(Rp2Leases::is_held(LeaseId::PRIMARY_RADIO));
        }
        assert!(!Rp2Leases::is_held(LeaseId::PRIMARY_RADIO));
        assert!(pio.ensure_live().is_ok());
        drop(pio);
    }

    #[test]
    fn shared_providers_execute_data_and_lifecycle_contracts() {
        let owner = 203;
        Rp2Leases::recover_owner(owner);

        let mut i2c = Rp2I2c::try_new(MockI2c::default(), 0, owner).unwrap();
        let mut input = [0; 2];
        i2c.write_read(0x28, &[1], &mut input).unwrap();
        assert_eq!(input, [0x52; 2]);
        assert_eq!(i2c.write(0x80, &[1]), Err(Rp2ProviderError::InvalidConfig));
        assert_eq!(i2c.into_inner().transactions, 1);

        let mut spi = Rp2Spi::try_new(MockSpi::default(), 0, owner).unwrap();
        let mut rx = [0; 3];
        spi.transfer(&[1, 2, 3], &mut rx).unwrap();
        assert_eq!(rx, [1, 2, 3]);
        assert_eq!(
            spi.transfer(&[1], &mut [0; 2]),
            Err(Rp2ProviderError::LengthMismatch)
        );
        drop(spi);

        let mut pwm = Rp2Pwm::try_new(MockPwm(0), RP2040_RUNTIME, 0, owner).unwrap();
        assert_eq!(pwm.max_duty(), 1_000);
        pwm.set_duty(375).unwrap();
        drop(pwm);

        let mut adc = Rp2Adc::try_new(MockAdc, owner).unwrap();
        assert_eq!(adc.max_sample(), 4_095);
        assert_eq!(adc.read(), Ok(2_048));
        drop(adc);

        let mut stream =
            Rp2ByteIo::try_new(MockByteIo::default(), Rp2Resource::Uart0, owner).unwrap();
        stream.write_all(&[1, 2, 3, 4, 5]).unwrap();
        let mut byte = [0];
        assert_eq!(stream.read_available(&mut byte), Ok(1));
        assert_eq!(byte, [0x42]);
        stream.flush().unwrap();
        drop(stream);

        let mut alarm = Rp2Alarm::try_new(MockAlarm::default(), MockClock, owner).unwrap();
        assert_eq!(alarm.arm_after_us(25), Ok(125));
        assert!(!alarm.poll_due(124));
        assert!(alarm.poll_due(125));
        drop(alarm);

        let mut power = Rp2Power::try_new(MockPower::default(), owner).unwrap();
        power.idle(IdleMode::CpuSleep).unwrap();
        drop(power);

        assert_eq!(Rp2Leases::recover_owner(owner), 0);
    }

    #[test]
    fn provider_rejects_use_after_owner_recovery() {
        let owner = 204;
        Rp2Leases::recover_owner(owner);
        let mut pwm = Rp2Pwm::try_new(MockPwm(0), RP2040_RUNTIME, 0, owner).unwrap();
        assert_eq!(Rp2Leases::recover_owner(owner), 1);
        assert_eq!(
            pwm.set_duty(100),
            Err(Rp2ProviderError::Lease(LeaseError::NotHeld))
        );
        drop(pwm);
        let next = Rp2Pwm::try_new(MockPwm(0), RP2040_RUNTIME, 0, owner + 1).unwrap();
        drop(next);
    }
}
