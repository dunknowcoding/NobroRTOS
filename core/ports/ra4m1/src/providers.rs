//! Reusable RA4M1 providers used by the native Rust port.
//!
//! AGT0 is a LOCO-clocked monotonic source and AGT1 is a chained one-shot alarm.
//! Both continue through ordinary Cortex-M `WFI` CPU sleep. Deep software standby
//! resets or stops composition-owned state and is intentionally not claimed here.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use nobro_hal::{
    CapabilityProfileKind, HalAlarm, HalByteIo, HalClock, HalCompatibility, HalLease,
    HardwareCapability, HardwareCapabilityDeclaration, HardwareCapabilitySet,
    HardwareCapabilityWitness,
};
use nobro_usb::{CdcState, MountedUsb, Stage, UsbIoError, UsbStack, RA4M1_USB_CONFIG};

use crate::lease::{Ra4m1LeaseGuard, Ra4m1Leases};
use crate::power_reset::{PowerVeto, Ra4m1Power};
use nobro_hal::LeaseId;

const LOCO_HZ: u64 = 32_768;
const AGT_PERIOD_TICKS: u64 = 65_536;
#[cfg(target_arch = "arm")]
const AGT0: usize = 0x4008_4000;
#[cfg(target_arch = "arm")]
const AGT1: usize = 0x4008_4100;
#[cfg(target_arch = "arm")]
const AGT_COUNT: usize = 0x00;
#[cfg(target_arch = "arm")]
const AGTCR: usize = 0x08;
#[cfg(target_arch = "arm")]
const AGTMR1: usize = 0x09;
#[cfg(target_arch = "arm")]
const AGTMR2: usize = 0x0a;
#[cfg(target_arch = "arm")]
const AGTIOC: usize = 0x0c;
#[cfg(target_arch = "arm")]
const ICU_IELSR_BASE: usize = 0x4000_6300;
#[cfg(target_arch = "arm")]
const IELSR_IR: u32 = 1 << 16;
#[cfg(target_arch = "arm")]
const ELC_EVENT_AGT0_INT: u32 = 30;
#[cfg(target_arch = "arm")]
const ELC_EVENT_AGT1_INT: u32 = 33;
pub const RA4M1_CLOCK_IRQ: usize = 28;
pub const RA4M1_ALARM_IRQ: usize = 29;
#[cfg(target_arch = "arm")]
const NVIC_ISER: usize = 0xE000_E100;
#[cfg(target_arch = "arm")]
const NVIC_ICER: usize = 0xE000_E180;
#[cfg(target_arch = "arm")]
const NVIC_ICPR: usize = 0xE000_E280;
#[cfg(target_arch = "arm")]
const NVIC_IPR: usize = 0xE000_E400;
#[cfg(target_arch = "arm")]
const MSTPCRD: usize = 0x4004_7008;
#[cfg(target_arch = "arm")]
const PRCR: usize = 0x4001_E3FE;
#[cfg(target_arch = "arm")]
const AGT0_MSTP: u32 = 1 << 3;
#[cfg(target_arch = "arm")]
const AGT1_MSTP: u32 = 1 << 2;
#[cfg(target_arch = "arm")]
const AGT_TUNDF: u8 = 1 << 5;
#[cfg(target_arch = "arm")]
const AGT_RUNNING: u8 = 1 << 1;
#[cfg(target_arch = "arm")]
const AGT_FORCE_STOP: u8 = 0xf4;
#[cfg(target_arch = "arm")]
const AGT_START: u8 = 0xf1;
#[cfg(target_arch = "arm")]
const AGT_LOCO_TIMER_MODE: u8 = 0x41;
const CLOCK_OWNER: u8 = 0xf0;
const ALARM_OWNER: u8 = 0xf1;

pub struct Ra4m1Providers;

impl HardwareCapabilityWitness<{ HardwareCapability::Timebase as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Deadline as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Event as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::DmaCompletion as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Uart as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::ByteIo as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Adc as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Pwm as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::I2c as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Spi as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Usb as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Reset as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Power as u8 }> for Ra4m1Providers {}
impl HardwareCapabilityWitness<{ HardwareCapability::Lease as u8 }> for Ra4m1Providers {}

