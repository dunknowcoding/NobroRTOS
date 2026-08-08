//! RP2040 implementations for the shared deep-provider lifecycle.

use core::convert::Infallible;

use nobro_hal::{
    Rp2CacheBackend, Rp2FlashBackend, Rp2PulseBackend, Rp2RtcBackend, Rp2WatchdogBackend,
};
use rp2040_hal::{self as hal, rtc::RealTimeClock};

const SIO_GPIO_IN: *const u32 = 0xd000_0004 as *const u32;
const TIMERAWL: *const u32 = 0x4005_4028 as *const u32;
const STORAGE_XIP: u32 = 0x101f_c000;
const STORAGE_OFFSET: u32 = STORAGE_XIP - 0x1000_0000;
const STORAGE_LEN: u32 = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rp2040DeepError {
    InvalidPin,
    Rtc,
}

pub struct Rp2040Pulse {
    pin: u8,
}

impl Rp2040Pulse {
    pub fn new(pin: u8) -> Result<Self, Rp2040DeepError> {
        if pin >= 30 {
            return Err(Rp2040DeepError::InvalidPin);
        }
        Ok(Self { pin })
    }

    fn now_us() -> u32 {
        unsafe { TIMERAWL.read_volatile() }
    }

    fn high(&self) -> bool {
        unsafe { SIO_GPIO_IN.read_volatile() & (1 << self.pin) != 0 }
    }
}

impl Rp2PulseBackend for Rp2040Pulse {
    type Error = Rp2040DeepError;

    fn read_pulse_us(&mut self, timeout_us: u32) -> Result<Option<u32>, Self::Error> {
        let began = Self::now_us();
        while !self.high() {
            if Self::now_us().wrapping_sub(began) >= timeout_us {
                return Ok(None);
            }
            core::hint::spin_loop();
        }
        let rising = Self::now_us();
        while self.high() {
            if Self::now_us().wrapping_sub(rising) >= timeout_us {
                return Ok(None);
            }
            core::hint::spin_loop();
        }
        Ok(Some(Self::now_us().wrapping_sub(rising)))
    }
}

pub struct Rp2040Watchdog(pub hal::Watchdog);

impl Rp2WatchdogBackend for Rp2040Watchdog {
    type Error = Infallible;

    fn arm(&mut self, timeout_us: u32) -> Result<(), Self::Error> {
        self.0.pause_on_debug(true);
        self.0.start(fugit::MicrosDurationU32::micros(timeout_us));
        Ok(())
    }

    fn feed(&mut self) -> Result<(), Self::Error> {
        hal::Watchdog::feed(&self.0);
        Ok(())
    }

    fn reset_observed(&self) -> bool {
        unsafe { &*hal::pac::WATCHDOG::PTR }
            .reason()
            .read()
            .timer()
            .bit_is_set()
    }
}

pub struct Rp2040Rtc(pub RealTimeClock);

const fn leap_year(year: u16) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn seconds_from_datetime(now: &hal::rtc::DateTime) -> u64 {
    let year = u64::from(now.year);
    let leap_days_before_year = (year + 3) / 4 - (year + 99) / 100 + (year + 399) / 400;
    let mut days = year * 365 + leap_days_before_year;
    const MONTH_DAYS: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for month in 1..now.month {
        days += u64::from(MONTH_DAYS[usize::from(month - 1)]);
        if month == 2 && leap_year(now.year) {
            days += 1;
        }
    }
    days += u64::from(now.day - 1);
    (((days * 24 + u64::from(now.hour)) * 60 + u64::from(now.minute)) * 60) + u64::from(now.second)
}

impl Rp2RtcBackend for Rp2040Rtc {
    type Error = Rp2040DeepError;

    fn ticks(&mut self) -> Result<u64, Self::Error> {
        let now = self.0.now().map_err(|_| Rp2040DeepError::Rtc)?;
        Ok(seconds_from_datetime(&now))
    }

