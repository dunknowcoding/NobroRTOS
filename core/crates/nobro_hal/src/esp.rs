//! Shared ESP32-C3/ESP32-S3 ownership and provider contracts.
//!
//! Register and pin construction remains in each exact port or Arduino
//! composition. This module owns the cross-family invariants: silicon-specific
//! limits, generation-safe leases, bounded provider wrappers, DMA cancellation,
//! event timestamps, CPU-idle/reset delegation, cache ownership, and optional
//! second-core ownership.

use core::{
    marker::PhantomData,
    sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering},
};

use embedded_hal::{i2c::I2c, spi::SpiBus};

use crate::{
    HalAdcChannel, HalAlarm, HalByteIo, HalClock, HalI2c, HalPower, HalPwmChannel, HalReset,
    HalSpi, IdleMode, LeaseClass, LeaseError, LeaseId, TransferMode,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EspSilicon {
    Esp32C3,
    Esp32S3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EspRuntimeContract {
    pub silicon: EspSilicon,
    pub cores: u8,
    pub gpio_count: u8,
    pub irq_routes: u8,
    pub uart_count: u8,
    pub i2c_count: u8,
    pub spi_count: u8,
    pub adc_units: u8,
    pub gdma_channels: u8,
    pub ledc_channels: u8,
    pub rmt_tx_channels: u8,
    pub rmt_rx_channels: u8,
    pub usb_serial_jtag: bool,
    pub usb_otg: bool,
    pub cache: bool,
}

pub const ESP32C3_RUNTIME: EspRuntimeContract = EspRuntimeContract {
    silicon: EspSilicon::Esp32C3,
    cores: 1,
    gpio_count: 22,
    irq_routes: 31,
    uart_count: 2,
    i2c_count: 1,
    spi_count: 1,
    adc_units: 2,
    gdma_channels: 3,
    ledc_channels: 6,
    rmt_tx_channels: 2,
    rmt_rx_channels: 2,
    usb_serial_jtag: true,
    usb_otg: false,
    cache: true,
};

pub const ESP32S3_RUNTIME: EspRuntimeContract = EspRuntimeContract {
    silicon: EspSilicon::Esp32S3,
    cores: 2,
    gpio_count: 45,
    irq_routes: 32,
    uart_count: 3,
    i2c_count: 2,
    spi_count: 2,
    adc_units: 2,
    gdma_channels: 5,
    ledc_channels: 8,
    rmt_tx_channels: 4,
    rmt_rx_channels: 4,
    usb_serial_jtag: true,
    usb_otg: true,
    cache: true,
};

impl EspRuntimeContract {
    /// Reports a routable GPIO number rather than treating a sparse pin space
    /// as a dense range. ESP32-S3 has no GPIO22..GPIO25.
    pub const fn supports_gpio(self, instance: u8) -> bool {
        match self.silicon {
            EspSilicon::Esp32C3 => instance < 22,
            EspSilicon::Esp32S3 => instance < 22 || (instance >= 26 && instance < 49),
        }
    }

    pub const fn rmt_channels(self) -> u8 {
        self.rmt_tx_channels + self.rmt_rx_channels
    }

    pub const fn supports(self, resource: LeaseId) -> bool {
        match resource.class {
            LeaseClass::Timer => resource.instance < 2,
            LeaseClass::Gpio => self.supports_gpio(resource.instance),
            LeaseClass::Irq => resource.instance < self.irq_routes,
            LeaseClass::I2c => resource.instance < self.i2c_count,
            LeaseClass::Spi => resource.instance < self.spi_count,
            LeaseClass::Radio => resource.instance == 0,
            LeaseClass::Pwm => resource.instance < self.ledc_channels,
            LeaseClass::EventRouter | LeaseClass::SoftwareEvent => resource.instance == 0,
            LeaseClass::Adc => resource.instance < self.adc_units,
            LeaseClass::Uart => resource.instance < self.uart_count,
            LeaseClass::Usb => resource.instance == 0 || (resource.instance == 1 && self.usb_otg),
            LeaseClass::Dma => resource.instance < self.gdma_channels,
            LeaseClass::Power | LeaseClass::Reset | LeaseClass::Cache => resource.instance == 0,
            LeaseClass::Pulse => resource.instance < self.rmt_channels(),
            LeaseClass::Core => resource.instance == 1 && self.cores > 1,
            LeaseClass::Pio => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EspContractError {
    InvalidChannel,
    EmptyTransfer,
    TransferTooLong,
    InvalidConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EspDmaPlan {
    pub silicon: EspSilicon,
    pub channel: u8,
    pub bytes: u16,
}

impl EspDmaPlan {
    pub const MAX_STAGED_BYTES: u16 = 4_096;

    pub const fn new(
        runtime: EspRuntimeContract,
        channel: u8,
        bytes: u16,
    ) -> Result<Self, EspContractError> {
        if channel >= runtime.gdma_channels {
            return Err(EspContractError::InvalidChannel);
        }
        if bytes == 0 {
            return Err(EspContractError::EmptyTransfer);
        }
        if bytes > Self::MAX_STAGED_BYTES {
            return Err(EspContractError::TransferTooLong);
        }
        Ok(Self {
            silicon: runtime.silicon,
            channel,
            bytes,
        })
    }
}

// Covers the largest supported C3/S3 composition without heap allocation.
const LEASE_SLOTS: usize = 128;

const fn lease_slot(id: LeaseId) -> Option<usize> {
    match id.class {
        LeaseClass::Timer if id.instance < 2 => Some(id.instance as usize),
        LeaseClass::Gpio if id.instance < 49 => Some(2 + id.instance as usize),
        LeaseClass::Irq if id.instance < 32 => Some(51 + id.instance as usize),
        LeaseClass::I2c if id.instance < 2 => Some(83 + id.instance as usize),
        LeaseClass::Spi if id.instance < 2 => Some(85 + id.instance as usize),
        LeaseClass::Radio if id.instance == 0 => Some(87),
        LeaseClass::Pwm if id.instance < 8 => Some(88 + id.instance as usize),
        LeaseClass::EventRouter if id.instance == 0 => Some(96),
        LeaseClass::SoftwareEvent if id.instance == 0 => Some(97),
        LeaseClass::Adc if id.instance < 2 => Some(98 + id.instance as usize),
        LeaseClass::Uart if id.instance < 3 => Some(100 + id.instance as usize),
        LeaseClass::Usb if id.instance < 2 => Some(103 + id.instance as usize),
        LeaseClass::Dma if id.instance < 5 => Some(105 + id.instance as usize),
        LeaseClass::Power if id.instance == 0 => Some(110),
        LeaseClass::Pulse if id.instance < 8 => Some(111 + id.instance as usize),
        LeaseClass::Core if id.instance == 1 => Some(119),
        LeaseClass::Reset if id.instance == 0 => Some(120),
        LeaseClass::Cache if id.instance == 0 => Some(121),
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

static SLOTS: [LeaseSlot; LEASE_SLOTS] = [const { LeaseSlot::new() }; LEASE_SLOTS];

pub struct EspLeases;

impl EspLeases {
    pub fn acquire(
        runtime: EspRuntimeContract,
        resource: LeaseId,
        owner: u8,
    ) -> Result<EspLeaseGuard, LeaseError> {
        if !runtime.supports(resource) {
            return Err(LeaseError::Unsupported);
        }
        let index = lease_slot(resource).ok_or(LeaseError::Unsupported)?;
        critical_section::with(|_| {
            let slot = &SLOTS[index];
            if slot.held.load(Ordering::Acquire) {
                return Err(LeaseError::AlreadyHeld);
            }
            let generation = slot.generation.load(Ordering::Acquire);
            if generation == u32::MAX {
                return Err(LeaseError::GenerationExhausted);
            }
            slot.owner.store(owner, Ordering::Release);
            slot.held.store(true, Ordering::Release);
            Ok(EspLeaseGuard {
                resource,
                owner,
                generation,
                live: true,
            })
        })
    }

    pub fn release(
        runtime: EspRuntimeContract,
        resource: LeaseId,
        owner: u8,
    ) -> Result<(), LeaseError> {
        if !runtime.supports(resource) {
            return Err(LeaseError::Unsupported);
        }
        Self::release_exact(resource, owner, None)
    }

    fn release_exact(
        resource: LeaseId,
        owner: u8,
        generation: Option<u32>,
    ) -> Result<(), LeaseError> {
        let index = lease_slot(resource).ok_or(LeaseError::Unsupported)?;
        critical_section::with(|_| {
            let slot = &SLOTS[index];
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
            slot.generation.store(
                slot.generation.load(Ordering::Acquire).saturating_add(1),
                Ordering::Release,
            );
            Ok(())
        })
    }

    pub fn is_held(runtime: EspRuntimeContract, resource: LeaseId) -> bool {
        runtime.supports(resource)
            && lease_slot(resource).is_some_and(|index| SLOTS[index].held.load(Ordering::Acquire))
    }

    pub fn owner(runtime: EspRuntimeContract, resource: LeaseId) -> Option<u8> {
        runtime.supports(resource).then_some(())?;
        lease_slot(resource).and_then(|index| {
            let slot = &SLOTS[index];
            slot.held
                .load(Ordering::Acquire)
                .then(|| slot.owner.load(Ordering::Acquire))
        })
    }

    pub fn recover_owner(runtime: EspRuntimeContract, owner: u8) -> usize {
        critical_section::with(|_| {
            let mut released = 0;
            for (index, slot) in SLOTS.iter().enumerate() {
                let supported =
                    lease_id_for_slot(index).is_some_and(|resource| runtime.supports(resource));
                if supported
                    && slot.held.load(Ordering::Acquire)
                    && slot.owner.load(Ordering::Acquire) == owner
                {
                    slot.held.store(false, Ordering::Release);
                    slot.owner.store(0, Ordering::Release);
                    slot.generation.store(
                        slot.generation.load(Ordering::Acquire).saturating_add(1),
                        Ordering::Release,
                    );
                    released += 1;
                }
            }
            released
        })
    }
}

fn lease_id_for_slot(index: usize) -> Option<LeaseId> {
    match index {
        0..=1 => Some(LeaseId::new(LeaseClass::Timer, index as u8)),
        2..=50 => Some(LeaseId::new(LeaseClass::Gpio, (index - 2) as u8)),
        51..=82 => Some(LeaseId::new(LeaseClass::Irq, (index - 51) as u8)),
        83..=84 => Some(LeaseId::new(LeaseClass::I2c, (index - 83) as u8)),
        85..=86 => Some(LeaseId::new(LeaseClass::Spi, (index - 85) as u8)),
        87 => Some(LeaseId::PRIMARY_RADIO),
        88..=95 => Some(LeaseId::new(LeaseClass::Pwm, (index - 88) as u8)),
        96 => Some(LeaseId::EVENT_ROUTER),
        97 => Some(LeaseId::SOFTWARE_EVENT),
        98..=99 => Some(LeaseId::new(LeaseClass::Adc, (index - 98) as u8)),
        100..=102 => Some(LeaseId::new(LeaseClass::Uart, (index - 100) as u8)),
        103..=104 => Some(LeaseId::new(LeaseClass::Usb, (index - 103) as u8)),
        105..=109 => Some(LeaseId::new(LeaseClass::Dma, (index - 105) as u8)),
        110 => Some(LeaseId::SYSTEM_POWER),
        111..=118 => Some(LeaseId::new(LeaseClass::Pulse, (index - 111) as u8)),
        119 => Some(LeaseId::SECONDARY_CORE),
        120 => Some(LeaseId::SYSTEM_RESET),
        121 => Some(LeaseId::SYSTEM_CACHE),
        _ => None,
    }
}

pub struct EspLeaseGuard {
    resource: LeaseId,
    owner: u8,
    generation: u32,
    live: bool,
}

impl EspLeaseGuard {
    pub const fn resource(&self) -> LeaseId {
        self.resource
    }

    pub const fn generation(&self) -> u32 {
        self.generation
    }

    pub fn ensure_live(&self) -> Result<(), LeaseError> {
        let index = lease_slot(self.resource).ok_or(LeaseError::NotHeld)?;
        let slot = &SLOTS[index];
        (self.live
            && slot.held.load(Ordering::Acquire)
            && slot.owner.load(Ordering::Acquire) == self.owner
            && slot.generation.load(Ordering::Acquire) == self.generation)
            .then_some(())
            .ok_or(LeaseError::NotHeld)
    }
}

impl Drop for EspLeaseGuard {
    fn drop(&mut self) {
        if self.live {
            let _ = EspLeases::release_exact(self.resource, self.owner, Some(self.generation));
            self.live = false;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EspProviderError<E> {
    Lease(LeaseError),
    Backend(E),
    InvalidConfig,
    LengthMismatch,
    Timeout,
}

pub trait EspGpioBackend {
    type Error;
    fn set_level(&mut self, high: bool) -> Result<(), Self::Error>;
    fn level(&mut self) -> Result<bool, Self::Error>;
}

pub struct EspGpio<B> {
    backend: B,
    lease: EspLeaseGuard,
}

impl<B> EspGpio<B> {
    pub fn try_new(
        backend: B,
        runtime: EspRuntimeContract,
        pin: u8,
        owner: u8,
    ) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            lease: EspLeases::acquire(runtime, LeaseId::new(LeaseClass::Gpio, pin), owner)?,
        })
    }
}

impl<B: EspGpioBackend> EspGpio<B> {
    pub fn set_level(&mut self, high: bool) -> Result<(), EspProviderError<B::Error>> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        self.backend
            .set_level(high)
            .map_err(EspProviderError::Backend)
    }

    pub fn level(&mut self) -> Result<bool, EspProviderError<B::Error>> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        self.backend.level().map_err(EspProviderError::Backend)
    }
}

pub trait EspIrqBackend {
    type Error;
    fn arm(&mut self) -> Result<(), Self::Error>;
    fn pending(&mut self) -> bool;
    fn clear(&mut self);
    fn disable(&mut self);
}

pub struct EspIrq<B: EspIrqBackend> {
    backend: B,
    lease: EspLeaseGuard,
    armed: bool,
}

impl<B: EspIrqBackend> EspIrq<B> {
    pub fn try_new(
        backend: B,
        runtime: EspRuntimeContract,
        route: u8,
        owner: u8,
    ) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            lease: EspLeases::acquire(runtime, LeaseId::new(LeaseClass::Irq, route), owner)?,
            armed: false,
        })
    }

    pub fn arm(&mut self) -> Result<(), EspProviderError<B::Error>> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        self.backend.arm().map_err(EspProviderError::Backend)?;
        self.armed = true;
        Ok(())
    }

    pub fn take_pending(&mut self) -> Result<bool, EspProviderError<B::Error>> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        if !self.armed {
            return Err(EspProviderError::InvalidConfig);
        }
        let pending = self.backend.pending();
        if pending {
            self.backend.clear();
        }
        Ok(pending)
    }
}

impl<B: EspIrqBackend> Drop for EspIrq<B> {
    fn drop(&mut self) {
        self.backend.disable();
    }
}

pub struct EspI2c<B> {
    backend: B,
    lease: EspLeaseGuard,
}

impl<B> EspI2c<B> {
    pub fn try_new(
        backend: B,
        runtime: EspRuntimeContract,
        instance: u8,
        owner: u8,
    ) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            lease: EspLeases::acquire(runtime, LeaseId::new(LeaseClass::I2c, instance), owner)?,
        })
    }
}

