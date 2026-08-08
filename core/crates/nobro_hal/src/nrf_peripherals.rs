//! Lifecycle-complete native peripherals for the exact ProMicro nRF52840.
//!
//! These providers use the same physical pin map and `0xE9000` application
//! ceiling as ArduinoNRF. Every recoverable peripheral is generation-leased;
//! the one-way hardware watchdog deliberately cannot be remounted after arm.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::lease::{LeaseError, LeaseGuard, Resource, ResourceLease};
use crate::traits::{HalAdcChannel, HalByteIo, HalPower, HalReset, IdleMode};

pub const APP_STORAGE_START: u32 = 0x000E_5000;
pub const APP_STORAGE_END: u32 = 0x000E_9000;
pub const FLASH_PAGE_SIZE: u32 = 4096;

const DIGITAL_TO_ABSOLUTE: [u8; 22] = [
    6, 8, 17, 20, 22, 24, 32, 11, 36, 38, 9, 10, 43, 45, 47, 2, 29, 31, 33, 34, 39, 15,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrfPeripheralError {
    InvalidPin,
    InvalidChannel,
    InvalidBaudrate,
    InvalidLength,
    InvalidAddress,
    Lease(LeaseError),
    Busy,
    Timeout,
    UnsupportedOnHost,
}

impl From<LeaseError> for NrfPeripheralError {
    fn from(value: LeaseError) -> Self {
        Self::Lease(value)
    }
}

#[cfg(target_arch = "arm")]
#[inline]
unsafe fn read32(address: u32) -> u32 {
    (address as *const u32).read_volatile()
}

#[cfg(target_arch = "arm")]
#[inline]
unsafe fn write32(address: u32, value: u32) {
    (address as *mut u32).write_volatile(value);
}

pub const fn absolute_pin(digital_pin: u8) -> Option<u8> {
    if (digital_pin as usize) < DIGITAL_TO_ABSOLUTE.len() {
        Some(DIGITAL_TO_ABSOLUTE[digital_pin as usize])
    } else {
        None
    }
}

#[cfg(target_arch = "arm")]
const fn gpio_base(absolute: u8) -> u32 {
    if absolute < 32 {
        0x5000_0000
    } else {
        0x5000_0300
    }
}

#[cfg(target_arch = "arm")]
const fn gpio_bit(absolute: u8) -> u8 {
    absolute & 31
}

/// One owner for the board's exposed GPIO bank. Pins are addressed by Arduino
/// digital number so package and native firmware cannot silently disagree.
pub struct NrfGpioPort {
    lease: LeaseGuard,
}

impl NrfGpioPort {
    pub fn try_acquire(owner: u8) -> Result<Self, LeaseError> {
        Ok(Self {
            lease: ResourceLease::acquire_guard(Resource::Gpio, owner)?,
        })
    }

    pub fn configure_output(&self, digital_pin: u8, _high: bool) -> Result<(), NrfPeripheralError> {
        self.lease.ensure_live()?;
        let _absolute = absolute_pin(digital_pin).ok_or(NrfPeripheralError::InvalidPin)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            let base = gpio_base(_absolute);
            let bit = gpio_bit(_absolute);
            write32(base + if _high { 0x508 } else { 0x50c }, 1 << bit);
            // DIR=output, INPUT=disconnect, standard S0S1 drive, no sense.
            write32(base + 0x700 + u32::from(bit) * 4, 0x0000_0003);
        }
        Ok(())
    }

    pub fn configure_input_pullup(&self, digital_pin: u8) -> Result<(), NrfPeripheralError> {
        self.lease.ensure_live()?;
        let _absolute = absolute_pin(digital_pin).ok_or(NrfPeripheralError::InvalidPin)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            let bit = gpio_bit(_absolute);
            // DIR=input, INPUT=connect, PULL=up.
            write32(gpio_base(_absolute) + 0x700 + u32::from(bit) * 4, 3 << 2);
        }
        Ok(())
    }

    pub fn write(&self, digital_pin: u8, _high: bool) -> Result<(), NrfPeripheralError> {
        self.lease.ensure_live()?;
        let _absolute = absolute_pin(digital_pin).ok_or(NrfPeripheralError::InvalidPin)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            write32(
                gpio_base(_absolute) + if _high { 0x508 } else { 0x50c },
                1 << gpio_bit(_absolute),
            );
        }
        Ok(())
    }

    pub fn read(&self, digital_pin: u8) -> Result<bool, NrfPeripheralError> {
        self.lease.ensure_live()?;
        let _absolute = absolute_pin(digital_pin).ok_or(NrfPeripheralError::InvalidPin)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            Ok(read32(gpio_base(_absolute) + 0x510) & (1 << gpio_bit(_absolute)) != 0)
        }
        #[cfg(not(target_arch = "arm"))]
        Ok(false)
    }
}

