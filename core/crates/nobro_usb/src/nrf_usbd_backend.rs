//! nRF52840 USB backend over the vendored `nrf-usbd` + `usbd-serial` CDC. Owns a
//! `'static` bus allocator so the `UsbDevice`/`SerialPort` can live inside the backend
//! struct.

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use cortex_m::peripheral::NVIC;
use nrf52840_pac::Interrupt;
use nrf_usbd::{
    request_bootloader_handoff, sanitize_handoff, HandoffSanitization, UsbPeripheral, Usbd,
    UsbdFault,
};
use usb_device::class_prelude::UsbClass;
use usb_device::device::{UsbDevice, UsbDeviceBuilder, UsbDeviceState, UsbVidPid};
use usb_device::{bus::UsbBusAllocator, device::StringDescriptors, UsbError};
use usbd_serial::SerialPort;

use crate::{backend_id, CdcState, UsbBackendError, UsbConfig, UsbStack};

struct Nrf52840Usbd;

const DRIVER_FAULT_NONE: u8 = 0;
const DRIVER_FAULT_ENABLE_TIMEOUT: u8 = 1;
const DRIVER_FAULT_HFCLK_TIMEOUT: u8 = 8;
const DRIVER_FAULT_STORAGE_CLAIMED: u8 = 2;
const DRIVER_FAULT_WAKE_TIMEOUT: u8 = 3;
const DRIVER_FAULT_POWER_READY_TIMEOUT: u8 = 4;
const DRIVER_FAULT_DISABLE_TIMEOUT: u8 = 5;
const DRIVER_FAULT_UNSUPPORTED_SILICON: u8 = 6;
const DRIVER_FAULT_PARITY_REPAIR_UNAVAILABLE: u8 = 7;
const DRIVER_FAULT_IN_BASE: u8 = 0x10;
const DRIVER_FAULT_OUT_BASE: u8 = 0x20;
const NRF_EP0_MAX_PACKET_SIZE: u8 = 64;
static DRIVER_FAULT: AtomicU8 = AtomicU8::new(DRIVER_FAULT_NONE);
static DRIVER_OPERATIONAL: AtomicBool = AtomicBool::new(false);

fn encode_driver_fault(fault: UsbdFault) -> u8 {
    match fault {
        UsbdFault::UnsupportedSilicon => DRIVER_FAULT_UNSUPPORTED_SILICON,
        UsbdFault::ParityRepairUnavailable => DRIVER_FAULT_PARITY_REPAIR_UNAVAILABLE,
        UsbdFault::EnableTimeout => DRIVER_FAULT_ENABLE_TIMEOUT,
        UsbdFault::HfclkTimeout => DRIVER_FAULT_HFCLK_TIMEOUT,
        UsbdFault::DmaStorageAlreadyClaimed => DRIVER_FAULT_STORAGE_CLAIMED,
        UsbdFault::WakeTimeout => DRIVER_FAULT_WAKE_TIMEOUT,
        UsbdFault::PowerReadyTimeout => DRIVER_FAULT_POWER_READY_TIMEOUT,
        UsbdFault::DisableTimeout => DRIVER_FAULT_DISABLE_TIMEOUT,
        UsbdFault::InDmaTimeout { endpoint } => DRIVER_FAULT_IN_BASE | (endpoint & 0x0f),
        UsbdFault::OutDmaTimeout { endpoint } => DRIVER_FAULT_OUT_BASE | (endpoint & 0x0f),
    }
}

fn decode_driver_fault(code: u8) -> Option<UsbBackendError> {
    match code {
        DRIVER_FAULT_NONE => None,
        DRIVER_FAULT_ENABLE_TIMEOUT => Some(UsbBackendError::StartupTimeout),
        DRIVER_FAULT_HFCLK_TIMEOUT => Some(UsbBackendError::StartupTimeout),
        DRIVER_FAULT_STORAGE_CLAIMED => Some(UsbBackendError::Unavailable),
        DRIVER_FAULT_WAKE_TIMEOUT => Some(UsbBackendError::ControllerTimeout),
        DRIVER_FAULT_POWER_READY_TIMEOUT => Some(UsbBackendError::StartupTimeout),
        DRIVER_FAULT_DISABLE_TIMEOUT => Some(UsbBackendError::ControllerTimeout),
        DRIVER_FAULT_UNSUPPORTED_SILICON => Some(UsbBackendError::Unsupported),
        DRIVER_FAULT_PARITY_REPAIR_UNAVAILABLE => Some(UsbBackendError::InvalidState),
        code if code & 0xf0 == DRIVER_FAULT_IN_BASE => Some(UsbBackendError::InTransferTimeout {
            endpoint: code & 0x0f,
        }),
        code if code & 0xf0 == DRIVER_FAULT_OUT_BASE => Some(UsbBackendError::OutTransferTimeout {
            endpoint: code & 0x0f,
        }),
        _ => Some(UsbBackendError::InvalidState),
    }
}