impl<B: I2c> HalI2c for EspI2c<B> {
    type Error = EspProviderError<B::Error>;
    const TRANSFER_MODE: TransferMode = TransferMode::Polling;

    fn write(&mut self, address: u8, bytes: &[u8]) -> Result<(), Self::Error> {
        if address >= 0x80 || bytes.is_empty() {
            return Err(EspProviderError::InvalidConfig);
        }
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        self.backend
            .write(address, bytes)
            .map_err(EspProviderError::Backend)
    }

    fn read(&mut self, address: u8, bytes: &mut [u8]) -> Result<(), Self::Error> {
        if address >= 0x80 || bytes.is_empty() {
            return Err(EspProviderError::InvalidConfig);
        }
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        self.backend
            .read(address, bytes)
            .map_err(EspProviderError::Backend)
    }

    fn write_read(
        &mut self,
        address: u8,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<(), Self::Error> {
        if address >= 0x80 || write.is_empty() || read.is_empty() {
            return Err(EspProviderError::InvalidConfig);
        }
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        self.backend
            .write_read(address, write, read)
            .map_err(EspProviderError::Backend)
    }
}

pub struct EspSpi<B> {
    backend: B,
    lease: EspLeaseGuard,
}

impl<B> EspSpi<B> {
    pub fn try_new(
        backend: B,
        runtime: EspRuntimeContract,
        instance: u8,
        owner: u8,
    ) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            lease: EspLeases::acquire(runtime, LeaseId::new(LeaseClass::Spi, instance), owner)?,
        })
    }
}