impl HalCompatibility for Ra4m1Providers {
    const DECLARATION: HardwareCapabilityDeclaration = {
        let witnesses = HardwareCapabilitySet::EMPTY
            .witnessed::<Self, { HardwareCapability::Timebase as u8 }>(HardwareCapability::Timebase)
            .witnessed::<Self, { HardwareCapability::Deadline as u8 }>(HardwareCapability::Deadline)
            .witnessed::<Self, { HardwareCapability::Event as u8 }>(HardwareCapability::Event)
            .witnessed::<Self, { HardwareCapability::DmaCompletion as u8 }>(
                HardwareCapability::DmaCompletion,
            )
            .witnessed::<Self, { HardwareCapability::Uart as u8 }>(HardwareCapability::Uart)
            .witnessed::<Self, { HardwareCapability::ByteIo as u8 }>(HardwareCapability::ByteIo)
            .witnessed::<Self, { HardwareCapability::Adc as u8 }>(HardwareCapability::Adc)
            .witnessed::<Self, { HardwareCapability::Pwm as u8 }>(HardwareCapability::Pwm)
            .witnessed::<Self, { HardwareCapability::I2c as u8 }>(HardwareCapability::I2c)
            .witnessed::<Self, { HardwareCapability::Spi as u8 }>(HardwareCapability::Spi)
            .witnessed::<Self, { HardwareCapability::Usb as u8 }>(HardwareCapability::Usb)
            .witnessed::<Self, { HardwareCapability::Reset as u8 }>(HardwareCapability::Reset)
            .witnessed::<Self, { HardwareCapability::Power as u8 }>(HardwareCapability::Power)
            .witnessed::<Self, { HardwareCapability::Lease as u8 }>(HardwareCapability::Lease);
        let supported = witnesses;
        let inapplicable = HardwareCapabilitySet::EMPTY
            .with(HardwareCapability::Servo)
            .with(HardwareCapability::Cache)
            .with(HardwareCapability::Multicore);
        HardwareCapabilityDeclaration::new(
            "ra4m1-native-partial-v3",
            CapabilityProfileKind::Constrained,
            supported,
            supported,
            inapplicable,
            HardwareCapabilitySet::ALL
                .without(supported)
                .without(inapplicable),
            witnesses,
        )
    };
}

const _: [(); 1] = [(); <Ra4m1Providers as HalCompatibility>::DECLARATION.is_valid() as usize];
const _: [(); 1] =
    [(); <Ra4m1Providers as HalCompatibility>::DECLARATION.is_exact_profile() as usize];

#[derive(Clone, Copy)]
struct ClockState {
    epochs: u64,
    initialized: bool,
}

impl ClockState {
    const fn uninitialized() -> Self {
        Self {
            epochs: 0,
            initialized: false,
        }
    }

    const fn started() -> Self {
        Self {
            epochs: 0,
            initialized: true,
        }
    }

    fn observe(&self, down_counter: u16) -> Option<u64> {
        if !self.initialized {
            return None;
        }
        Some(
            self.epochs
                .saturating_mul(AGT_PERIOD_TICKS)
                .saturating_add(u64::from(u16::MAX - down_counter)),
        )
    }

    fn underflow(&mut self) {
        if self.initialized {
            self.epochs = self.epochs.saturating_add(1);
        }
    }
}

struct ClockStorage(UnsafeCell<ClockState>);

// SAFETY: every access is serialized by `critical_section::with`.
unsafe impl Sync for ClockStorage {}

static CLOCK: ClockStorage = ClockStorage(UnsafeCell::new(ClockState::uninitialized()));

pub struct Ra4m1Clock;

impl Ra4m1Clock {
    pub const TICK_HZ: u64 = LOCO_HZ;
    pub const ADVANCES_IN_CPU_SLEEP: bool = true;
    pub const ADVANCES_IN_DEEP_STANDBY: bool = false;