/// Hardware-latched edge input using one GPIOTE channel.
pub struct NrfGpioteInput {
    lease: LeaseGuard,
    channel: u8,
}

impl NrfGpioteInput {
    pub fn try_new(owner: u8, channel: u8, digital_pin: u8) -> Result<Self, NrfPeripheralError> {
        if channel >= 8 {
            return Err(NrfPeripheralError::InvalidChannel);
        }
        let _absolute = absolute_pin(digital_pin).ok_or(NrfPeripheralError::InvalidPin)?;
        let lease = ResourceLease::acquire_guard(Resource::Gpiote, owner)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            let bit = gpio_bit(_absolute);
            // Input connected, no pull. Owning GPIOTE conflicts with the
            // coarse GPIO-bank lease, so this pin configuration cannot race a
            // separate NobroRTOS GPIO owner.
            write32(gpio_base(_absolute) + 0x700 + u32::from(bit) * 4, 0);
            let config = 1 | (u32::from(bit) << 8) | (u32::from(_absolute >= 32) << 13) | (3 << 16); // event mode, toggle polarity
            write32(0x4000_6000 + 0x510 + u32::from(channel) * 4, config);
            write32(0x4000_6000 + 0x100 + u32::from(channel) * 4, 0);
        }
        Ok(Self { lease, channel })
    }

    pub fn take_edge(&self) -> Result<bool, NrfPeripheralError> {
        self.lease.ensure_live()?;
        #[cfg(target_arch = "arm")]
        unsafe {
            let event = 0x4000_6000 + 0x100 + u32::from(self.channel) * 4;
            if read32(event) != 0 {
                write32(event, 0);
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub const fn event_address(&self) -> u32 {
        0x4000_6000 + 0x100 + self.channel as u32 * 4
    }
}

/// Hardware timestamped pulse-width capture. GPIOTE latches both edges and PPI
/// captures TIMER2 before software observes the event.
pub struct NrfPulseCapture {
    input: NrfGpioteInput,
    ppi: LeaseGuard,
    timer: LeaseGuard,
    rising_at: Option<u32>,
    digital_pin: u8,
}

impl NrfPulseCapture {
    pub fn try_new(owner: u8, digital_pin: u8) -> Result<Self, NrfPeripheralError> {
        let input = NrfGpioteInput::try_new(owner, 1, digital_pin)?;
        let ppi = ResourceLease::acquire_guard(Resource::Ppi, owner)?;
        let timer = ResourceLease::acquire_guard(Resource::Timer2, owner)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            write32(0x4000_A004, 1); // STOP
            write32(0x4000_A00C, 1); // CLEAR
            write32(0x4000_A504, 0); // timer mode
            write32(0x4000_A508, 3); // 32 bit
            write32(0x4000_A510, 4); // 1 MHz
            write32(0x4001_F000 + 0x510 + 2 * 8, input.event_address());
            write32(0x4001_F000 + 0x514 + 2 * 8, 0x4000_A040); // CAPTURE[0]
            write32(0x4001_F000 + 0x504, 1 << 2); // CHENSET
            write32(0x4000_A000, 1); // START
        }
        Ok(Self {
            input,
            ppi,
            timer,
            rising_at: None,
            digital_pin,
        })
    }

    pub fn poll_width_us(&mut self) -> Result<Option<u32>, NrfPeripheralError> {
        self.ppi.ensure_live()?;
        self.timer.ensure_live()?;
        if !self.input.take_edge()? {
            return Ok(None);
        }
        #[cfg(target_arch = "arm")]
        let captured = unsafe { read32(0x4000_A540) };
        #[cfg(not(target_arch = "arm"))]
        let captured = 0;
        let high = {
            let _absolute = absolute_pin(self.digital_pin).ok_or(NrfPeripheralError::InvalidPin)?;
            #[cfg(target_arch = "arm")]
            unsafe {
                read32(gpio_base(_absolute) + 0x510) & (1 << gpio_bit(_absolute)) != 0
            }
            #[cfg(not(target_arch = "arm"))]
            false
        };
        if high {
            self.rising_at = Some(captured);
            Ok(None)
        } else {
            Ok(self
                .rising_at
                .take()
                .map(|start| captured.wrapping_sub(start)))
        }
    }
}

/// Blocking EasyDMA UART on the board's silk TX/RX pins (P0.06/P0.08).
pub struct NrfUarte0 {
    lease: LeaseGuard,
}

impl NrfUarte0 {
    pub fn try_new(owner: u8, baud: u32) -> Result<Self, NrfPeripheralError> {
        let baudrate = match baud {
            9_600 => 0x0027_5000,
            38_400 => 0x009D_5000,
            115_200 => 0x01D7_E000,
            1_000_000 => 0x1000_0000,
            _ => return Err(NrfPeripheralError::InvalidBaudrate),
        };
        let lease = ResourceLease::acquire_guard(Resource::Uarte0, owner)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            write32(0x4000_250c, 6); // PSEL.TXD
            write32(0x4000_2514, 8); // PSEL.RXD
            write32(0x4000_2508, 0xFFFF_FFFF); // RTS disconnected
            write32(0x4000_2510, 0xFFFF_FFFF); // CTS disconnected
            write32(0x4000_2524, baudrate);
            write32(0x4000_256c, 0); // no parity/flow control
            write32(0x4000_2500, 8); // UARTE enabled
        }
        let _ = baudrate;
        Ok(Self { lease })
    }

    #[cfg(target_arch = "arm")]
    fn wait_event(&self, event: u32) -> Result<(), NrfPeripheralError> {
        for _ in 0..1_000_000 {
            if unsafe { read32(event) } != 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(NrfPeripheralError::Timeout)
    }
}

impl HalByteIo for NrfUarte0 {
    type Error = NrfPeripheralError;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        self.lease.ensure_live()?;
        if bytes.is_empty() {
            return Ok(0);
        }
        #[cfg(target_arch = "arm")]
        {
            let completed = unsafe {
                write32(0x4000_2110, 0); // EVENTS_ENDRX
                write32(0x4000_2144, 0); // EVENTS_RXTO
                write32(0x4000_2534, bytes.as_mut_ptr() as u32);
                write32(0x4000_2538, bytes.len().min(u16::MAX as usize) as u32);
                write32(0x4000_2000, 1); // STARTRX
                let mut completed = false;
                for _ in 0..4096 {
                    if read32(0x4000_2110) != 0 {
                        completed = true;
                        break;
                    }
                    core::hint::spin_loop();
                }
                completed
            };
            if !completed {
                unsafe { write32(0x4000_2004, 1) }; // STOPRX
                                                    // AMOUNT is not final until RXTO acknowledges STOPRX.
                self.wait_event(0x4000_2144)?;
            }
            Ok(unsafe { read32(0x4000_253c) } as usize)
        }
        #[cfg(not(target_arch = "arm"))]
        Err(NrfPeripheralError::UnsupportedOnHost)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.lease.ensure_live()?;
        #[cfg(target_arch = "arm")]
        for chunk in bytes.chunks(u16::MAX as usize) {
            unsafe {
                write32(0x4000_2120, 0); // EVENTS_ENDTX
                write32(0x4000_2544, chunk.as_ptr() as u32);
                write32(0x4000_2548, chunk.len() as u32);
                write32(0x4000_2008, 1); // STARTTX
            }
            self.wait_event(0x4000_2120)?;
        }
        #[cfg(not(target_arch = "arm"))]
        if !bytes.is_empty() {
            return Err(NrfPeripheralError::UnsupportedOnHost);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.lease.ensure_live()?;
        Ok(())
    }
}

/// Single-ended SAADC channel for the three exposed analog pads.
pub struct NrfSaadc {
    lease: LeaseGuard,
    analog_input: u8,
}

impl NrfSaadc {
    pub fn try_new(owner: u8, analog_pin: u8) -> Result<Self, NrfPeripheralError> {
        let analog_input = match analog_pin {
            0 => 0,
            1 => 5,
            2 => 7,
            _ => return Err(NrfPeripheralError::InvalidChannel),
        };
        let lease = ResourceLease::acquire_guard(Resource::Saadc, owner)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            write32(0x4000_7500, 1); // ENABLE
            write32(0x4000_7510, u32::from(analog_input + 1));
            write32(0x4000_7514, 0); // PSELN disconnected
                                     // gain 1/6, internal 0.6 V reference, 10 us acquisition, single-ended
            write32(0x4000_7518, 2 << 16);
            write32(0x4000_75f0, 2); // 12 bit
            write32(0x4000_75f4, 0); // no oversampling
        }
        Ok(Self {
            lease,
            analog_input,
        })
    }

    pub const fn analog_input(&self) -> u8 {
        self.analog_input
    }
}