impl<B: SpiBus<u8>> HalSpi for EspSpi<B> {
    type Error = EspProviderError<B::Error>;
    const TRANSFER_MODE: TransferMode = TransferMode::Polling;

    fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> Result<(), Self::Error> {
        if write.len() != read.len() {
            return Err(EspProviderError::LengthMismatch);
        }
        if write.is_empty() {
            return Ok(());
        }
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        self.backend
            .transfer(read, write)
            .map_err(EspProviderError::Backend)
    }
}

pub trait EspPwmBackend {
    type Error;
    fn max_duty(&self) -> u16;
    fn set_duty(&mut self, duty: u16) -> Result<(), Self::Error>;
}

pub struct EspPwm<B> {
    backend: B,
    lease: EspLeaseGuard,
}

impl<B> EspPwm<B> {
    pub fn try_new(
        backend: B,
        runtime: EspRuntimeContract,
        channel: u8,
        owner: u8,
    ) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            lease: EspLeases::acquire(runtime, LeaseId::new(LeaseClass::Pwm, channel), owner)?,
        })
    }
}

impl<B: EspPwmBackend> HalPwmChannel for EspPwm<B> {
    type Error = EspProviderError<B::Error>;

    fn max_duty(&self) -> u16 {
        self.backend.max_duty()
    }