    /// Claim AGT0 and start a LOCO-clocked free-running timebase.
    pub fn try_init() -> Result<(), nobro_hal::LeaseError> {
        Ra4m1Leases::acquire(LeaseId::SYSTEM_TIMER, CLOCK_OWNER)?;
        critical_section::with(|_| {
            // SAFETY: the critical-section token serializes the only mutable access.
            let state = unsafe { &mut *CLOCK.0.get() };
            *state = ClockState::started();
        });
        #[cfg(target_arch = "arm")]
        unsafe {
            start_module(AGT0_MSTP);
            initialize_agt(AGT0, RA4M1_CLOCK_IRQ, ELC_EVENT_AGT0_INT, u16::MAX);
        }
        Ok(())
    }

    #[track_caller]
    pub fn init() {
        if let Err(error) = Self::try_init() {
            panic!("RA4M1 AGT0 timebase lease failed: {error:?}");
        }
    }

    fn ticks() -> u64 {
        critical_section::with(|_| {
            #[cfg(target_arch = "arm")]
            unsafe {
                service_clock_underflow();
            }
            #[cfg(target_arch = "arm")]
            let counter = unsafe { read16(AGT0 + AGT_COUNT) };
            #[cfg(not(target_arch = "arm"))]
            let counter = u16::MAX;
            // SAFETY: the critical-section token serializes state and the AGT sample.
            unsafe { (&*CLOCK.0.get()).observe(counter).unwrap_or(0) }
        })
    }
}