    fn ticks_per_second(&self) -> u32 {
        1
    }
}

type VoidFn = unsafe extern "C" fn();
type EraseFn = unsafe extern "C" fn(u32, usize, u32, u8);
type ProgramFn = unsafe extern "C" fn(u32, *const u8, usize);

#[inline(never)]
#[link_section = ".data.nobro_flash_command"]
unsafe fn erase_from_ram(
    connect: VoidFn,
    exit_xip: VoidFn,
    erase: EraseFn,
    flush: VoidFn,
    enter_xip: VoidFn,
    offset: u32,
) {
    connect();
    exit_xip();
    erase(offset, 4096, 65536, 0xd8);
    flush();
    enter_xip();
}

#[inline(never)]
#[link_section = ".data.nobro_flash_command"]
unsafe fn program_from_ram(
    connect: VoidFn,
    exit_xip: VoidFn,
    program: ProgramFn,
    flush: VoidFn,
    enter_xip: VoidFn,
    offset: u32,
    page: *const u8,
) {
    connect();
    exit_xip();
    program(offset, page, 256);
    flush();
    enter_xip();
}

/// Reserved-flash backend. Construction is unsafe because core 1 and every
/// flash-reading DMA master must remain stopped throughout erase/program.
pub struct Rp2040Flash;

impl Rp2040Flash {
    pub unsafe fn before_core1_start() -> Self {
        Self
    }
}

impl Rp2FlashBackend for Rp2040Flash {
    type Error = Infallible;

    fn storage_len(&self) -> u32 {
        STORAGE_LEN
    }

    fn erase_sector(&mut self, offset: u32) -> Result<(), Self::Error> {
        let connect = hal::rom_data::connect_internal_flash::ptr();
        let exit_xip = hal::rom_data::flash_exit_xip::ptr();
        let erase = hal::rom_data::flash_range_erase::ptr();
        let flush = hal::rom_data::flash_flush_cache::ptr();
        let enter_xip = hal::rom_data::flash_enter_cmd_xip::ptr();
        // The RP HAL critical section combines local interrupt exclusion with
        // the cross-core spinlock. The unsafe constructor additionally proves
        // that core 1 and every XIP-reading DMA master are stopped before this
        // boot-time flash transaction starts.
        critical_section::with(|_| unsafe {
            erase_from_ram(
                connect,
                exit_xip,
                erase,
                flush,
                enter_xip,
                STORAGE_OFFSET + offset,
            )
        });
        Ok(())
    }

    fn program_page(&mut self, offset: u32, page: &[u8; 256]) -> Result<(), Self::Error> {
        let connect = hal::rom_data::connect_internal_flash::ptr();
        let exit_xip = hal::rom_data::flash_exit_xip::ptr();
        let program = hal::rom_data::flash_range_program::ptr();
        let flush = hal::rom_data::flash_flush_cache::ptr();
        let enter_xip = hal::rom_data::flash_enter_cmd_xip::ptr();
        // Use the RP cross-core critical section for the complete XIP-off
        // interval; local-only interrupt masking cannot exclude the other core.
        critical_section::with(|_| unsafe {
            program_from_ram(
                connect,
                exit_xip,
                program,
                flush,
                enter_xip,
                STORAGE_OFFSET + offset,
                page.as_ptr(),
            )
        });
        Ok(())
    }

    fn read(&self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        unsafe {
            core::ptr::copy_nonoverlapping(
                (STORAGE_XIP + offset) as *const u8,
                bytes.as_mut_ptr(),
                bytes.len(),
            );
        }
        Ok(())
    }
}

pub struct Rp2040Cache;

impl Rp2CacheBackend for Rp2040Cache {
    type Error = Infallible;

    fn flush_xip(&mut self) -> Result<(), Self::Error> {
        unsafe { hal::rom_data::flash_flush_cache() };
        Ok(())
    }
}
