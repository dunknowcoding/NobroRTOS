//! Portable provider adapters for the exact SAMD21G18A composition.

use core::convert::Infallible;
use portable_atomic::{AtomicBool, Ordering};

use embedded_hal::i2c::I2c;
use embedded_hal::spi::SpiBus;
use embedded_hal_02::Pwm;
use embedded_hal_nb::serial::{Read, Write};
use nobro_hal::{
    CapabilityProfileKind, HalAlarm, HalByteIo, HalClock, HalCompatibility, HalI2c, HalPwmChannel,
    HalSpi, HardwareCapability, HardwareCapabilityDeclaration, HardwareCapabilitySet,
    HardwareCapabilityWitness, TransferMode,
};
use usb_device::bus::UsbBus;
use usb_device::device::{
    StringDescriptors, UsbDevice, UsbDeviceBuilder, UsbDeviceState, UsbVidPid,
};
use usbd_serial::SerialPort;

use crate::lease::{
    Samd21LeaseGuard, Samd21Leases, RTC_LEASE, SERCOM0_UART_LEASE, SERCOM3_I2C_LEASE,
    SERCOM4_SPI_LEASE, TC4_DEADLINE_LEASE, TCC1_PWM_LEASE, USB_LEASE,
};

pub struct Samd21Providers;

impl HardwareCapabilityWitness<{ HardwareCapability::Timebase as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Deadline as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Event as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::DmaCompletion as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Gpio as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Irq as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Uart as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::ByteIo as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pwm as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::I2c as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Spi as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Usb as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Rtc as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Reset as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Power as u8 }> for Samd21Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Lease as u8 }> for Samd21Providers {}

impl HalCompatibility for Samd21Providers {
    const DECLARATION: HardwareCapabilityDeclaration = {
        let supported = HardwareCapabilitySet::EMPTY
            .witnessed::<Self, { HardwareCapability::Timebase as u8 }>(HardwareCapability::Timebase)
            .witnessed::<Self, { HardwareCapability::Deadline as u8 }>(HardwareCapability::Deadline)
            .witnessed::<Self, { HardwareCapability::Event as u8 }>(HardwareCapability::Event)
            .witnessed::<Self, { HardwareCapability::DmaCompletion as u8 }>(
                HardwareCapability::DmaCompletion,
            )
            .witnessed::<Self, { HardwareCapability::Gpio as u8 }>(HardwareCapability::Gpio)
            .witnessed::<Self, { HardwareCapability::Irq as u8 }>(HardwareCapability::Irq)
            .witnessed::<Self, { HardwareCapability::Uart as u8 }>(HardwareCapability::Uart)
            .witnessed::<Self, { HardwareCapability::ByteIo as u8 }>(HardwareCapability::ByteIo)
            .witnessed::<Self, { HardwareCapability::Pwm as u8 }>(HardwareCapability::Pwm)
            .witnessed::<Self, { HardwareCapability::I2c as u8 }>(HardwareCapability::I2c)
            .witnessed::<Self, { HardwareCapability::Spi as u8 }>(HardwareCapability::Spi)
            .witnessed::<Self, { HardwareCapability::Usb as u8 }>(HardwareCapability::Usb)
            .witnessed::<Self, { HardwareCapability::Rtc as u8 }>(HardwareCapability::Rtc)
            .witnessed::<Self, { HardwareCapability::Reset as u8 }>(HardwareCapability::Reset)
            .witnessed::<Self, { HardwareCapability::Power as u8 }>(HardwareCapability::Power)
            .witnessed::<Self, { HardwareCapability::Lease as u8 }>(HardwareCapability::Lease);
        let inapplicable = HardwareCapabilitySet::EMPTY
            .with(HardwareCapability::Servo)
            .with(HardwareCapability::Cache)
            .with(HardwareCapability::Multicore);
        HardwareCapabilityDeclaration::new(
            "samd21-native-partial-v3",
            CapabilityProfileKind::Constrained,
            supported,
            supported,
            inapplicable,
            HardwareCapabilitySet::ALL
                .without(supported)
                .without(inapplicable),
            supported,
        )
    };
}

const _: [(); 1] = [(); <Samd21Providers as HalCompatibility>::DECLARATION.is_valid() as usize];
const _: [(); 1] =
    [(); <Samd21Providers as HalCompatibility>::DECLARATION.is_exact_profile() as usize];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderError<E> {
    Lease(nobro_hal::LeaseError),
    Backend(E),
    InvalidConfig,
    LengthMismatch,
    Timeout,
    UsbWouldBlock,
}