impl HalClock for Ra4m1Clock {
    fn now_us() -> u64 {
        Self::ticks().saturating_mul(1_000_000) / LOCO_HZ
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlarmError {
    ZeroDelay,
    DelayTooLong,
    Lease(nobro_hal::LeaseError),
}

fn delay_to_ticks(delay_us: u64) -> Result<u64, AlarmError> {
    if delay_us == 0 {
        return Err(AlarmError::ZeroDelay);
    }
    let scaled = delay_us
        .checked_mul(LOCO_HZ)
        .ok_or(AlarmError::DelayTooLong)?;
    Ok(scaled.saturating_add(999_999) / 1_000_000)
}

fn take_alarm_chunk(remaining_ticks: u64) -> Result<(u16, u64), AlarmError> {
    if remaining_ticks == 0 {
        return Err(AlarmError::ZeroDelay);
    }
    let ticks = remaining_ticks.min(AGT_PERIOD_TICKS);
    Ok(((ticks - 1) as u16, remaining_ticks - ticks))
}

pub struct Ra4m1Alarm {
    _lease: Ra4m1LeaseGuard,
    deadline_us: Option<u64>,
    remaining_ticks: u64,
}

static ALARM_FIRED: AtomicBool = AtomicBool::new(false);

impl Ra4m1Alarm {
    pub const MAX_CHUNK_TICKS: u64 = AGT_PERIOD_TICKS;
    pub const MAX_CHUNK_US: u64 = AGT_PERIOD_TICKS * 1_000_000 / LOCO_HZ;

    pub fn try_new(owner: u8) -> Result<Self, AlarmError> {
        let lease = Ra4m1Leases::acquire_guard(LeaseId::DEADLINE_TIMER, owner)
            .map_err(AlarmError::Lease)?;
        #[cfg(target_arch = "arm")]
        unsafe {
            start_module(AGT1_MSTP);
            stop_agt(AGT1);
            configure_irq(RA4M1_ALARM_IRQ, ELC_EVENT_AGT1_INT);
        }
        Ok(Self {
            _lease: lease,
            deadline_us: None,
            remaining_ticks: 0,
        })
    }

    #[track_caller]
    pub fn new() -> Self {
        match Self::try_new(ALARM_OWNER) {
            Ok(alarm) => alarm,
            Err(error) => panic!("RA4M1 AGT1 alarm initialization failed: {error:?}"),
        }
    }

    fn arm_next_chunk(&mut self) -> Result<(), AlarmError> {
        let (_reload, remaining) = take_alarm_chunk(self.remaining_ticks)?;
        self.remaining_ticks = remaining;
        ALARM_FIRED.store(false, Ordering::Release);
        #[cfg(target_arch = "arm")]
        unsafe {
            initialize_agt(AGT1, RA4M1_ALARM_IRQ, ELC_EVENT_AGT1_INT, _reload);
        }
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
        let ticks = delay_to_ticks(delay_us)?;
        self.deadline_us = Some(deadline);
        self.remaining_ticks = ticks;
        self.arm_next_chunk()?;
        Ok(deadline)
    }

    fn cancel(&mut self) {
        #[cfg(target_arch = "arm")]
        unsafe {
            stop_agt(AGT1);
            clear_irq(RA4M1_ALARM_IRQ);
        }
        ALARM_FIRED.store(false, Ordering::Release);
        self.deadline_us = None;
        self.remaining_ticks = 0;
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
        } else if ALARM_FIRED.swap(false, Ordering::AcqRel) {
            if self.remaining_ticks == 0 {
                self.cancel();
                true
            } else {
                let _ = self.arm_next_chunk();
                false
            }
        } else {
            false
        }
    }
}

impl Drop for Ra4m1Alarm {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(target_arch = "arm")]
unsafe fn ielsr(irq: usize) -> usize {
    ICU_IELSR_BASE + irq * 4
}

#[cfg(target_arch = "arm")]
unsafe fn start_module(mask: u32) {
    let prior = read16(PRCR) & 0x0003;
    write16(PRCR, 0xa502);
    write32(MSTPCRD, read32(MSTPCRD) & !mask);
    write16(PRCR, 0xa500 | prior);
}

#[cfg(target_arch = "arm")]
unsafe fn configure_irq(irq: usize, event: u32) {
    let mask = 1u32 << irq;
    write32(NVIC_ICER, mask);
    write32(NVIC_ICPR, mask);
    write32(ielsr(irq), event);
    write8(NVIC_IPR + irq, 0xc0);
    write32(NVIC_ISER, mask);
}

#[cfg(target_arch = "arm")]
unsafe fn clear_irq(irq: usize) {
    write32(ielsr(irq), read32(ielsr(irq)) & !IELSR_IR);
    write32(NVIC_ICPR, 1u32 << irq);
}

#[cfg(target_arch = "arm")]
unsafe fn stop_agt(base: usize) {
    write8(base + AGTCR, AGT_FORCE_STOP);
    for _ in 0..65_536 {
        if read8(base + AGTCR) & AGT_RUNNING == 0 {
            break;
        }
        core::hint::spin_loop();
    }
}

#[cfg(target_arch = "arm")]
unsafe fn initialize_agt(base: usize, irq: usize, event: u32, reload: u16) {
    stop_agt(base);
    write8(base + AGTMR2, 0);
    write8(base + AGTMR1, AGT_LOCO_TIMER_MODE);
    write8(base + AGTMR2, 0);
    write8(base + AGTIOC, 0);
    write16(base + AGT_COUNT, reload);
    write8(base + AGTCR, 0);
    configure_irq(irq, event);
    write8(base + AGTCR, AGT_START);
}

#[cfg(target_arch = "arm")]
unsafe fn service_clock_underflow() {
    if read8(AGT0 + AGTCR) & AGT_TUNDF != 0 {
        let state = &mut *CLOCK.0.get();
        state.underflow();
        let value = read8(AGT0 + AGTCR) & !0xf0;
        write8(AGT0 + AGTCR, value);
        clear_irq(RA4M1_CLOCK_IRQ);
    }
}

#[cfg(target_arch = "arm")]
pub unsafe extern "C" fn ra4m1_clock_irq() {
    critical_section::with(|_| service_clock_underflow());
}

#[cfg(target_arch = "arm")]
pub unsafe extern "C" fn ra4m1_alarm_irq() {
    stop_agt(AGT1);
    let value = read8(AGT1 + AGTCR) & !0xf0;
    write8(AGT1 + AGTCR, value);
    clear_irq(RA4M1_ALARM_IRQ);
    ALARM_FIRED.store(true, Ordering::Release);
}

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
unsafe fn read32(address: usize) -> u32 {
    (address as *const u32).read_volatile()
}

#[cfg(target_arch = "arm")]
unsafe fn write32(address: usize, value: u32) {
    (address as *mut u32).write_volatile(value);
}

/// RA4M1 byte provider over the backend selected and exclusively owned by `nobro_usb`.
pub struct Ra4m1Usb {
    mounted: MountedUsb,
    _power_veto: PowerVeto,
}

impl Ra4m1Usb {
    /// Mount the port's fixed flash-resident descriptor identity.
    ///
    /// This provider intentionally accepts no arbitrary `UsbConfig`: the raw-register
    /// backend cannot generate descriptors at runtime, so an input mismatch would only
    /// turn into a late target panic.
    pub fn try_mount() -> Result<Self, nobro_usb::UsbMountError> {
        let mounted = nobro_usb::try_mount(&RA4M1_USB_CONFIG)?;
        let power_veto = match Ra4m1Power::veto(0) {
            Ok(veto) => veto,
            // Reason bit zero is statically within the admitted 32-bit mask.
            Err(_) => unreachable!(),
        };
        Ok(Self {
            mounted,
            _power_veto: power_veto,
        })
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
        let _ = self.mounted.poll();
    }