impl HalAdcChannel for NrfSaadc {
    type Error = NrfPeripheralError;

    fn max_sample(&self) -> u16 {
        4095
    }

    fn read(&mut self) -> Result<u16, Self::Error> {
        self.lease.ensure_live()?;
        #[cfg(target_arch = "arm")]
        unsafe {
            let mut sample = 0i16;
            write32(0x4000_7104, 0); // EVENTS_END
            write32(0x4000_762c, (&mut sample as *mut i16) as u32);
            write32(0x4000_7630, 1);
            write32(0x4000_7000, 1); // START
            write32(0x4000_7004, 1); // SAMPLE
            for _ in 0..1_000_000 {
                if read32(0x4000_7104) != 0 {
                    return Ok(sample.max(0) as u16);
                }
                core::hint::spin_loop();
            }
            Err(NrfPeripheralError::Timeout)
        }
        #[cfg(not(target_arch = "arm"))]
        Err(NrfPeripheralError::UnsupportedOnHost)
    }
}

/// RTC2 System-ON retained counter at 32.768 kHz.
pub struct NrfRtc2 {
    lease: LeaseGuard,
}

impl NrfRtc2 {
    pub fn try_start(owner: u8) -> Result<Self, LeaseError> {
        let lease = ResourceLease::acquire_guard(Resource::Rtc2, owner)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            write32(0x4002_4008, 1); // CLEAR
            write32(0x4002_4508, 0); // PRESCALER, 32768 Hz
            write32(0x4002_4000, 1); // START
        }
        Ok(Self { lease })
    }

    pub fn ticks(&self) -> Result<u32, LeaseError> {
        self.lease.ensure_live()?;
        #[cfg(target_arch = "arm")]
        unsafe {
            Ok(read32(0x4002_4504) & 0x00FF_FFFF)
        }
        #[cfg(not(target_arch = "arm"))]
        Ok(0)
    }
}

