//! Complete-board lifecycle providers for the UNO R4 WiFi RA4M1.
//!
//! GPIO is implemented directly against the RA4M1 PFS registers. IRQ, pulse,
//! watchdog, calendar RTC, and data-flash providers deliberately wrap a mounted
//! backend: the native Rust image and the Arduino/FSP image can therefore share
//! the same exclusive, generation-checked NobroRTOS contract without pretending
//! that two owners may program the same peripheral.

use core::sync::atomic::{AtomicBool, Ordering};

use nobro_hal::{LeaseClass, LeaseError, LeaseId};

use crate::lease::{header_irq_channel, Ra4m1LeaseGuard, Ra4m1Leases};

pub const RA4M1_DATA_FLASH_START: u32 = 0x4010_0000;
pub const RA4M1_DATA_FLASH_LEN: u32 = 8 * 1024;
// RA4M1 MF3 data flash erases in 1 KiB blocks and programs in one-byte units.
// Do not copy the 64-byte geometry used by some other RA families.
pub const RA4M1_DATA_FLASH_ERASE_SIZE: u32 = 1024;
pub const RA4M1_DATA_FLASH_WRITE_SIZE: u32 = 1;

/// Exact Arduino UNO R4 WiFi header mapping D0..D13, A0..A5.
const HEADER_PINS: [(u8, u8); 20] = [
    (3, 1),
    (3, 2),
    (1, 4),
    (1, 5),
    (1, 6),
    (1, 7),
    (1, 11),
    (1, 12),
    (3, 4),
    (3, 3),
    (1, 3),
    (4, 11),
    (4, 10),
    (1, 2),
    (0, 14),
    (0, 0),
    (0, 1),
    (0, 2),
    (1, 1),
    (1, 0),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeepError<E> {
    Lease(LeaseError),
    Backend(E),
    InvalidConfig,
    UnsupportedOnHost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpioMode {
    Input,
    InputPullUp,
    OutputLow,
    OutputHigh,
}

pub struct Ra4m1Gpio {
    lease: Ra4m1LeaseGuard,
    #[cfg_attr(not(target_arch = "arm"), allow(dead_code))]
    pin: u8,
}

impl Ra4m1Gpio {
    pub fn try_new(
        pin: u8,
        mode: GpioMode,
        owner: u8,
    ) -> Result<Self, DeepError<core::convert::Infallible>> {
        if usize::from(pin) >= HEADER_PINS.len() {
            return Err(DeepError::InvalidConfig);
        }
        let lease = Ra4m1Leases::acquire_guard(LeaseId::new(LeaseClass::Gpio, pin), owner)
            .map_err(DeepError::Lease)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            write_pfs(pin, mode_value(mode));
        }
        #[cfg(not(target_arch = "arm"))]
        let _ = mode;
        Ok(Self { lease, pin })
    }

    pub fn write(&mut self, high: bool) -> Result<(), DeepError<core::convert::Infallible>> {
        self.lease.ensure_live().map_err(DeepError::Lease)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            let address = pfs_address(self.pin);
            let current = (address as *const u32).read_volatile();
            (address as *mut u32).write_volatile((current & !1) | u32::from(high));
        }
        #[cfg(not(target_arch = "arm"))]
        let _ = high;
        Ok(())
    }

    pub fn read(&self) -> Result<bool, DeepError<core::convert::Infallible>> {
        self.lease.ensure_live().map_err(DeepError::Lease)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            Ok((pfs_address(self.pin) as *const u32).read_volatile() & (1 << 1) != 0)
        }
        #[cfg(not(target_arch = "arm"))]
        {
            Err(DeepError::UnsupportedOnHost)
        }
    }
}

#[cfg(target_arch = "arm")]
const fn pfs_address(pin: u8) -> usize {
    let (port, bit) = HEADER_PINS[pin as usize];
    0x4004_0800 + port as usize * 0x40 + bit as usize * 4
}