    pub fn configured(&self) -> bool {
        self.mounted.state() == CdcState::Configured
    }

    pub fn stage(&self) -> Stage {
        self.mounted.stage()
    }

    /// Force the native controller to drop D+ while the board USB mux is restored to
    /// its upload-visible bridge route.
    pub fn disconnect_link(&mut self) {
        self.mounted.disconnect_link();
    }

    /// Re-arm the existing controller instance after the board mux is routed to RA4M1.
    pub fn reconnect_link(&mut self) {
        self.mounted.reconnect_link();
    }
}

impl HalByteIo for Ra4m1Usb {
    type Error = UsbIoError;

    fn read_available(&mut self, bytes: &mut [u8]) -> Result<usize, Self::Error> {
        self.mounted.read_available(bytes)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.mounted.write_all(bytes)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.mounted.flush_pending()
    }
}

#[cfg(test)]
mod tests {
    use super::{delay_to_ticks, take_alarm_chunk, AlarmError, ClockState, AGT_PERIOD_TICKS};

    #[test]
    fn clock_state_extends_down_counter_observations() {
        let state = ClockState::started();
        assert_eq!(state.observe(u16::MAX), Some(0));
        assert_eq!(state.observe(u16::MAX - 48), Some(48));
    }

    #[test]
    fn clock_state_extends_arbitrary_interrupt_serviced_wraps() {
        let mut state = ClockState::started();
        for _ in 0..3 {
            state.underflow();
        }
        assert_eq!(state.observe(u16::MAX - 7), Some(3 * AGT_PERIOD_TICKS + 7));
    }

    #[test]
    fn uninitialized_clock_does_not_invent_elapsed_time() {
        let mut state = ClockState::uninitialized();
        assert_eq!(state.observe(123), None);
        state.underflow();
        assert_eq!(state.epochs, 0);
    }

    #[test]
    fn alarm_delay_rounds_up_without_arming_early() {
        assert_eq!(delay_to_ticks(0), Err(AlarmError::ZeroDelay));
        assert_eq!(delay_to_ticks(1), Ok(1));
        assert_eq!(delay_to_ticks(30), Ok(1));
        assert_eq!(delay_to_ticks(31), Ok(2));
    }

    #[test]
    fn alarm_delay_rejects_overflow() {
        assert_eq!(delay_to_ticks(u64::MAX), Err(AlarmError::DelayTooLong));
    }

    #[test]
    fn long_alarm_is_partitioned_without_dropping_or_extending_time() {
        let total = AGT_PERIOD_TICKS * 3 + 17;
        let mut remaining = total;
        let mut accumulated = 0;
        let mut chunks = 0;
        while remaining != 0 {
            let (reload, next) = take_alarm_chunk(remaining).unwrap();
            let chunk = u64::from(reload) + 1;
            assert!((1..=AGT_PERIOD_TICKS).contains(&chunk));
            accumulated += chunk;
            remaining = next;
            chunks += 1;
        }
        assert_eq!(accumulated, total);
        assert_eq!(chunks, 4);
        assert_eq!(take_alarm_chunk(0), Err(AlarmError::ZeroDelay));
    }
}