fn reported_driver_fault() -> Option<UsbBackendError> {
    decode_driver_fault(DRIVER_FAULT.load(Ordering::Acquire))
}

fn timer0_is_microsecond_timebase(mode_is_timer: bool, bitmode_is_32: bool, prescaler: u8) -> bool {
    mode_is_timer && bitmode_is_32 && prescaler == 4
}

// nrf-usbd applies the mandatory USB errata itself; it only needs the register base.
unsafe impl UsbPeripheral for Nrf52840Usbd {
    const REGISTERS: *const () = 0x4002_7000 as *const ();

    fn on_fault(fault: UsbdFault) {
        let _ = DRIVER_FAULT.compare_exchange(
            DRIVER_FAULT_NONE,
            encode_driver_fault(fault),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn on_operational_change(operational: bool) {
        DRIVER_OPERATIONAL.store(operational, Ordering::Release);
    }

    fn vbus_present() -> bool {
        unsafe { nrf_vbus_present() }
    }

    fn power_ready() -> bool {
        unsafe { nrf_usb_power_ready() }
    }

    fn request_hfclk() {
        // Request-only ownership is deliberate: this backend never issues
        // TASKS_HFCLKSTOP because radio/SoftDevice code may share HFXO. A future
        // SoftDevice-backed board implementation can replace this hook with its
        // clock-provider API without changing the USBD lifecycle.
        unsafe {
            let clock = &*nrf52840_pac::CLOCK::ptr();
            let status = clock.hfclkstat.read();
            if !(status.state().is_running() && status.src().is_xtal()) {
                clock.events_hfclkstarted.reset();
                clock.tasks_hfclkstart.write(|w| w.bits(1));
            }
        }
    }

    fn hfclk_running() -> bool {
        unsafe {
            let status = (&*nrf52840_pac::CLOCK::ptr()).hfclkstat.read();
            status.state().is_running() && status.src().is_xtal()
        }
    }

    fn monotonic_us_32() -> Option<u32> {
        // Nobro's nRF timebase reserves CC0 as its software-capture channel; CC1/CC2
        // are event timestamps and CC3 is the deadline/wake compare. Reuse CC0 here
        // while the driver's critical section excludes concurrent captures. Never
        // capture into CC3: doing so would overwrite an armed scheduler deadline.
        // If TIMER0 has not been started, the vendored driver also advances its
        // explicit poll-count fallback, so a frozen zero cannot suppress timeout.
        unsafe {
            let timer = &*nrf52840_pac::TIMER0::ptr();
            if !timer0_is_microsecond_timebase(
                timer.mode.read().mode().is_timer(),
                timer.bitmode.read().bitmode().is_32bit(),
                timer.prescaler.read().prescaler().bits(),
            ) {
                // Do not mutate CC0 or mislabel arbitrary timer ticks as microseconds
                // when an application has not installed Nobro's 1 MHz timebase.
                return None;
            }
            timer.tasks_capture[0].write(|w| w.bits(1));
            Some(timer.cc[0].read().bits())
        }
    }
}

type Bus = Usbd<Nrf52840Usbd>;

// The allocator must outlive the device + serial (which borrow it), so it lives in a
// static. A board mounts a single USB stack, so a single slot is sufficient.
static mut ALLOC: MaybeUninit<UsbBusAllocator<Bus>> = MaybeUninit::uninit();

struct MountClaim(AtomicBool);

impl MountClaim {
    const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    fn try_claim(&self) -> bool {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

static MOUNT_CLAIM: MountClaim = MountClaim::new();

const POWER: u32 = 0x4000_0000; // POWER shares the base region on nRF52
const USBREGSTATUS: u32 = POWER + 0x438;
const VBUS_DETECTED: u32 = 1;
const USB_OUTPUT_READY: u32 = 1 << 1;

// nRF52840 bootloader -> application USB startup contract:
//   1. observe physical VBUS;
//   2. disconnect, prove disabled, and issue a best-effort cleanup of the old session;
//   3. let nrf-usbd enable with revision-gated errata, then repeat the authoritative
//      session cleanup and release any inherited forced DP/DM drive after READY while
//      D+ is still detached;
//   4. pass the poll-driven OUTPUTRDY gate;
//   5. advertise the hardware's 64-byte EP0 before engaging the pull-up.
// Keep this sequence centralized here. The board hook requests HFXO and proves it is
// selected before ENABLE; READY then acknowledges the controller transition. Application
// code must not duplicate CLOCK tasks, and this backend never blindly stops a clock that
// radio or SoftDevice code may share. Engaging D+ before HFXO, READY, and OUTPUTRDY can
// race or corrupt signalling on the analogue link.
// Falling back to usb-device's 8-byte EP0 default also needlessly exercises the
// multi-packet device-descriptor path during a firmware-stage handoff.

unsafe fn rd(a: u32) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}

unsafe fn nrf_vbus_present() -> bool {
    rd(USBREGSTATUS) & VBUS_DETECTED != 0
}

fn usb_power_status_ready(status: u32) -> bool {
    status & (VBUS_DETECTED | USB_OUTPUT_READY) == (VBUS_DETECTED | USB_OUTPUT_READY)
}

unsafe fn nrf_usb_power_ready() -> bool {
    usb_power_status_ready(rd(USBREGSTATUS))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartAttempt {
    Ready,
    NoVbus,
    DisableTimeout,
    UnsupportedSilicon,
}

/// Bring up USBD from a clean state without waiting for a cable forever.
///
/// Raw registers avoid taking the PAC singleton and support both reset and bootloader
/// handoff. The caller samples VBUS again after this bounded sequence before publishing
/// any connected state.
unsafe fn peripheral_clean_start() -> StartAttempt {
    if !nrf_vbus_present() {
        return StartAttempt::NoVbus;
    }

    // This backend is polled. Mask and unpend a bootloader-owned USBD IRQ before the
    // PAC-derived transaction disconnects, proves disable completion, and issues a
    // best-effort post-disable cleanup. nrf-usbd repeats that complete register pass
    // after its next ENABLE/READY transition, when the session registers and NODRIVE
    // task are accessible, and before it engages D+.
    NVIC::mask(Interrupt::USBD);
    let sanitized = sanitize_handoff::<Nrf52840Usbd>();
    NVIC::unpend(Interrupt::USBD);
    match sanitized {
        HandoffSanitization::Complete => StartAttempt::Ready,
        HandoffSanitization::DisableTimeout => StartAttempt::DisableTimeout,
        HandoffSanitization::UnsupportedSilicon => StartAttempt::UnsupportedSilicon,
    }
}

/// CDC-ACM over nrf-usbd.
pub(crate) struct NrfUsbdCdc {
    cfg: UsbConfig,
    serial: Option<SerialPort<'static, Bus>>,
    dev: Option<UsbDevice<'static, Bus>>,
    state: CdcState,
    saw_vbus: bool,
    awaiting_fresh_reset: bool,
    fault: Option<UsbBackendError>,
}

impl NrfUsbdCdc {
    pub(crate) fn mount(cfg: &UsbConfig) -> Self {
        assert!(
            MOUNT_CLAIM.try_claim(),
            "the nRF USB backend can only be mounted once"
        );
        // Device construction eventually enables nrf-usbd and waits for controller
        // readiness. Defer it to the first VBUS-observed poll so mount is cable-independent.
        NrfUsbdCdc {
            cfg: *cfg,
            serial: None,
            dev: None,
            state: CdcState::Disconnected,
            saw_vbus: false,
            awaiting_fresh_reset: false,
            fault: None,
        }
    }

    #[inline(never)]
    unsafe fn initialize(&mut self) -> StartAttempt {
        let start = peripheral_clean_start();
        if start != StartAttempt::Ready {
            self.fault = match start {
                StartAttempt::DisableTimeout => Some(UsbBackendError::ControllerTimeout),
                StartAttempt::UnsupportedSilicon => Some(UsbBackendError::Unsupported),
                StartAttempt::NoVbus | StartAttempt::Ready => None,
            };
            return start;
        }

        let cfg = self.cfg;
        unsafe {
            core::ptr::addr_of_mut!(ALLOC).write(MaybeUninit::new(UsbBusAllocator::new(
                Usbd::new(Nrf52840Usbd),
            )));
            let alloc: &'static UsbBusAllocator<Bus> = &*(*core::ptr::addr_of!(ALLOC)).as_ptr();
            // Publish the class before constructing the larger device object so
            // both capacity-sized values never coexist in the startup frame.
            self.serial = Some(SerialPort::new(alloc));
            let strings = StringDescriptors::default()
                .manufacturer(cfg.manufacturer)
                .product(cfg.product)
                .serial_number(cfg.serial);
            self.dev = Some(
                UsbDeviceBuilder::new(alloc, UsbVidPid(cfg.vid, cfg.pid))
                    .strings(&[strings])
                    .unwrap()
                    .device_class(usbd_serial::USB_CLASS_CDC)
                    // nRF52840 implements a 64-byte control endpoint. The 8-byte
                    // usb-device default splits the 18-byte device descriptor across the
                    // driver's multi-packet EP0 path and makes firmware-stage handoff more
                    // fragile than the controller-native packet size.
                    // Do not remove this just because 8 is USB-spec legal: this is a
                    // controller/driver integration constraint, not a descriptor preference.
                    .max_packet_size_0(NRF_EP0_MAX_PACKET_SIZE)
                    .unwrap()
                    .build(),
            );
        }
        self.fault = reported_driver_fault();
        StartAttempt::Ready
    }

    fn observe_disconnect(&mut self) -> CdcState {
        // Drive one non-blocking lifecycle observation on every unplugged poll. This
        // drops the pull-up immediately, requests disable, and eventually releases the
        // revision-specific active workaround only after ENABLE reads back Disabled.
        if let Some(dev) = self.dev.as_mut() {
            let _ = dev.force_reset();
        }
        if self.saw_vbus {
            // Do not carry class buffers or control-line state into a later host session.
            if let Some(serial) = self.serial.as_mut() {
                serial.reset();
            }
        }
        // Initialization can race cable removal before `saw_vbus` is published. The
        // bus then exists with USBD enabled and D+ intentionally detached. Mark every
        // constructed instance for a bounded reset so a later attach re-runs the
        // post-ENABLE regulator gate and reconnects the pull-up.
        self.awaiting_fresh_reset = self.dev.is_some();
        self.saw_vbus = false;
        self.fault = None;
        self.state = CdcState::Disconnected;
        self.state
    }
}

fn translate_io<T>(result: usb_device::Result<T>) -> Result<Option<T>, UsbBackendError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(UsbError::WouldBlock) => Ok(None),
        Err(UsbError::ParseError) => Err(UsbBackendError::Parse),
        Err(UsbError::BufferOverflow) => Err(UsbBackendError::BufferOverflow),
        Err(UsbError::EndpointOverflow) => Err(UsbBackendError::EndpointOverflow),
        Err(UsbError::EndpointMemoryOverflow) => Err(UsbBackendError::EndpointMemoryOverflow),
        Err(UsbError::InvalidEndpoint) => Err(UsbBackendError::InvalidEndpoint),
        Err(UsbError::Unsupported) => Err(UsbBackendError::Unsupported),
        Err(UsbError::InvalidState) => Err(UsbBackendError::InvalidState),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReconnectAction {
    Ready,
    Retry,
    Fault(UsbBackendError),
}

fn reconnect_action(
    reset: Option<Result<Option<()>, UsbBackendError>>,
    driver_fault: Option<UsbBackendError>,
    vbus_present: bool,
) -> ReconnectAction {
    if let Some(error) = driver_fault {
        return ReconnectAction::Fault(error);
    }
    if !vbus_present {
        return ReconnectAction::Retry;
    }
    match reset {
        Some(Ok(Some(()))) => ReconnectAction::Ready,
        // InvalidState without a terminal driver fault means VBUS or controller
        // state changed during the bounded reset. Keep the link unpublished and
        // retry on a later poll instead of suppressing all further attach attempts.
        Some(Err(UsbBackendError::InvalidState)) | Some(Ok(None)) | None => ReconnectAction::Retry,
        Some(Err(error)) => ReconnectAction::Fault(error),
    }
}

fn visible_state(
    vbus: bool,
    awaiting_fresh_reset: bool,
    operational: bool,
    state: UsbDeviceState,
) -> CdcState {
    if !vbus || (awaiting_fresh_reset && state != UsbDeviceState::Default) {
        return CdcState::Disconnected;
    }
    if !operational {
        return if state == UsbDeviceState::Suspend {
            CdcState::Suspended
        } else {
            CdcState::Disconnected
        };
    }
    match state {
        UsbDeviceState::Default => CdcState::Default,
        UsbDeviceState::Addressed => CdcState::Addressed,
        UsbDeviceState::Configured => CdcState::Configured,
        UsbDeviceState::Suspend => CdcState::Suspended,
    }
}

impl UsbStack for NrfUsbdCdc {
    fn poll(&mut self) -> CdcState {
        if let Some(error) = reported_driver_fault() {
            self.fault = Some(error);
            self.state = CdcState::Disconnected;
            return self.state;
        }
        let present = unsafe { nrf_vbus_present() };
        if !present {
            return self.observe_disconnect();
        }

        if self.dev.is_none() && !self.saw_vbus {
            let _ = unsafe { self.initialize() };
        }

        if self.awaiting_fresh_reset && !self.saw_vbus {
            let reset = self.dev.as_mut().map(|dev| translate_io(dev.force_reset()));
            let action = reconnect_action(reset, reported_driver_fault(), unsafe {
                nrf_vbus_present()
            });
            match action {
                ReconnectAction::Ready => {}
                ReconnectAction::Retry => {
                    self.saw_vbus = false;
                    self.state = CdcState::Disconnected;
                    return self.state;
                }
                ReconnectAction::Fault(error) => {
                    self.fault = Some(error);
                    self.saw_vbus = false;
                    self.state = CdcState::Disconnected;
                    return self.state;
                }
            }
        }
        self.saw_vbus = unsafe { nrf_vbus_present() };
        if !self.saw_vbus {
            return self.observe_disconnect();
        }

        let (Some(dev), Some(serial)) = (&mut self.dev, &mut self.serial) else {
            self.state = CdcState::Disconnected;
            return self.state;
        };
        dev.poll(&mut [serial]);
        if self.awaiting_fresh_reset && dev.state() == UsbDeviceState::Default {
            self.awaiting_fresh_reset = false;
        }
        if self.fault.is_some() {
            self.state = CdcState::Disconnected;
            return self.state;
        }
        self.state = visible_state(
            self.saw_vbus,
            self.awaiting_fresh_reset,
            DRIVER_OPERATIONAL.load(Ordering::Acquire),
            dev.state(),
        );
        self.state
    }

    fn write(&mut self, data: &[u8]) -> usize {
        match self.try_write(data) {
            Ok(accepted) => accepted,
            Err(error) => {
                self.fault = Some(error);
                self.state = CdcState::Disconnected;
                0
            }
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> usize {
        match self.try_read(buf) {
            Ok(read) => read,
            Err(error) => {
                self.fault = Some(error);
                self.state = CdcState::Disconnected;
                0
            }
        }
    }

    fn flush(&mut self) -> bool {
        match self.try_flush() {
            Ok(idle) => idle,
            Err(error) => {
                self.fault = Some(error);
                self.state = CdcState::Disconnected;
                false
            }
        }
    }

    fn try_write(&mut self, data: &[u8]) -> Result<usize, UsbBackendError> {
        let Some(serial) = self.serial.as_mut() else {
            return Err(self.fault.unwrap_or(UsbBackendError::Unavailable));
        };
        let result = serial.write(data);
        if let Some(error) = reported_driver_fault() {
            self.fault = Some(error);
            return Err(error);
        }
        translate_io(result).map(|accepted| accepted.unwrap_or(0))
    }

    fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, UsbBackendError> {
        let Some(serial) = self.serial.as_mut() else {
            return Err(self.fault.unwrap_or(UsbBackendError::Unavailable));
        };
        let result = serial.read(buf);
        if let Some(error) = reported_driver_fault() {
            self.fault = Some(error);
            return Err(error);
        }
        translate_io(result).map(|read| read.unwrap_or(0))
    }

    fn try_flush(&mut self) -> Result<bool, UsbBackendError> {
        let Some(serial) = self.serial.as_mut() else {
            return Err(self.fault.unwrap_or(UsbBackendError::Unavailable));
        };
        let result = serial.flush();
        if let Some(error) = reported_driver_fault() {
            self.fault = Some(error);
            return Err(error);
        }
        translate_io(result).map(|idle| idle.is_some())
    }

    fn force_reenumeration(&mut self) -> Result<(), UsbBackendError> {
        if let Some(error) = reported_driver_fault() {
            self.fault = Some(error);
            return Err(error);
        }
        if !unsafe { nrf_vbus_present() } {
            return Err(UsbBackendError::Unavailable);
        }
        let Some(dev) = self.dev.as_mut() else {
            return Err(self.fault.unwrap_or(UsbBackendError::Unavailable));
        };

        let action = reconnect_action(
            Some(translate_io(dev.force_reset())),
            reported_driver_fault(),
            unsafe { nrf_vbus_present() },
        );
        match action {
            ReconnectAction::Ready => {
                if let Some(serial) = self.serial.as_mut() {
                    serial.reset();
                }
                self.saw_vbus = true;
                self.awaiting_fresh_reset = true;
                self.fault = None;
                self.state = CdcState::Disconnected;
                Ok(())
            }
            ReconnectAction::Retry => {
                self.saw_vbus = false;
                self.state = CdcState::Disconnected;
                Err(UsbBackendError::InvalidState)
            }
            ReconnectAction::Fault(error) => {
                self.fault = Some(error);
                self.saw_vbus = false;
                self.state = CdcState::Disconnected;
                Err(error)
            }
        }
    }

    fn poll_bootloader_handoff(&mut self) -> Result<bool, UsbBackendError> {
        if let Some(error) = reported_driver_fault() {
            self.fault = Some(error);
            return Err(error);
        }
        let Some(dev) = self.dev.as_mut() else {
            return Err(self.fault.unwrap_or(UsbBackendError::Unavailable));
        };

        request_bootloader_handoff();
        let complete = match dev.force_reset() {
            Ok(()) => true,
            Err(UsbError::WouldBlock) => false,
            Err(error) => return translate_io::<()>(Err(error)).map(|_| false),
        };
        if let Some(error) = reported_driver_fault() {
            self.fault = Some(error);
            return Err(error);
        }
        self.saw_vbus = false;
        self.awaiting_fresh_reset = false;
        self.state = CdcState::Disconnected;
        Ok(complete)
    }

    fn backend_fault(&self) -> Option<UsbBackendError> {
        self.fault.or_else(reported_driver_fault)
    }

    fn configured(&self) -> bool {
        self.state == CdcState::Configured
    }

    fn backend_id(&self) -> u32 {
        backend_id::NRF_USBD
    }
}

#[cfg(test)]
mod tests {
    use usb_device::{device::UsbDeviceState, UsbError};

    use super::{
        decode_driver_fault, encode_driver_fault, reconnect_action, timer0_is_microsecond_timebase,
        translate_io, usb_power_status_ready, visible_state, MountClaim, ReconnectAction,
        NRF_EP0_MAX_PACKET_SIZE, USB_OUTPUT_READY, VBUS_DETECTED,
    };
    use crate::{CdcState, UsbBackendError};
    use nrf_usbd::UsbdFault;

    #[test]
    fn mount_claim_is_permanent_and_rejects_a_second_mount() {
        let claim = MountClaim::new();
        assert!(claim.try_claim());
        assert!(!claim.try_claim());
    }

    #[test]
    fn usb_power_ready_signal_requires_vbus_and_regulator_output() {
        assert!(!usb_power_status_ready(0));
        assert!(!usb_power_status_ready(VBUS_DETECTED));
        assert!(!usb_power_status_ready(USB_OUTPUT_READY));
        assert!(usb_power_status_ready(VBUS_DETECTED | USB_OUTPUT_READY));
        assert!(usb_power_status_ready(
            VBUS_DETECTED | USB_OUTPUT_READY | 0x8000_0000
        ));
    }

    #[test]
    fn nrf_control_endpoint_uses_the_hardware_packet_size() {
        assert_eq!(NRF_EP0_MAX_PACKET_SIZE, 64);
    }

    #[test]
    fn usb_lifecycle_clock_accepts_only_nobros_one_mhz_timer0_contract() {
        assert!(timer0_is_microsecond_timebase(true, true, 4));
        assert!(!timer0_is_microsecond_timebase(false, true, 4));
        assert!(!timer0_is_microsecond_timebase(true, false, 4));
        assert!(!timer0_is_microsecond_timebase(true, true, 0));
    }

    #[test]
    fn would_block_is_distinct_from_backend_faults() {
        assert_eq!(translate_io::<u8>(Err(UsbError::WouldBlock)), Ok(None));
        assert_eq!(
            translate_io::<u8>(Err(UsbError::InvalidEndpoint)),
            Err(UsbBackendError::InvalidEndpoint)
        );
    }

    #[test]
    fn driver_timeout_reports_preserve_direction_and_endpoint() {
        for endpoint in 0..8 {
            assert_eq!(
                decode_driver_fault(encode_driver_fault(UsbdFault::InDmaTimeout { endpoint })),
                Some(UsbBackendError::InTransferTimeout { endpoint })
            );
            assert_eq!(
                decode_driver_fault(encode_driver_fault(UsbdFault::OutDmaTimeout { endpoint })),
                Some(UsbBackendError::OutTransferTimeout { endpoint })
            );
        }
        assert_eq!(
            decode_driver_fault(encode_driver_fault(UsbdFault::EnableTimeout)),
            Some(UsbBackendError::StartupTimeout)
        );
        assert_eq!(
            decode_driver_fault(encode_driver_fault(UsbdFault::DmaStorageAlreadyClaimed)),
            Some(UsbBackendError::Unavailable)
        );
        assert_eq!(
            decode_driver_fault(encode_driver_fault(UsbdFault::WakeTimeout)),
            Some(UsbBackendError::ControllerTimeout)
        );
        assert_eq!(
            decode_driver_fault(encode_driver_fault(UsbdFault::PowerReadyTimeout)),
            Some(UsbBackendError::StartupTimeout)
        );
        assert_eq!(
            decode_driver_fault(encode_driver_fault(UsbdFault::DisableTimeout)),
            Some(UsbBackendError::ControllerTimeout)
        );
        assert_eq!(
            decode_driver_fault(encode_driver_fault(UsbdFault::UnsupportedSilicon)),
            Some(UsbBackendError::Unsupported)
        );
        assert_eq!(
            decode_driver_fault(encode_driver_fault(UsbdFault::ParityRepairUnavailable)),
            Some(UsbBackendError::InvalidState)
        );
    }

    #[test]
    fn reconnect_retries_transient_reset_failures_and_latches_driver_faults() {
        assert_eq!(
            reconnect_action(Some(Err(UsbBackendError::InvalidState)), None, true),
            ReconnectAction::Retry
        );
        assert_eq!(
            reconnect_action(Some(Ok(Some(()))), None, true),
            ReconnectAction::Ready
        );
        assert_eq!(
            reconnect_action(Some(Ok(Some(()))), None, false),
            ReconnectAction::Retry
        );
        assert_eq!(
            reconnect_action(
                Some(Err(UsbBackendError::InvalidState)),
                Some(UsbBackendError::ControllerTimeout),
                true
            ),
            ReconnectAction::Fault(UsbBackendError::ControllerTimeout)
        );
    }

    #[test]
    fn live_vbus_overrides_a_stale_configured_state() {
        assert_eq!(
            visible_state(false, false, true, UsbDeviceState::Configured),
            CdcState::Disconnected
        );
        assert_eq!(
            visible_state(true, false, false, UsbDeviceState::Suspend),
            CdcState::Suspended
        );
        assert_eq!(
            visible_state(true, true, true, UsbDeviceState::Configured),
            CdcState::Disconnected
        );
        assert_eq!(
            visible_state(true, true, true, UsbDeviceState::Default),
            CdcState::Default
        );
        assert_eq!(
            visible_state(true, false, false, UsbDeviceState::Configured),
            CdcState::Disconnected
        );
    }
}
