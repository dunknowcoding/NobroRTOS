//! Reusable RA4M1 providers used by the native Rust port.
//!
//! The clock uses the Cortex-M4 DWT counter after the board clock is configured at
//! 48 MHz. Callers must sample it at least once per 32-bit counter wrap (about 89.48 s).
//! It measures active core-clock time: clock-stopping sleep/deep-standby and debugger
//! halts are not promised to advance it. Reinitialize or reconcile it against an always-on
//! source after such a transition. The deadline provider owns SysTick and composes
//! long deadlines from bounded 24-bit chunks; callers do not inherit the raw SysTick
//! reload ceiling. ADC, PWM, I2C, and SPI on Arduino UNO R4 are exposed through
//! `NobroArduinoProviders.h`, which delegates to the installed Arduino Renesas core
//! instead of duplicating FSP.

use core::cell::UnsafeCell;

use cortex_m::peripheral::{DCB, DWT, SYST};
use nobro_hal::{
    CapabilityProfileKind, HalAlarm, HalByteIo, HalClock, HalCompatibility, HardwareCapability,
    HardwareCapabilityDeclaration, HardwareCapabilitySet, HardwareCapabilityWitness,
};
use nobro_usb::{CdcState, MountedUsb, Stage, UsbIoError, UsbStack, RA4M1_USB_CONFIG};

const CORE_CLOCK_HZ: u64 = 48_000_000;
const CYCLES_PER_US: u64 = CORE_CLOCK_HZ / 1_000_000;
const SYST_MAX_RELOAD: u32 = 0x00ff_ffff;

pub struct Ra4m1Providers;

impl HardwareCapabilityWitness<{ HardwareCapability::Timebase as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Deadline as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::DmaCompletion as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Usb as u8 }> for Ra4m1Providers {}

impl HalCompatibility for Ra4m1Providers {
    const DECLARATION: HardwareCapabilityDeclaration = {
        let witnesses = HardwareCapabilitySet::EMPTY
            .witnessed::<Self, { HardwareCapability::Timebase as u8 }>(
                HardwareCapability::Timebase,
            )
            .witnessed::<Self, { HardwareCapability::Deadline as u8 }>(
                HardwareCapability::Deadline,
            )
            .witnessed::<Self, { HardwareCapability::DmaCompletion as u8 }>(
                HardwareCapability::DmaCompletion,
            )
            .witnessed::<Self, { HardwareCapability::Usb as u8 }>(HardwareCapability::Usb);
        let supported = witnesses;
        HardwareCapabilityDeclaration::new(
            "provider-ra4m1-v2",
            CapabilityProfileKind::Constrained,
            supported,
            supported,
            HardwareCapabilitySet::EMPTY,
            HardwareCapabilitySet::ALL.without(supported),
            witnesses,
        )
    };
}

const _: [(); 1] =
    [(); <Ra4m1Providers as HalCompatibility>::DECLARATION.is_valid() as usize];
const _: [(); 1] =
    [(); <Ra4m1Providers as HalCompatibility>::DECLARATION.is_exact_profile() as usize];

#[derive(Clone, Copy)]
struct ClockState {
    last_cycles: u32,
    total_cycles: u64,
    initialized: bool,
}

impl ClockState {
    const fn uninitialized() -> Self {
        Self {
            last_cycles: 0,
            total_cycles: 0,
            initialized: false,
        }
    }

    const fn started_at(current: u32) -> Self {
        Self {
            last_cycles: current,
            total_cycles: 0,
            initialized: true,
        }
    }

    /// Extend one observed 32-bit sample into accumulated cycles.
    ///
    /// This is deliberately pure with respect to hardware: the caller owns
    /// synchronization and supplies the DWT sample. `wrapping_sub` accounts for one
    /// wrap, while the documented sampling contract rules out two unseen wraps.
    fn observe(&mut self, current: u32) -> Option<u64> {
        if !self.initialized {
            return None;
        }
        self.total_cycles = self
            .total_cycles
            .saturating_add(u64::from(current.wrapping_sub(self.last_cycles)));
        self.last_cycles = current;
        Some(self.total_cycles)
    }
}

struct ClockStorage(UnsafeCell<ClockState>);

// SAFETY: every access is serialized by `critical_section::with`.
unsafe impl Sync for ClockStorage {}

static CLOCK: ClockStorage = ClockStorage(UnsafeCell::new(ClockState::uninitialized()));

pub struct Ra4m1Clock;

impl Ra4m1Clock {
    /// Approximate interval, in microseconds, before the 32-bit 48 MHz counter wraps.
    /// Callers must observe the clock strictly sooner than this interval.
    pub const COUNTER_WRAP_US: u64 = (1_u64 << 32) / CYCLES_PER_US;

    /// Whether this provider promises progress while the core clock is stopped.
    pub const ADVANCES_WHEN_CORE_CLOCK_STOPPED: bool = false;