/// Dedicated four-page persistent application region. The linker excludes it
/// from executable FLASH in both noSD and S140 layouts.
pub struct NrfNvmc {
    lease: LeaseGuard,
}

impl NrfNvmc {
    pub const PAGE_COUNT: u32 = (APP_STORAGE_END - APP_STORAGE_START) / FLASH_PAGE_SIZE;

    pub fn try_acquire(owner: u8) -> Result<Self, LeaseError> {
        Ok(Self {
            lease: ResourceLease::acquire_guard(Resource::Nvmc, owner)?,
        })
    }

    pub const fn word_address(page: u32, word: u32) -> Option<u32> {
        if page >= Self::PAGE_COUNT || word >= FLASH_PAGE_SIZE / 4 {
            None
        } else {
            Some(APP_STORAGE_START + page * FLASH_PAGE_SIZE + word * 4)
        }
    }

    #[cfg(target_arch = "arm")]
    fn wait_ready() -> Result<(), NrfPeripheralError> {
        for _ in 0..2_000_000 {
            if unsafe { read32(0x4001_E400) } & 1 != 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(NrfPeripheralError::Timeout)
    }

    pub fn erase_page(&mut self, page: u32) -> Result<(), NrfPeripheralError> {
        self.lease.ensure_live()?;
        let _address = Self::word_address(page, 0).ok_or(NrfPeripheralError::InvalidAddress)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            write32(0x4001_E504, 2);
            Self::wait_ready()?;
            write32(0x4001_E508, _address);
            Self::wait_ready()?;
            write32(0x4001_E504, 0);
            Self::wait_ready()?;
        }
        Ok(())
    }

    pub fn write_word(
        &mut self,
        page: u32,
        word: u32,
        _value: u32,
    ) -> Result<(), NrfPeripheralError> {
        self.lease.ensure_live()?;
        let _address = Self::word_address(page, word).ok_or(NrfPeripheralError::InvalidAddress)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            if read32(_address) != 0xFFFF_FFFF {
                return Err(NrfPeripheralError::Busy);
            }
            write32(0x4001_E504, 1);
            Self::wait_ready()?;
            write32(_address, _value);
            Self::wait_ready()?;
            write32(0x4001_E504, 0);
            Self::wait_ready()?;
        }
        Ok(())
    }

    pub fn read_word(&self, page: u32, word: u32) -> Result<u32, NrfPeripheralError> {
        self.lease.ensure_live()?;
        let _address = Self::word_address(page, word).ok_or(NrfPeripheralError::InvalidAddress)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            Ok(read32(_address))
        }
        #[cfg(not(target_arch = "arm"))]
        Ok(0xFFFF_FFFF)
    }
}