#[cfg(target_arch = "arm")]
const fn mode_value(mode: GpioMode) -> u32 {
    match mode {
        GpioMode::Input => 0,
        GpioMode::InputPullUp => 1 << 4,
        GpioMode::OutputLow => 1 << 2,
        GpioMode::OutputHigh => (1 << 2) | 1,
    }
}

#[cfg(target_arch = "arm")]
unsafe fn write_pfs(pin: u8, value: u32) {
    let pwpr = 0x4004_0d03 as *mut u8;
    pwpr.write_volatile(0);
    pwpr.write_volatile(0x40);
    (pfs_address(pin) as *mut u32).write_volatile(value);
    pwpr.write_volatile(0);
    pwpr.write_volatile(0x80);
}

pub trait IrqBackend {
    type Error;
    fn arm(&mut self) -> Result<(), Self::Error>;
    fn pending(&mut self) -> bool;
    fn clear(&mut self);
    fn disable(&mut self);
}

pub struct Ra4m1Irq<B: IrqBackend> {
    backend: B,
    lease: Ra4m1LeaseGuard,
}

impl<B: IrqBackend> Ra4m1Irq<B> {
    pub fn try_mount(mut backend: B, pin: u8, owner: u8) -> Result<Self, DeepError<B::Error>> {
        if header_irq_channel(pin).is_none() {
            return Err(DeepError::InvalidConfig);
        }
        let lease = Ra4m1Leases::acquire_guard(LeaseId::new(LeaseClass::Irq, pin), owner)
            .map_err(DeepError::Lease)?;
        backend.arm().map_err(DeepError::Backend)?;
        Ok(Self { backend, lease })
    }

    pub fn take_pending(&mut self) -> Result<bool, DeepError<B::Error>> {
        self.lease.ensure_live().map_err(DeepError::Lease)?;
        let pending = self.backend.pending();
        if pending {
            self.backend.clear();
        }
        Ok(pending)
    }
}

impl<B: IrqBackend> Drop for Ra4m1Irq<B> {
    fn drop(&mut self) {
        self.backend.disable();
    }
}

pub trait PulseBackend {
    type Error;
    fn read_width_us(&mut self, timeout_us: u32) -> Result<Option<u32>, Self::Error>;
}

pub struct Ra4m1Pulse<B> {
    backend: B,
    lease: Ra4m1LeaseGuard,
}

impl<B: PulseBackend> Ra4m1Pulse<B> {
    pub fn try_mount(backend: B, owner: u8) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            lease: Ra4m1Leases::acquire_guard(LeaseId::PRIMARY_PULSE, owner)?,
        })
    }

    pub fn read_width_us(&mut self, timeout_us: u32) -> Result<Option<u32>, DeepError<B::Error>> {
        self.lease.ensure_live().map_err(DeepError::Lease)?;
        if timeout_us == 0 {
            return Err(DeepError::InvalidConfig);
        }
        self.backend
            .read_width_us(timeout_us)
            .map_err(DeepError::Backend)
    }
}

pub trait WatchdogBackend {
    type Error;
    fn arm(&mut self, timeout_ms: u32) -> Result<(), Self::Error>;
    fn feed(&mut self) -> Result<(), Self::Error>;
}

static RA4M1_WATCHDOG_ARMED: AtomicBool = AtomicBool::new(false);

pub struct Ra4m1Watchdog<B> {
    backend: B,
    lease: Option<Ra4m1LeaseGuard>,
    armed: bool,
}

impl<B: WatchdogBackend> Ra4m1Watchdog<B> {
    pub fn try_mount(backend: B, owner: u8) -> Result<Self, LeaseError> {
        if RA4M1_WATCHDOG_ARMED.load(Ordering::Acquire) {
            return Err(LeaseError::AlreadyHeld);
        }
        Ok(Self {
            backend,
            lease: Some(Ra4m1Leases::acquire_guard(LeaseId::SYSTEM_WATCHDOG, owner)?),
            armed: false,
        })
    }