    fn set_duty(&mut self, duty: u16) -> Result<(), Self::Error> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        if duty > self.backend.max_duty() {
            return Err(EspProviderError::InvalidConfig);
        }
        self.backend
            .set_duty(duty)
            .map_err(EspProviderError::Backend)
    }
}

pub trait EspPulseBackend {
    type Error;
    fn configure(&mut self, tick_hz: u32) -> Result<(), Self::Error>;
    fn transmit(&mut self, levels_us: &[(u16, u16)]) -> Result<(), Self::Error>;
    fn cancel(&mut self);
}

pub struct EspPulse<B: EspPulseBackend> {
    backend: B,
    lease: EspLeaseGuard,
    configured: bool,
}

impl<B: EspPulseBackend> EspPulse<B> {
    pub fn try_new(
        backend: B,
        runtime: EspRuntimeContract,
        channel: u8,
        owner: u8,
    ) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            lease: EspLeases::acquire(runtime, LeaseId::new(LeaseClass::Pulse, channel), owner)?,
            configured: false,
        })
    }

    pub fn configure(&mut self, tick_hz: u32) -> Result<(), EspProviderError<B::Error>> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        if tick_hz == 0 {
            return Err(EspProviderError::InvalidConfig);
        }
        self.backend
            .configure(tick_hz)
            .map_err(EspProviderError::Backend)?;
        self.configured = true;
        Ok(())
    }

    pub fn transmit(&mut self, levels_us: &[(u16, u16)]) -> Result<(), EspProviderError<B::Error>> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        if !self.configured || levels_us.is_empty() {
            return Err(EspProviderError::InvalidConfig);
        }
        self.backend
            .transmit(levels_us)
            .map_err(EspProviderError::Backend)
    }

    pub fn cancel(&mut self) {
        self.backend.cancel();
    }
}

impl<B: EspPulseBackend> Drop for EspPulse<B> {
    fn drop(&mut self) {
        self.backend.cancel();
    }
}

pub trait EspAdcBackend {
    type Error;
    fn max_sample(&self) -> u16;
    fn read(&mut self) -> Result<u16, Self::Error>;
}

pub struct EspAdc<B> {
    backend: B,
    lease: EspLeaseGuard,
}

impl<B> EspAdc<B> {
    pub fn try_new(
        backend: B,
        runtime: EspRuntimeContract,
        unit: u8,
        owner: u8,
    ) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            lease: EspLeases::acquire(runtime, LeaseId::new(LeaseClass::Adc, unit), owner)?,
        })
    }
}

impl<B: EspAdcBackend> HalAdcChannel for EspAdc<B> {
    type Error = EspProviderError<B::Error>;

    fn max_sample(&self) -> u16 {
        self.backend.max_sample()
    }

    fn read(&mut self) -> Result<u16, Self::Error> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        self.backend.read().map_err(EspProviderError::Backend)
    }
}

pub trait EspByteIoBackend {
    type Error;
    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error>;
    fn write_some(&mut self, bytes: &[u8]) -> Result<usize, Self::Error>;
    fn flush(&mut self) -> Result<(), Self::Error>;
}