    /// Start the free-running DWT cycle counter after the RA4M1 clock reaches 48 MHz.
    ///
    /// Call [`HalClock::now_us`] at least once every 89.48 seconds while the core clock
    /// runs. The 32-bit counter cannot reveal multiple wraps between observations.
    /// DWT is not an always-on wall clock: reinitialize or externally reconcile elapsed
    /// time after any mode that stops the core clock, including deep standby.
    pub fn init(dcb: &mut DCB, dwt: &mut DWT) {
        critical_section::with(|_| {
            dcb.enable_trace();
            dwt.set_cycle_count(0);
            dwt.enable_cycle_counter();
            let current = DWT::cycle_count();
            // SAFETY: the critical-section token serializes the only mutable access.
            let state = unsafe { &mut *CLOCK.0.get() };
            *state = ClockState::started_at(current);
        });
    }
}

impl HalClock for Ra4m1Clock {
    fn now_us() -> u64 {
        critical_section::with(|_| {
            // Read CYCCNT while interrupts are masked so the sample and the state update
            // form one observation. Otherwise an interrupt-level caller could advance the
            // extension state between the sample and update, making the older sample look
            // like a nearly complete counter wrap.
            let current = DWT::cycle_count();
            // SAFETY: the critical-section token serializes the only mutable access.
            let state = unsafe { &mut *CLOCK.0.get() };
            state.observe(current).unwrap_or(0) / CYCLES_PER_US
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlarmError {
    ZeroDelay,
    DelayTooLong,
}

fn systick_reload(delay_us: u64) -> Result<u32, AlarmError> {
    if delay_us == 0 {
        return Err(AlarmError::ZeroDelay);
    }
    let cycles = delay_us
        .checked_mul(CYCLES_PER_US)
        .ok_or(AlarmError::DelayTooLong)?;
    let reload = cycles.checked_sub(1).ok_or(AlarmError::ZeroDelay)?;
    if reload > u64::from(SYST_MAX_RELOAD) {
        return Err(AlarmError::DelayTooLong);
    }
    Ok(reload as u32)
}

fn take_alarm_chunk(remaining_us: u64) -> Result<(u64, u64), AlarmError> {
    if remaining_us == 0 {
        return Err(AlarmError::ZeroDelay);
    }
    let chunk = remaining_us.min(Ra4m1Alarm::MAX_CHUNK_US);
    Ok((chunk, remaining_us - chunk))
}

pub struct Ra4m1Alarm {
    systick: SYST,
    deadline_us: Option<u64>,
    remaining_us: u64,
}

impl Ra4m1Alarm {
    /// Largest whole-microsecond hardware chunk representable by 24-bit SysTick.
    pub const MAX_CHUNK_US: u64 = (SYST_MAX_RELOAD as u64 + 1) / CYCLES_PER_US;

    /// Hardware reload ceiling exposed for admission checks and diagnostics.
    pub const MAX_RELOAD: u32 = SYST_MAX_RELOAD;

    pub fn new(mut systick: SYST) -> Self {
        systick.disable_counter();
        systick.disable_interrupt();
        systick.set_clock_source(cortex_m::peripheral::syst::SystClkSource::Core);
        Self {
            systick,
            deadline_us: None,
            remaining_us: 0,
        }
    }

    fn arm_next_chunk(&mut self) -> Result<(), AlarmError> {
        let (chunk, remaining) = take_alarm_chunk(self.remaining_us)?;
        let reload = systick_reload(chunk)?;
        self.remaining_us = remaining;
        self.systick.disable_counter();
        self.systick.set_reload(reload);
        self.systick.clear_current();
        self.systick.enable_counter();
        Ok(())
    }
}

impl HalAlarm for Ra4m1Alarm {
    type Error = AlarmError;

    fn arm_after_us(&mut self, delay_us: u64) -> Result<u64, Self::Error> {
        if delay_us == 0 {
            return Err(AlarmError::ZeroDelay);
        }
        let deadline = Ra4m1Clock::now_us()
            .checked_add(delay_us)
            .ok_or(AlarmError::DelayTooLong)?;
        self.deadline_us = Some(deadline);
        self.remaining_us = delay_us;
        self.arm_next_chunk()?;
        Ok(deadline)
    }

    fn cancel(&mut self) {
        self.systick.disable_counter();
        self.systick.clear_current();
        self.deadline_us = None;
        self.remaining_us = 0;
    }

    fn deadline_us(&self) -> Option<u64> {
        self.deadline_us
    }

    fn poll_due(&mut self, now_us: u64) -> bool {
        if self.deadline_us.is_none() {
            return false;
        }
        if self.deadline_us.is_some_and(|deadline| now_us >= deadline) {
            self.cancel();
            true
        } else if self.systick.has_wrapped() {
            if self.remaining_us == 0 {
                self.cancel();
                true
            } else {
                // The remaining duration was validated before the first chunk. A later
                // chunk is at most MAX_CHUNK_US and therefore cannot fail.
                let _ = self.arm_next_chunk();
                false
            }
        } else {
            false
        }
    }
}

/// RA4M1 byte provider over the backend selected and exclusively owned by `nobro_usb`.
pub struct Ra4m1Usb(MountedUsb);

impl Ra4m1Usb {
    /// Mount the port's fixed flash-resident descriptor identity.
    ///
    /// This provider intentionally accepts no arbitrary `UsbConfig`: the raw-register
    /// backend cannot generate descriptors at runtime, so an input mismatch would only
    /// turn into a late target panic.
    pub fn try_mount() -> Result<Self, nobro_usb::UsbMountError> {
        nobro_usb::try_mount(&RA4M1_USB_CONFIG).map(Self)
    }

    /// Compatibility wrapper for firmware that deliberately treats mount failure as
    /// unrecoverable. Interactive firmware should use [`Self::try_mount`] and preserve
    /// its existing transport when the process-wide USB claim is unavailable.
    #[track_caller]
    pub fn mount() -> Self {
        match Self::try_mount() {
            Ok(usb) => usb,
            Err(error) => panic!("RA4M1 native USB mount failed: {error:?}"),
        }
    }

    pub fn poll(&mut self) {
        let _ = self.0.poll();
    }

    pub fn configured(&self) -> bool {
        self.0.state() == CdcState::Configured
    }

    pub fn stage(&self) -> Stage {
        self.0.stage()
    }

    /// Force the native controller to drop D+ while the board USB mux is restored to
    /// its upload-visible bridge route.
    pub fn disconnect_link(&mut self) {
        self.0.disconnect_link();
    }

    /// Re-arm the existing controller instance after the board mux is routed to RA4M1.
    pub fn reconnect_link(&mut self) {
        self.0.reconnect_link();
    }
}

impl HalByteIo for Ra4m1Usb {
    type Error = UsbIoError;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        self.0.read_available(bytes)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.write_all(bytes)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.0.flush_pending()
    }
}

#[cfg(test)]
mod tests {
    use super::{systick_reload, take_alarm_chunk, AlarmError, ClockState, Ra4m1Alarm};

    #[test]
    fn clock_state_extends_normal_observations() {
        let mut state = ClockState::started_at(100);
        assert_eq!(state.observe(148), Some(48));
        assert_eq!(state.observe(196), Some(96));
    }

    #[test]
    fn clock_state_extends_one_counter_wrap() {
        let mut state = ClockState::started_at(0xffff_fff0);
        assert_eq!(state.observe(0x0000_0020), Some(48));
        assert_eq!(state.observe(0x0000_0050), Some(96));
    }

    #[test]
    fn uninitialized_clock_does_not_invent_elapsed_time() {
        let mut state = ClockState::uninitialized();
        assert_eq!(state.observe(123), None);
        assert_eq!(state.last_cycles, 0);
        assert_eq!(state.total_cycles, 0);
    }

    #[test]
    fn alarm_reload_accepts_exact_hardware_boundaries() {
        assert_eq!(systick_reload(0), Err(AlarmError::ZeroDelay));
        assert_eq!(systick_reload(1), Ok(47));
        assert_eq!(
            systick_reload(Ra4m1Alarm::MAX_CHUNK_US),
            Ok((Ra4m1Alarm::MAX_CHUNK_US * 48 - 1) as u32)
        );
        assert!(systick_reload(Ra4m1Alarm::MAX_CHUNK_US).unwrap() <= Ra4m1Alarm::MAX_RELOAD);
    }

    #[test]
    fn alarm_reload_rejects_first_unrepresentable_and_overflowing_delay() {
        assert_eq!(
            systick_reload(Ra4m1Alarm::MAX_CHUNK_US + 1),
            Err(AlarmError::DelayTooLong)
        );
        assert_eq!(systick_reload(u64::MAX), Err(AlarmError::DelayTooLong));
    }

    #[test]
    fn long_alarm_is_partitioned_without_dropping_or_extending_time() {
        let total = Ra4m1Alarm::MAX_CHUNK_US * 3 + 17;
        let mut remaining = total;
        let mut accumulated = 0;
        let mut chunks = 0;
        while remaining != 0 {
            let (chunk, next) = take_alarm_chunk(remaining).unwrap();
            assert!((1..=Ra4m1Alarm::MAX_CHUNK_US).contains(&chunk));
            accumulated += chunk;
            remaining = next;
            chunks += 1;
        }
        assert_eq!(accumulated, total);
        assert_eq!(chunks, 4);
        assert_eq!(take_alarm_chunk(0), Err(AlarmError::ZeroDelay));
    }
}