    pub fn arm(&mut self, timeout_ms: u32) -> Result<(), DeepError<B::Error>> {
        self.lease
            .as_ref()
            .ok_or(DeepError::InvalidConfig)?
            .ensure_live()
            .map_err(DeepError::Lease)?;
        if self.armed || timeout_ms == 0 {
            return Err(DeepError::InvalidConfig);
        }
        RA4M1_WATCHDOG_ARMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| DeepError::InvalidConfig)?;
        self.backend.arm(timeout_ms).map_err(DeepError::Backend)?;
        self.armed = true;
        Ok(())
    }

    pub fn feed(&mut self) -> Result<(), DeepError<B::Error>> {
        self.lease
            .as_ref()
            .ok_or(DeepError::InvalidConfig)?
            .ensure_live()
            .map_err(DeepError::Lease)?;
        if !self.armed {
            return Err(DeepError::InvalidConfig);
        }
        self.backend.feed().map_err(DeepError::Backend)
    }
}

impl<B> Drop for Ra4m1Watchdog<B> {
    fn drop(&mut self) {
        if self.armed {
            // RA4M1 WDT cannot be stopped after activation. Preserve ownership
            // until reset instead of publishing a false reusable lease.
            if let Some(lease) = self.lease.take() {
                core::mem::forget(lease);
            }
        }
    }
}

pub trait RtcBackend {
    type Error;
    fn unix_seconds(&mut self) -> Result<u64, Self::Error>;
}

pub struct Ra4m1Rtc<B> {
    backend: B,
    lease: Ra4m1LeaseGuard,
}

impl<B: RtcBackend> Ra4m1Rtc<B> {
    pub fn try_mount(backend: B, owner: u8) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            lease: Ra4m1Leases::acquire_guard(LeaseId::SYSTEM_RTC, owner)?,
        })
    }

    pub fn unix_seconds(&mut self) -> Result<u64, DeepError<B::Error>> {
        self.lease.ensure_live().map_err(DeepError::Lease)?;
        self.backend.unix_seconds().map_err(DeepError::Backend)
    }
}

pub trait DataFlashBackend {
    type Error;
    fn erase_block(&mut self, absolute_address: u32) -> Result<(), Self::Error>;
    fn write(&mut self, absolute_address: u32, bytes: &[u8]) -> Result<(), Self::Error>;
    fn read(&mut self, absolute_address: u32, bytes: &mut [u8]) -> Result<(), Self::Error>;
}

pub struct Ra4m1DataFlash<B> {
    backend: B,
    lease: Ra4m1LeaseGuard,
}

impl<B: DataFlashBackend> Ra4m1DataFlash<B> {
    pub fn try_mount(backend: B, owner: u8) -> Result<Self, LeaseError> {
        Ok(Self {
            backend,
            lease: Ra4m1Leases::acquire_guard(LeaseId::APPLICATION_FLASH, owner)?,
        })
    }

    fn address(offset: u32, length: usize) -> Option<u32> {
        let length = u32::try_from(length).ok()?;
        offset
            .checked_add(length)
            .filter(|end| *end <= RA4M1_DATA_FLASH_LEN)
            .map(|_| RA4M1_DATA_FLASH_START + offset)
    }

    pub fn erase(&mut self, offset: u32) -> Result<(), DeepError<B::Error>> {
        self.lease.ensure_live().map_err(DeepError::Lease)?;
        if offset % RA4M1_DATA_FLASH_ERASE_SIZE != 0 {
            return Err(DeepError::InvalidConfig);
        }
        let address = Self::address(offset, RA4M1_DATA_FLASH_ERASE_SIZE as usize)
            .ok_or(DeepError::InvalidConfig)?;
        self.backend
            .erase_block(address)
            .map_err(DeepError::Backend)
    }