pub struct EspByteIo<B> {
    backend: B,
    lease: EspLeaseGuard,
}

impl<B> EspByteIo<B> {
    pub fn try_new(
        backend: B,
        runtime: EspRuntimeContract,
        resource: LeaseId,
        owner: u8,
    ) -> Result<Self, LeaseError> {
        if !matches!(resource.class, LeaseClass::Uart | LeaseClass::Usb) {
            return Err(LeaseError::Unsupported);
        }
        Ok(Self {
            backend,
            lease: EspLeases::acquire(runtime, resource, owner)?,
        })
    }
}

impl<B: EspByteIoBackend> HalByteIo for EspByteIo<B> {
    type Error = EspProviderError<B::Error>;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        self.backend
            .read_available(bytes)
            .map_err(EspProviderError::Backend)
    }

    fn write_all(&mut self, mut bytes: &[u8]) -> Result<(), Self::Error> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        let mut no_progress = 0u32;
        while !bytes.is_empty() {
            let count = self
                .backend
                .write_some(bytes)
                .map_err(EspProviderError::Backend)?;
            if count > bytes.len() {
                return Err(EspProviderError::InvalidConfig);
            }
            if count == 0 {
                no_progress = no_progress.saturating_add(1);
                if no_progress == 100_000 {
                    return Err(EspProviderError::Timeout);
                }
            } else {
                no_progress = 0;
                bytes = &bytes[count..];
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        self.backend.flush().map_err(EspProviderError::Backend)
    }
}

pub trait EspAlarmBackend {
    type Error;
    fn arm_after_us(&mut self, delay_us: u64) -> Result<(), Self::Error>;
    fn cancel(&mut self);
    fn elapsed(&mut self) -> bool;
}

pub struct EspAlarm<B: EspAlarmBackend, C> {
    backend: B,
    _clock: PhantomData<C>,
    lease: EspLeaseGuard,
    deadline_us: Option<u64>,
}

impl<B: EspAlarmBackend, C> EspAlarm<B, C> {
    pub fn try_new(backend: B, runtime: EspRuntimeContract, owner: u8) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            _clock: PhantomData,
            lease: EspLeases::acquire(runtime, LeaseId::DEADLINE_TIMER, owner)?,
            deadline_us: None,
        })
    }
}

impl<B: EspAlarmBackend, C: HalClock> HalAlarm for EspAlarm<B, C> {
    type Error = EspProviderError<B::Error>;

    fn arm_after_us(&mut self, delay_us: u64) -> Result<u64, Self::Error> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        if delay_us == 0 {
            return Err(EspProviderError::InvalidConfig);
        }
        let deadline = C::now_us()
            .checked_add(delay_us)
            .ok_or(EspProviderError::InvalidConfig)?;
        self.backend
            .arm_after_us(delay_us)
            .map_err(EspProviderError::Backend)?;
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

impl<B: EspAlarmBackend, C> Drop for EspAlarm<B, C> {
    fn drop(&mut self) {
        self.backend.cancel();
    }
}

pub trait EspDmaBackend {
    type Error;
    fn start(&mut self, bytes: u16) -> Result<(), Self::Error>;
    fn complete(&mut self) -> bool;
    fn cancel(&mut self);
}

pub struct EspDmaCompletion<B: EspDmaBackend> {
    backend: B,
    lease: EspLeaseGuard,
    plan: EspDmaPlan,
    running: bool,
    completions: u32,
    cancellations: u32,
}

impl<B: EspDmaBackend> EspDmaCompletion<B> {
    pub fn try_new(
        backend: B,
        runtime: EspRuntimeContract,
        plan: EspDmaPlan,
        owner: u8,
    ) -> Result<Self, LeaseError> {
        if plan.silicon != runtime.silicon {
            return Err(LeaseError::Unsupported);
        }
        Ok(Self {
            backend,
            lease: EspLeases::acquire(runtime, LeaseId::new(LeaseClass::Dma, plan.channel), owner)?,
            plan,
            running: false,
            completions: 0,
            cancellations: 0,
        })
    }

    pub fn start(&mut self) -> Result<(), EspProviderError<B::Error>> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        if self.running {
            return Err(EspProviderError::InvalidConfig);
        }
        self.backend
            .start(self.plan.bytes)
            .map_err(EspProviderError::Backend)?;
        self.running = true;
        Ok(())
    }