pub struct Samd21I2c<B> {
    backend: B,
    _lease: Samd21LeaseGuard,
}

impl<B> Samd21I2c<B> {
    pub fn try_new(backend: B, owner: u8) -> Result<Self, nobro_hal::LeaseError> {
        Ok(Self {
            backend,
            _lease: Samd21Leases::acquire_guard(SERCOM3_I2C_LEASE, owner)?,
        })
    }

    pub fn into_inner(self) -> B {
        self.backend
    }
}

impl<B> HalI2c for Samd21I2c<B>
where
    B: I2c,
{
    type Error = ProviderError<B::Error>;
    const TRANSFER_MODE: TransferMode = TransferMode::Polling;

    fn write(&mut self, address: u8, bytes: &[u8]) -> Result<(), Self::Error> {
        if address >= 0x80 || bytes.is_empty() {
            return Err(ProviderError::InvalidConfig);
        }
        self._lease.ensure_live().map_err(ProviderError::Lease)?;
        self.backend
            .write(address, bytes)
            .map_err(ProviderError::Backend)
    }

    fn read(&mut self, address: u8, bytes: &mut [u8]) -> Result<(), Self::Error> {
        if address >= 0x80 || bytes.is_empty() {
            return Err(ProviderError::InvalidConfig);
        }
        self._lease.ensure_live().map_err(ProviderError::Lease)?;
        self.backend
            .read(address, bytes)
            .map_err(ProviderError::Backend)
    }

    fn write_read(
        &mut self,
        address: u8,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<(), Self::Error> {
        if address >= 0x80 || write.is_empty() || read.is_empty() {
            return Err(ProviderError::InvalidConfig);
        }
        self._lease.ensure_live().map_err(ProviderError::Lease)?;
        self.backend
            .write_read(address, write, read)
            .map_err(ProviderError::Backend)
    }
}

pub struct Samd21Spi<B> {
    backend: B,
    _lease: Samd21LeaseGuard,
}

impl<B> Samd21Spi<B> {
    pub fn try_new(backend: B, owner: u8) -> Result<Self, nobro_hal::LeaseError> {
        Ok(Self {
            backend,
            _lease: Samd21Leases::acquire_guard(SERCOM4_SPI_LEASE, owner)?,
        })
    }
}

impl<B> HalSpi for Samd21Spi<B>
where
    B: SpiBus<u8>,
{
    type Error = ProviderError<B::Error>;
    const TRANSFER_MODE: TransferMode = TransferMode::Polling;

    fn transfer(&mut self, write: &[u8], read: &mut [u8]) -> Result<(), Self::Error> {
        if write.len() != read.len() {
            return Err(ProviderError::LengthMismatch);
        }
        if write.is_empty() {
            return Ok(());
        }
        self._lease.ensure_live().map_err(ProviderError::Lease)?;
        self.backend
            .transfer(read, write)
            .map_err(ProviderError::Backend)
    }
}

pub struct Samd21Pwm<P, C> {
    backend: P,
    channel: C,
    _lease: Samd21LeaseGuard,
}

impl<P, C> Samd21Pwm<P, C>
where
    P: Pwm<Channel = C, Duty = u32>,
    C: Copy,
{
    pub fn try_new(mut backend: P, channel: C, owner: u8) -> Result<Self, nobro_hal::LeaseError> {
        let lease = Samd21Leases::acquire_guard(TCC1_PWM_LEASE, owner)?;
        backend.enable(channel);
        Ok(Self {
            backend,
            channel,
            _lease: lease,
        })
    }
}

impl<P, C> HalPwmChannel for Samd21Pwm<P, C>
where
    P: Pwm<Channel = C, Duty = u32>,
    C: Copy,
{
    type Error = ProviderError<Infallible>;

    fn max_duty(&self) -> u16 {
        u16::MAX
    }

    fn set_duty(&mut self, duty: u16) -> Result<(), Self::Error> {
        self._lease.ensure_live().map_err(ProviderError::Lease)?;
        let hardware_max = self.backend.get_max_duty();
        let scaled = u64::from(duty)
            .saturating_mul(u64::from(hardware_max))
            .checked_div(u64::from(u16::MAX))
            .unwrap_or(0) as u32;
        self.backend.set_duty(self.channel, scaled);
        Ok(())
    }
}

pub struct Samd21Uart<U> {
    uart: U,
    _lease: Samd21LeaseGuard,
}

impl<U> Samd21Uart<U> {
    pub fn try_new(uart: U, owner: u8) -> Result<Self, nobro_hal::LeaseError> {
        Ok(Self {
            uart,
            _lease: Samd21Leases::acquire_guard(SERCOM0_UART_LEASE, owner)?,
        })
    }
}

impl<U, E> HalByteIo for Samd21Uart<U>
where
    U: Read<u8, Error = E> + Write<u8, Error = E>,
{
    type Error = ProviderError<E>;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        self._lease.ensure_live().map_err(ProviderError::Lease)?;
        let mut count = 0;
        while count < bytes.len() {
            match self.uart.read() {
                Ok(byte) => {
                    bytes[count] = byte;
                    count += 1;
                }
                Err(nb::Error::WouldBlock) => break,
                Err(nb::Error::Other(error)) => return Err(ProviderError::Backend(error)),
            }
        }
        Ok(count)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self._lease.ensure_live().map_err(ProviderError::Lease)?;
        for &byte in bytes {
            let mut budget = 100_000u32;
            loop {
                match self.uart.write(byte) {
                    Ok(()) => break,
                    Err(nb::Error::WouldBlock) if budget != 0 => budget -= 1,
                    Err(nb::Error::WouldBlock) => return Err(ProviderError::Timeout),
                    Err(nb::Error::Other(error)) => return Err(ProviderError::Backend(error)),
                }
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self._lease.ensure_live().map_err(ProviderError::Lease)?;
        let mut budget = 100_000u32;
        loop {
            match self.uart.flush() {
                Ok(()) => return Ok(()),
                Err(nb::Error::WouldBlock) if budget != 0 => budget -= 1,
                Err(nb::Error::WouldBlock) => return Err(ProviderError::Timeout),
                Err(nb::Error::Other(error)) => return Err(ProviderError::Backend(error)),
            }
        }
    }
}

pub struct Samd21Usb<'a, B: UsbBus> {
    serial: SerialPort<'a, B>,
    device: UsbDevice<'a, B>,
    _lease: Samd21LeaseGuard,
}

impl<'a, B: UsbBus> Samd21Usb<'a, B> {
    pub fn try_mount(
        allocator: &'a usb_device::bus::UsbBusAllocator<B>,
        owner: u8,
    ) -> Result<Self, nobro_hal::LeaseError> {
        let lease = Samd21Leases::acquire_guard(USB_LEASE, owner)?;
        let serial = SerialPort::new(allocator);
        let strings = [StringDescriptors::default()
            .manufacturer("NobroRTOS")
            .product("NobroRTOS SAMD21")
            .serial_number("SAMD21-NATIVE")];
        let device = UsbDeviceBuilder::new(allocator, UsbVidPid(0x1209, 0x4e42))
            .strings(&strings)
            .expect("static USB descriptor strings are valid")
            .device_class(usbd_serial::USB_CLASS_CDC)
            .build();
        Ok(Self {
            serial,
            device,
            _lease: lease,
        })
    }

    pub fn poll(&mut self) -> bool {
        self.device.poll(&mut [&mut self.serial])
    }

    pub fn configured(&self) -> bool {
        self.device.state() == UsbDeviceState::Configured
    }
}

impl<B: UsbBus> HalByteIo for Samd21Usb<'_, B> {
    type Error = ProviderError<usb_device::UsbError>;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        self._lease.ensure_live().map_err(ProviderError::Lease)?;
        let _ = self.poll();
        match self.serial.read(bytes) {
            Ok(count) => Ok(count),
            Err(usb_device::UsbError::WouldBlock) => Ok(0),
            Err(error) => Err(ProviderError::Backend(error)),
        }
    }

    fn write_all(&mut self, mut bytes: &[u8]) -> Result<(), Self::Error> {
        self._lease.ensure_live().map_err(ProviderError::Lease)?;
        let mut budget = 100_000u32;
        while !bytes.is_empty() {
            let _ = self.poll();
            match self.serial.write(bytes) {
                Ok(0) | Err(usb_device::UsbError::WouldBlock) if budget != 0 => budget -= 1,
                Ok(0) | Err(usb_device::UsbError::WouldBlock) => {
                    return Err(ProviderError::Timeout);
                }
                Ok(written) => bytes = &bytes[written..],
                Err(error) => return Err(ProviderError::Backend(error)),
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self._lease.ensure_live().map_err(ProviderError::Lease)?;
        let _ = self.poll();
        Ok(())
    }
}

static RTC_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub struct Samd21Clock;

impl Samd21Clock {
    pub const TICK_HZ: u64 = 32_768;

    /// Claim the already clock-routed RTC.  The exact board composition routes
    /// GCLK1 (the external 32.768 kHz crystal) before calling this function.
    #[cfg(target_arch = "arm")]
    pub fn try_init(
        rtc: atsamd_hal::rtc::Rtc<atsamd_hal::rtc::Count32Mode>,
        owner: u8,
    ) -> Result<(), nobro_hal::LeaseError> {
        let guard = Samd21Leases::acquire_guard(RTC_LEASE, owner)?;
        if RTC_INITIALIZED.swap(true, Ordering::AcqRel) {
            drop(guard);
            return Err(nobro_hal::LeaseError::AlreadyHeld);
        }
        // The HAL has already routed GCLK1 and started MODE0. Keep the unique
        // peripheral owner alive for the process lifetime; timestamps use the
        // synchronized COUNT register below.
        let _rtc = rtc;
        core::mem::forget(guard);
        Ok(())
    }

    #[cfg(not(target_arch = "arm"))]
    pub fn try_init(owner: u8) -> Result<(), nobro_hal::LeaseError> {
        let guard = Samd21Leases::acquire_guard(RTC_LEASE, owner)?;
        if RTC_INITIALIZED.swap(true, Ordering::AcqRel) {
            drop(guard);
            return Err(nobro_hal::LeaseError::AlreadyHeld);
        }
        core::mem::forget(guard);
        Ok(())
    }

    #[cfg(target_arch = "arm")]
    fn ticks() -> u32 {
        unsafe { (0x4000_1410 as *const u32).read_volatile() }
    }

    #[cfg(not(target_arch = "arm"))]
    fn ticks() -> u32 {
        0
    }
}

impl HalClock for Samd21Clock {
    fn now_us() -> u64 {
        u64::from(Self::ticks()).saturating_mul(1_000_000) / Self::TICK_HZ
    }
}

pub trait AlarmTimer {
    type Error;

    fn start_us(&mut self, delay_us: u32) -> Result<(), Self::Error>;
    fn poll_elapsed(&mut self) -> Result<bool, Self::Error>;
    fn cancel(&mut self);
}

#[cfg(target_arch = "arm")]
impl AlarmTimer for atsamd_hal::timer::TimerCounter4 {
    type Error = Infallible;

    fn start_us(&mut self, delay_us: u32) -> Result<(), Self::Error> {
        use atsamd_hal::timer_traits::InterruptDrivenTimer;
        let duration = atsamd_hal::time::Nanoseconds::from_ticks(delay_us.saturating_mul(1_000));
        InterruptDrivenTimer::start(self, duration);
        Ok(())
    }

    fn poll_elapsed(&mut self) -> Result<bool, Self::Error> {
        use atsamd_hal::timer_traits::InterruptDrivenTimer;
        match InterruptDrivenTimer::wait(self) {
            Ok(()) => Ok(true),
            Err(nb::Error::WouldBlock) => Ok(false),
            Err(nb::Error::Other(error)) => match error {},
        }
    }

    fn cancel(&mut self) {
        // TC4 COUNT16.CTRLA.ENABLE is bit 1. The register is synchronized;
        // this bounded provider never hands the timer to another owner before
        // the lease guard is released.
        unsafe {
            const TC4_CTRLA: *mut u16 = 0x4200_3000 as *mut u16;
            const TC4_STATUS: *const u8 = 0x4200_300f as *const u8;
            TC4_CTRLA.write_volatile(TC4_CTRLA.read_volatile() & !0x0002);
            for _ in 0..65_536 {
                if TC4_STATUS.read_volatile() & 0x80 == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
        }
    }
}

pub struct Samd21Alarm<T: AlarmTimer> {
    timer: T,
    _lease: Samd21LeaseGuard,
    deadline_us: Option<u64>,
}

impl<T: AlarmTimer> Samd21Alarm<T> {
    pub fn try_new(timer: T, owner: u8) -> Result<Self, nobro_hal::LeaseError> {
        Ok(Self {
            timer,
            _lease: Samd21Leases::acquire_guard(TC4_DEADLINE_LEASE, owner)?,
            deadline_us: None,
        })
    }
}

impl<T: AlarmTimer> HalAlarm for Samd21Alarm<T> {
    type Error = ProviderError<T::Error>;

    fn arm_after_us(&mut self, delay_us: u64) -> Result<u64, Self::Error> {
        self._lease.ensure_live().map_err(ProviderError::Lease)?;
        if delay_us == 0 {
            return Err(ProviderError::InvalidConfig);
        }
        let deadline = Samd21Clock::now_us()
            .checked_add(delay_us)
            .ok_or(ProviderError::InvalidConfig)?;
        let delay_us = u32::try_from(delay_us).map_err(|_| ProviderError::InvalidConfig)?;
        self.timer
            .start_us(delay_us)
            .map_err(ProviderError::Backend)?;
        self.deadline_us = Some(deadline);
        Ok(deadline)
    }

    fn cancel(&mut self) {
        self.timer.cancel();
        self.deadline_us = None;
    }

    fn deadline_us(&self) -> Option<u64> {
        self.deadline_us
    }

    fn poll_due(&mut self, now_us: u64) -> bool {
        let deadline_reached = self.deadline_us.is_some_and(|deadline| now_us >= deadline);
        let timer_elapsed = !deadline_reached
            && self
                .timer
                .poll_elapsed()
                .ok()
                .is_some_and(core::convert::identity);
        if deadline_reached || timer_elapsed {
            self.cancel();
            true
        } else {
            false
        }
    }
}

impl<T: AlarmTimer> Drop for Samd21Alarm<T> {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeI2c;

    #[derive(Default)]
    struct FakeAlarm {
        elapsed: bool,
        cancelled: bool,
    }

    impl AlarmTimer for FakeAlarm {
        type Error = Infallible;

        fn start_us(&mut self, _: u32) -> Result<(), Self::Error> {
            Ok(())
        }

        fn poll_elapsed(&mut self) -> Result<bool, Self::Error> {
            Ok(self.elapsed)
        }

        fn cancel(&mut self) {
            self.cancelled = true;
        }
    }

    impl embedded_hal::i2c::ErrorType for FakeI2c {
        type Error = embedded_hal::i2c::ErrorKind;
    }

    impl I2c for FakeI2c {
        fn transaction(
            &mut self,
            _: u8,
            operations: &mut [embedded_hal::i2c::Operation<'_>],
        ) -> Result<(), Self::Error> {
            for operation in operations {
                if let embedded_hal::i2c::Operation::Read(bytes) = operation {
                    bytes.fill(0x32);
                }
            }
            Ok(())
        }
    }

    #[test]
    fn declaration_is_exact_and_does_not_claim_adc() {
        let declaration = <Samd21Providers as HalCompatibility>::DECLARATION;
        assert!(declaration.is_exact_profile());
        assert!(!declaration.supported.contains(HardwareCapability::Adc));
        assert!(declaration.supported.contains(HardwareCapability::Usb));
    }

    #[test]
    fn i2c_adapter_rejects_empty_or_invalid_transactions() {
        let _lock = crate::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut bus = Samd21I2c::try_new(FakeI2c, 4).unwrap();
        assert_eq!(bus.write(0x24, &[]), Err(ProviderError::InvalidConfig));
        let mut byte = [0];
        assert_eq!(bus.read(0x80, &mut byte), Err(ProviderError::InvalidConfig));
        bus.read(0x24, &mut byte).unwrap();
        assert_eq!(byte, [0x32]);
    }

    #[test]
    fn alarm_lifecycle_is_owned_and_bounded() {
        let _lock = crate::TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut alarm = Samd21Alarm::try_new(FakeAlarm::default(), 7).unwrap();
        assert_eq!(alarm.arm_after_us(0), Err(ProviderError::InvalidConfig));
        assert_eq!(alarm.arm_after_us(10), Ok(10));
        assert!(!alarm.poll_due(9));
        assert!(alarm.poll_due(10));
        assert_eq!(alarm.deadline_us(), None);
    }
}