static WATCHDOG_ARMED: AtomicBool = AtomicBool::new(false);

/// One-way hardware watchdog. Once armed, the provider intentionally remains
/// globally owned until reset because nRF52840 hardware cannot stop it.
pub struct NrfWatchdog;

impl NrfWatchdog {
    pub const RELOAD: u32 = 0x6E52_4635;

    pub const fn valid_timeout_ticks(ticks: u32) -> bool {
        ticks >= 15
    }

    pub fn try_arm(timeout_ticks: u32) -> Result<Self, NrfPeripheralError> {
        if !Self::valid_timeout_ticks(timeout_ticks) {
            return Err(NrfPeripheralError::InvalidLength);
        }
        WATCHDOG_ARMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| NrfPeripheralError::Busy)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            write32(0x4001_0504, timeout_ticks);
            write32(0x4001_0508, 1); // run during System-ON sleep
            write32(0x4001_050c, 1); // pause while debugger halts the CPU
            write32(0x4001_0600, Self::RELOAD);
            write32(0x4001_0000, 1);
        }
        Ok(Self)
    }

    pub fn feed(&mut self) {
        #[cfg(target_arch = "arm")]
        unsafe {
            write32(0x4001_0600, Self::RELOAD);
        }
    }

    pub fn watchdog_reset_observed() -> bool {
        #[cfg(target_arch = "arm")]
        unsafe {
            read32(0x4000_0400) & (1 << 1) != 0
        }
        #[cfg(not(target_arch = "arm"))]
        false
    }

    pub fn reset_cause_bits() -> u32 {
        #[cfg(target_arch = "arm")]
        unsafe {
            read32(0x4000_0400)
        }
        #[cfg(not(target_arch = "arm"))]
        0
    }

    pub fn clear_reset_cause(bits: u32) {
        #[cfg(target_arch = "arm")]
        unsafe {
            // RESETREAS is write-one-to-clear.
            write32(0x4000_0400, bits);
        }
        #[cfg(not(target_arch = "arm"))]
        let _ = bits;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NrfResetCause(pub u32);

pub struct NrfReset;

impl HalReset for NrfReset {
    type Cause = NrfResetCause;

    fn reset_cause() -> Self::Cause {
        #[cfg(target_arch = "arm")]
        unsafe {
            NrfResetCause(read32(0x4000_0400))
        }
        #[cfg(not(target_arch = "arm"))]
        NrfResetCause(0)
    }

    fn system_reset() -> ! {
        #[cfg(target_arch = "arm")]
        cortex_m::peripheral::SCB::sys_reset();
        #[cfg(not(target_arch = "arm"))]
        panic!("nRF reset is unavailable on the host")
    }
}

/// System-ON CPU idle only. This provider never enters SYSTEMOFF implicitly.
pub struct NrfCpuPower;

impl HalPower for NrfCpuPower {
    type Error = core::convert::Infallible;

    fn idle(&mut self, mode: IdleMode) -> Result<(), Self::Error> {
        match mode {
            IdleMode::CpuSleep => {
                #[cfg(target_arch = "arm")]
                {
                    cortex_m::asm::dsb();
                    cortex_m::asm::wfi();
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arduino_pin_map_is_exact_and_rejects_unexposed_pins() {
        assert_eq!(absolute_pin(0), Some(6));
        assert_eq!(absolute_pin(6), Some(32));
        assert_eq!(absolute_pin(15), Some(2));
        assert_eq!(absolute_pin(21), Some(15));
        assert_eq!(absolute_pin(22), None);
    }

    #[test]
    fn storage_region_is_bounded_below_both_bootloaders() {
        assert_eq!(NrfNvmc::PAGE_COUNT, 4);
        assert_eq!(NrfNvmc::word_address(0, 0), Some(APP_STORAGE_START));
        assert_eq!(NrfNvmc::word_address(3, 1023), Some(APP_STORAGE_END - 4));
        assert_eq!(NrfNvmc::word_address(4, 0), None);
        assert_eq!(NrfNvmc::word_address(0, 1024), None);
        assert_eq!(APP_STORAGE_END, 0xE9000);
    }

    #[test]
    fn watchdog_timeout_rejects_non_hardware_values() {
        assert!(!NrfWatchdog::valid_timeout_ticks(0));
        assert!(!NrfWatchdog::valid_timeout_ticks(14));
        assert!(NrfWatchdog::valid_timeout_ticks(15));
    }
}