    pub fn poll_complete(&mut self) -> Result<bool, EspProviderError<B::Error>> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        if !self.running {
            return Ok(false);
        }
        if self.backend.complete() {
            self.running = false;
            self.completions = self.completions.saturating_add(1);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn cancel(&mut self) {
        if self.running {
            self.backend.cancel();
            self.running = false;
            self.cancellations = self.cancellations.saturating_add(1);
        }
    }

    pub const fn diagnostics(&self) -> (u32, u32) {
        (self.completions, self.cancellations)
    }
}

impl<B: EspDmaBackend> Drop for EspDmaCompletion<B> {
    fn drop(&mut self) {
        if self.running {
            self.backend.cancel();
            self.running = false;
        }
    }
}

pub trait EspEventBackend {
    type Error;
    fn arm(&mut self) -> Result<(), Self::Error>;
    fn trigger(&mut self) -> Result<(), Self::Error>;
    fn cancel(&mut self);
}

pub struct EspEventCapture<B: EspEventBackend, C> {
    backend: B,
    _clock: PhantomData<C>,
    lease: EspLeaseGuard,
    armed_at_us: Option<u64>,
    last_latency_us: u32,
}

impl<B: EspEventBackend, C: HalClock> EspEventCapture<B, C> {
    pub fn try_new(backend: B, runtime: EspRuntimeContract, owner: u8) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            _clock: PhantomData,
            lease: EspLeases::acquire(runtime, LeaseId::EVENT_ROUTER, owner)?,
            armed_at_us: None,
            last_latency_us: 0,
        })
    }

    pub fn arm(&mut self) -> Result<(), EspProviderError<B::Error>> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        self.backend.arm().map_err(EspProviderError::Backend)?;
        self.armed_at_us = Some(C::now_us());
        Ok(())
    }

    pub fn trigger(&mut self) -> Result<u32, EspProviderError<B::Error>> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        let started = self.armed_at_us.ok_or(EspProviderError::InvalidConfig)?;
        self.backend.trigger().map_err(EspProviderError::Backend)?;
        let elapsed = C::now_us().saturating_sub(started);
        self.last_latency_us = elapsed.min(u64::from(u32::MAX)) as u32;
        self.armed_at_us = None;
        Ok(self.last_latency_us)
    }

    pub const fn last_latency_us(&self) -> u32 {
        self.last_latency_us
    }
}

impl<B: EspEventBackend, C> Drop for EspEventCapture<B, C> {
    fn drop(&mut self) {
        self.backend.cancel();
    }
}

pub trait EspPowerBackend {
    type Error;
    fn cpu_sleep(&mut self) -> Result<(), Self::Error>;
}

pub struct EspPower<B> {
    backend: B,
    lease: EspLeaseGuard,
}

impl<B> EspPower<B> {
    pub fn try_new(backend: B, runtime: EspRuntimeContract, owner: u8) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            lease: EspLeases::acquire(runtime, LeaseId::SYSTEM_POWER, owner)?,
        })
    }
}

impl<B: EspPowerBackend> HalPower for EspPower<B> {
    type Error = EspProviderError<B::Error>;

    fn idle(&mut self, mode: IdleMode) -> Result<(), Self::Error> {
        self.lease.ensure_live().map_err(EspProviderError::Lease)?;
        match mode {
            IdleMode::CpuSleep => self.backend.cpu_sleep().map_err(EspProviderError::Backend),
        }
    }
}

pub trait EspResetBackend {
    type Cause: Copy + Eq;
    fn reset_cause() -> Self::Cause;
    fn system_reset() -> !;
}

pub struct EspReset<B>(PhantomData<B>);

impl<B: EspResetBackend> HalReset for EspReset<B> {
    type Cause = B::Cause;

    fn reset_cause() -> Self::Cause {
        B::reset_cause()
    }

    fn system_reset() -> ! {
        B::system_reset()
    }
}

pub struct EspCacheContract {
    lease: EspLeaseGuard,
}

impl EspCacheContract {
    pub fn try_acquire(runtime: EspRuntimeContract, owner: u8) -> Result<Self, LeaseError> {
        Ok(Self {
            lease: EspLeases::acquire(runtime, LeaseId::SYSTEM_CACHE, owner)?,
        })
    }

    pub fn ensure_live(&self) -> Result<(), LeaseError> {
        self.lease.ensure_live()
    }
}

pub struct EspMulticoreContract {
    lease: EspLeaseGuard,
}

impl EspMulticoreContract {
    pub fn try_acquire(runtime: EspRuntimeContract, owner: u8) -> Result<Self, LeaseError> {
        Ok(Self {
            lease: EspLeases::acquire(runtime, LeaseId::SECONDARY_CORE, owner)?,
        })
    }

