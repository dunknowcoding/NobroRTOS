//! RP2350A implementations for the shared deep-provider lifecycle.

use core::convert::Infallible;

use nobro_hal::{
    Rp2CacheBackend, Rp2FlashBackend, Rp2PulseBackend, Rp2RtcBackend, Rp2WatchdogBackend,
};
use rp235x_hal::{self as hal, powman::Powman};

const SIO_GPIO_IN: *const u32 = 0xd000_0004 as *const u32;
const TIMERAWL: *const u32 = 0x400b_0028 as *const u32;
const STORAGE_XIP: u32 = 0x103f_c000;
const STORAGE_OFFSET: u32 = STORAGE_XIP - 0x1000_0000;
const STORAGE_LEN: u32 = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rp2350DeepError {
    InvalidPin,
}

pub struct Rp2350Pulse {
    pin: u8,
}

impl Rp2350Pulse {
    pub fn new(pin: u8) -> Result<Self, Rp2350DeepError> {
        if pin >= 30 {
            return Err(Rp2350DeepError::InvalidPin);
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

impl Rp2PulseBackend for Rp2350Pulse {
    type Error = Rp2350DeepError;

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

pub struct Rp2350Watchdog(pub hal::Watchdog);

impl Rp2WatchdogBackend for Rp2350Watchdog {
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

pub struct Rp2350Rtc(pub Powman);

impl Rp2RtcBackend for Rp2350Rtc {
    type Error = Infallible;

    fn ticks(&mut self) -> Result<u64, Self::Error> {
        Ok(self.0.aot_get_time())
    }

    fn ticks_per_second(&self) -> u32 {
        1_000
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

pub struct Rp2350Flash;

impl Rp2350Flash {
    pub unsafe fn before_core1_start() -> Self {
        Self
    }
}

impl Rp2FlashBackend for Rp2350Flash {
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

pub struct Rp2350Cache;

impl Rp2CacheBackend for Rp2350Cache {
    type Error = Infallible;

    fn flush_xip(&mut self) -> Result<(), Self::Error> {
        unsafe { hal::rom_data::flash_flush_cache() };
        Ok(())
    }
}