    pub fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), DeepError<B::Error>> {
        self.lease.ensure_live().map_err(DeepError::Lease)?;
        if bytes.is_empty() {
            return Err(DeepError::InvalidConfig);
        }
        let address = Self::address(offset, bytes.len()).ok_or(DeepError::InvalidConfig)?;
        self.backend
            .write(address, bytes)
            .map_err(DeepError::Backend)
    }

    pub fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), DeepError<B::Error>> {
        self.lease.ensure_live().map_err(DeepError::Lease)?;
        let address = Self::address(offset, bytes.len()).ok_or(DeepError::InvalidConfig)?;
        self.backend
            .read(address, bytes)
            .map_err(DeepError::Backend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nobro_hal::HalLease;

    #[derive(Default)]
    struct MockFlash {
        erased: u32,
        written: u32,
    }

    struct MockWatchdog;

    impl WatchdogBackend for MockWatchdog {
        type Error = ();

        fn arm(&mut self, _: u32) -> Result<(), Self::Error> {
            Ok(())
        }

        fn feed(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    impl DataFlashBackend for MockFlash {
        type Error = ();
        fn erase_block(&mut self, address: u32) -> Result<(), Self::Error> {
            self.erased = address;
            Ok(())
        }
        fn write(&mut self, address: u32, _: &[u8]) -> Result<(), Self::Error> {
            self.written = address;
            Ok(())
        }
        fn read(&mut self, _: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            bytes.fill(0xa5);
            Ok(())
        }
    }

    #[test]
    fn exact_header_map_and_flash_bounds_are_enforced() {
        let _lock = crate::TEST_LOCK.lock().unwrap();
        assert_eq!(HEADER_PINS[5], (1, 7));
        assert_eq!(HEADER_PINS[14], (0, 14));
        assert_eq!(RA4M1_DATA_FLASH_ERASE_SIZE, 1024);
        assert_eq!(RA4M1_DATA_FLASH_WRITE_SIZE, 1);
        let mut flash = Ra4m1DataFlash::try_mount(MockFlash::default(), 42).unwrap();
        assert_eq!(flash.erase(1), Err(DeepError::InvalidConfig));
        assert_eq!(flash.erase(0), Ok(()));
        assert_eq!(flash.write(0, &[]), Err(DeepError::InvalidConfig));
        assert_eq!(flash.write(0, &[1, 2, 3]), Ok(()));
        assert_eq!(flash.write(RA4M1_DATA_FLASH_LEN - 4, &[1, 2, 3, 4]), Ok(()));
        assert_eq!(
            flash.write(RA4M1_DATA_FLASH_LEN, &[1, 2, 3, 4]),
            Err(DeepError::InvalidConfig)
        );
    }

    #[test]
    fn gpio_pin_lease_conflicts_with_pwm_and_adc() {
        let _lock = crate::TEST_LOCK.lock().unwrap();
        let d5 = Ra4m1Gpio::try_new(5, GpioMode::Input, 1).unwrap();
        assert!(matches!(
            Ra4m1Leases::acquire_guard(LeaseId::PRIMARY_PWM, 2),
            Err(LeaseError::AlreadyHeld)
        ));
        drop(d5);
        let a0 = Ra4m1Gpio::try_new(14, GpioMode::Input, 3).unwrap();
        assert!(matches!(
            Ra4m1Leases::acquire_guard(LeaseId::PRIMARY_ADC, 4),
            Err(LeaseError::AlreadyHeld)
        ));
        drop(a0);
    }

    #[test]
    fn armed_watchdog_cannot_be_reissued_after_owner_recovery() {
        let _lock = crate::TEST_LOCK.lock().unwrap();
        let owner = 77;
        let mut watchdog = Ra4m1Watchdog::try_mount(MockWatchdog, owner).unwrap();
        watchdog.arm(4_000).unwrap();
        drop(watchdog);
        assert_eq!(Ra4m1Leases::release_all_for_owner(owner), 1);
        assert!(matches!(
            Ra4m1Watchdog::try_mount(MockWatchdog, owner + 1),
            Err(LeaseError::AlreadyHeld)
        ));
    }
}