    pub fn generation(&self) -> Result<u32, LeaseError> {
        self.lease.ensure_live()?;
        Ok(self.lease.generation())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::convert::Infallible;
    use embedded_hal::{i2c, spi};

    struct Clock;
    impl HalClock for Clock {
        fn now_us() -> u64 {
            100
        }
    }

    #[derive(Default)]
    struct Dma {
        done: bool,
        cancelled: bool,
    }

    impl EspDmaBackend for Dma {
        type Error = Infallible;

        fn start(&mut self, _: u16) -> Result<(), Self::Error> {
            self.done = true;
            Ok(())
        }

        fn complete(&mut self) -> bool {
            self.done
        }

        fn cancel(&mut self) {
            self.cancelled = true;
        }
    }

    struct Event;
    impl EspEventBackend for Event {
        type Error = Infallible;

        fn arm(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn trigger(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn cancel(&mut self) {}
    }

    #[derive(Default)]
    struct Gpio(bool);

    impl EspGpioBackend for Gpio {
        type Error = Infallible;

        fn set_level(&mut self, high: bool) -> Result<(), Self::Error> {
            self.0 = high;
            Ok(())
        }

        fn level(&mut self) -> Result<bool, Self::Error> {
            Ok(self.0)
        }
    }

    #[derive(Default)]
    struct Irq {
        armed: bool,
        pending: bool,
    }

    impl EspIrqBackend for Irq {
        type Error = Infallible;

        fn arm(&mut self) -> Result<(), Self::Error> {
            self.armed = true;
            self.pending = true;
            Ok(())
        }

        fn pending(&mut self) -> bool {
            self.armed && self.pending
        }

        fn clear(&mut self) {
            self.pending = false;
        }

        fn disable(&mut self) {
            self.armed = false;
        }
    }

    #[derive(Default)]
    struct I2cBackend;

    impl i2c::ErrorType for I2cBackend {
        type Error = Infallible;
    }

    impl I2c for I2cBackend {
        fn transaction(
            &mut self,
            address: u8,
            operations: &mut [i2c::Operation<'_>],
        ) -> Result<(), Self::Error> {
            for operation in operations {
                if let i2c::Operation::Read(bytes) = operation {
                    bytes.fill(address);
                }
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct SpiBackend;

    impl spi::ErrorType for SpiBackend {
        type Error = Infallible;
    }

    impl SpiBus<u8> for SpiBackend {
        fn read(&mut self, words: &mut [u8]) -> Result<(), Self::Error> {
            words.fill(0x5a);
            Ok(())
        }

        fn write(&mut self, _: &[u8]) -> Result<(), Self::Error> {
            Ok(())
        }

        fn transfer(&mut self, read: &mut [u8], write: &[u8]) -> Result<(), Self::Error> {
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

    #[derive(Default)]
    struct Pwm(u16);

    impl EspPwmBackend for Pwm {
        type Error = Infallible;

        fn max_duty(&self) -> u16 {
            1_000
        }

        fn set_duty(&mut self, duty: u16) -> Result<(), Self::Error> {
            self.0 = duty;
            Ok(())
        }
    }

    struct Pulse;

    impl EspPulseBackend for Pulse {
        type Error = Infallible;

        fn configure(&mut self, _: u32) -> Result<(), Self::Error> {
            Ok(())
        }

        fn transmit(&mut self, _: &[(u16, u16)]) -> Result<(), Self::Error> {
            Ok(())
        }

        fn cancel(&mut self) {}
    }

    struct Adc;

    impl EspAdcBackend for Adc {
        type Error = Infallible;

        fn max_sample(&self) -> u16 {
            4_095
        }

        fn read(&mut self) -> Result<u16, Self::Error> {
            Ok(2_048)
        }
    }

    #[derive(Default)]
    struct ByteIo {
        written: usize,
    }

    impl EspByteIoBackend for ByteIo {
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
    struct AlarmBackend {
        elapsed: bool,
    }

    impl EspAlarmBackend for AlarmBackend {
        type Error = Infallible;

        fn arm_after_us(&mut self, _: u64) -> Result<(), Self::Error> {
            self.elapsed = true;
            Ok(())
        }

        fn cancel(&mut self) {
            self.elapsed = false;
        }

        fn elapsed(&mut self) -> bool {
            self.elapsed
        }
    }

    struct Power;

    impl EspPowerBackend for Power {
        type Error = Infallible;

        fn cpu_sleep(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    struct Reset;

    impl EspResetBackend for Reset {
        type Cause = bool;

        fn reset_cause() -> Self::Cause {
            false
        }

        fn system_reset() -> ! {
            loop {
                core::hint::spin_loop();
            }
        }
    }

    #[test]
    fn silicon_limits_are_distinct_and_exact() {
        assert_eq!(ESP32C3_RUNTIME.cores, 1);
        assert_eq!(ESP32S3_RUNTIME.cores, 2);
        assert_eq!(ESP32C3_RUNTIME.gdma_channels, 3);
        assert_eq!(ESP32S3_RUNTIME.gdma_channels, 5);
        assert_eq!(ESP32C3_RUNTIME.ledc_channels, 6);
        assert_eq!(ESP32S3_RUNTIME.ledc_channels, 8);
        assert_eq!(ESP32C3_RUNTIME.rmt_tx_channels, 2);
        assert_eq!(ESP32C3_RUNTIME.rmt_rx_channels, 2);
        assert_eq!(ESP32S3_RUNTIME.rmt_tx_channels, 4);
        assert_eq!(ESP32S3_RUNTIME.rmt_rx_channels, 4);
        assert!(!ESP32C3_RUNTIME.usb_otg);
        assert!(ESP32S3_RUNTIME.usb_otg);
        assert_eq!(ESP32S3_RUNTIME.gpio_count, 45);
        assert!(!ESP32S3_RUNTIME.supports_gpio(22));
        assert!(ESP32S3_RUNTIME.supports_gpio(48));
    }

    #[test]
    fn runtime_rejects_other_silicons_tail_resources() {
        assert!(!ESP32C3_RUNTIME.supports(LeaseId::new(LeaseClass::Dma, 3)));
        assert!(ESP32S3_RUNTIME.supports(LeaseId::new(LeaseClass::Dma, 3)));
        assert!(!ESP32C3_RUNTIME.supports(LeaseId::SECONDARY_CORE));
        assert!(ESP32S3_RUNTIME.supports(LeaseId::SECONDARY_CORE));
        assert!(!ESP32C3_RUNTIME.supports(LeaseId::new(LeaseClass::Usb, 1)));
        assert!(ESP32S3_RUNTIME.supports(LeaseId::new(LeaseClass::Usb, 1)));
    }

    #[test]
    fn lease_recovery_invalidates_stale_generation() {
        let owner = 217;
        EspLeases::recover_owner(ESP32S3_RUNTIME, owner);
        let guard = EspLeases::acquire(ESP32S3_RUNTIME, LeaseId::PRIMARY_DMA, owner).unwrap();
        assert_eq!(
            EspLeases::acquire(ESP32S3_RUNTIME, LeaseId::PRIMARY_DMA, owner + 1).err(),
            Some(LeaseError::AlreadyHeld)
        );
        assert_eq!(EspLeases::recover_owner(ESP32S3_RUNTIME, owner), 1);
        assert_eq!(guard.ensure_live(), Err(LeaseError::NotHeld));
    }

    #[test]
    fn dma_completion_and_cancellation_are_bounded() {
        let owner = 218;
        EspLeases::recover_owner(ESP32C3_RUNTIME, owner);
        let plan = EspDmaPlan::new(ESP32C3_RUNTIME, 0, 64).unwrap();
        let mut dma =
            EspDmaCompletion::try_new(Dma::default(), ESP32C3_RUNTIME, plan, owner).unwrap();
        dma.start().unwrap();
        assert_eq!(dma.poll_complete(), Ok(true));
        dma.start().unwrap();
        dma.cancel();
        assert_eq!(dma.diagnostics(), (1, 1));
    }

    #[test]
    fn event_and_multicore_ownership_are_independent() {
        let owner = 219;
        EspLeases::recover_owner(ESP32S3_RUNTIME, owner);
        let mut event =
            EspEventCapture::<_, Clock>::try_new(Event, ESP32S3_RUNTIME, owner).unwrap();
        event.arm().unwrap();
        assert_eq!(event.trigger(), Ok(0));
        let core = EspMulticoreContract::try_acquire(ESP32S3_RUNTIME, owner).unwrap();
        assert!(core.generation().is_ok());
        assert_eq!(
            EspMulticoreContract::try_acquire(ESP32C3_RUNTIME, owner).err(),
            Some(LeaseError::Unsupported)
        );
    }

    #[test]
    fn shared_data_providers_enforce_bounds_and_live_leases() {
        let owner = 220;
        EspLeases::recover_owner(ESP32S3_RUNTIME, owner);

        let mut gpio = EspGpio::try_new(Gpio::default(), ESP32S3_RUNTIME, 48, owner).unwrap();
        gpio.set_level(true).unwrap();
        assert_eq!(gpio.level(), Ok(true));
        assert_eq!(
            EspGpio::try_new(Gpio::default(), ESP32S3_RUNTIME, 22, owner).err(),
            Some(LeaseError::Unsupported)
        );

        let mut irq = EspIrq::try_new(Irq::default(), ESP32S3_RUNTIME, 0, owner).unwrap();
        irq.arm().unwrap();
        assert_eq!(irq.take_pending(), Ok(true));
        assert_eq!(irq.take_pending(), Ok(false));

        let mut i2c = EspI2c::try_new(I2cBackend, ESP32S3_RUNTIME, 0, owner).unwrap();
        let mut i2c_read = [0; 2];
        i2c.write_read(0x42, &[1], &mut i2c_read).unwrap();
        assert_eq!(i2c_read, [0x42; 2]);

        let mut spi = EspSpi::try_new(SpiBackend, ESP32S3_RUNTIME, 0, owner).unwrap();
        let mut spi_read = [0; 3];
        spi.transfer(&[1, 2, 3], &mut spi_read).unwrap();
        assert_eq!(spi_read, [1, 2, 3]);

        let mut pwm = EspPwm::try_new(Pwm::default(), ESP32S3_RUNTIME, 0, owner).unwrap();
        pwm.set_duty(500).unwrap();
        assert_eq!(pwm.set_duty(1_001), Err(EspProviderError::InvalidConfig));

        let mut pulse = EspPulse::try_new(Pulse, ESP32S3_RUNTIME, 0, owner).unwrap();
        assert_eq!(
            pulse.transmit(&[(10, 10)]),
            Err(EspProviderError::InvalidConfig)
        );
        pulse.configure(1_000_000).unwrap();
        pulse.transmit(&[(10, 10)]).unwrap();

        let mut adc = EspAdc::try_new(Adc, ESP32S3_RUNTIME, 0, owner).unwrap();
        assert_eq!(adc.max_sample(), 4_095);
        assert_eq!(adc.read(), Ok(2_048));

        let mut bytes = EspByteIo::try_new(
            ByteIo::default(),
            ESP32S3_RUNTIME,
            LeaseId::PRIMARY_UART,
            owner,
        )
        .unwrap();
        let mut input = [0; 1];
        assert_eq!(bytes.read_available(&mut input), Ok(1));
        assert_eq!(input, [0x42]);
        bytes.write_all(&[1, 2, 3]).unwrap();
        bytes.flush().unwrap();
    }

    #[test]
    fn lifecycle_power_reset_and_cache_contracts_are_owned() {
        let owner = 221;
        EspLeases::recover_owner(ESP32S3_RUNTIME, owner);
        let mut alarm =
            EspAlarm::<_, Clock>::try_new(AlarmBackend::default(), ESP32S3_RUNTIME, owner).unwrap();
        assert_eq!(alarm.arm_after_us(10), Ok(110));
        assert!(alarm.poll_due(100));

        let mut power = EspPower::try_new(Power, ESP32S3_RUNTIME, owner).unwrap();
        assert_eq!(power.idle(IdleMode::CpuSleep), Ok(()));
        assert_eq!(EspReset::<Reset>::reset_cause(), false);

        let cache = EspCacheContract::try_acquire(ESP32S3_RUNTIME, owner).unwrap();
        assert_eq!(cache.ensure_live(), Ok(()));
    }
}
