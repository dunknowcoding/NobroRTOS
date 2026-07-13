//! nRF52840 USB backend over the vendored `nrf-usbd` + `usbd-serial` CDC (the default for
//! the nRF dev boards, matching ArduinoNRF Layer-0's native `NrfUsbd`). Owns a `'static`
//! bus allocator so the `UsbDevice`/`SerialPort` can live inside the backend struct.

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use cortex_m::peripheral::NVIC;
use nrf52840_pac::Interrupt;
use nrf_usbd::{sanitize_handoff, UsbPeripheral, Usbd, UsbdFault};
use usb_device::class_prelude::UsbClass;
use usb_device::device::{UsbDevice, UsbDeviceBuilder, UsbDeviceState, UsbVidPid};
use usb_device::{bus::UsbBusAllocator, device::StringDescriptors, UsbError};
use usbd_serial::SerialPort;

use crate::{backend_id, CdcState, UsbBackendError, UsbConfig, UsbStack};

struct Nrf52840Usbd;

const DRIVER_FAULT_NONE: u8 = 0;
const DRIVER_FAULT_ENABLE_TIMEOUT: u8 = 1;
const DRIVER_FAULT_STORAGE_CLAIMED: u8 = 2;
const DRIVER_FAULT_IN_BASE: u8 = 0x10;
const DRIVER_FAULT_OUT_BASE: u8 = 0x20;
static DRIVER_FAULT: AtomicU8 = AtomicU8::new(DRIVER_FAULT_NONE);

fn encode_driver_fault(fault: UsbdFault) -> u8 {
    match fault {
        UsbdFault::EnableTimeout => DRIVER_FAULT_ENABLE_TIMEOUT,
        UsbdFault::DmaStorageAlreadyClaimed => DRIVER_FAULT_STORAGE_CLAIMED,
        UsbdFault::InDmaTimeout { endpoint } => DRIVER_FAULT_IN_BASE | (endpoint & 0x0f),
        UsbdFault::OutDmaTimeout { endpoint } => DRIVER_FAULT_OUT_BASE | (endpoint & 0x0f),
    }
}

fn decode_driver_fault(code: u8) -> Option<UsbBackendError> {
    match code {
        DRIVER_FAULT_NONE => None,
        DRIVER_FAULT_ENABLE_TIMEOUT => Some(UsbBackendError::StartupTimeout),
        DRIVER_FAULT_STORAGE_CLAIMED => Some(UsbBackendError::Unavailable),
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

    fn vbus_present() -> bool {
        unsafe { nrf_vbus_present() }
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

const CLOCK: u32 = 0x4000_0000;
const POWER: u32 = 0x4000_0000; // POWER shares the base region on nRF52
const EVENTS_HFCLKSTARTED: u32 = CLOCK + 0x100;
const HFCLKSTAT: u32 = CLOCK + 0x40C;
const USBREGSTATUS: u32 = POWER + 0x438;
const HFCLK_RUNNING: u32 = 1 << 16;
const HFCLK_XTAL: u32 = 1;
const VBUS_DETECTED: u32 = 1;
// This is an iteration bound, not a wall-clock API. It prevents a corrupt handoff or
// unavailable crystal from trapping the application forever during peripheral startup.
const HFXO_START_POLL_LIMIT: usize = 1_000_000;

unsafe fn rd(a: u32) -> u32 {
    core::ptr::read_volatile(a as *const u32)
}
unsafe fn wr(a: u32, v: u32) {
    core::ptr::write_volatile(a as *mut u32, v);
}

fn poll_until(limit: usize, mut ready: impl FnMut() -> bool) -> bool {
    for _ in 0..limit {
        if ready() {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

unsafe fn nrf_vbus_present() -> bool {
    rd(USBREGSTATUS) & VBUS_DETECTED != 0
}

unsafe fn hfxo_ready() -> bool {
    rd(HFCLKSTAT) & (HFCLK_RUNNING | HFCLK_XTAL) == (HFCLK_RUNNING | HFCLK_XTAL)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartAttempt {
    Ready,
    NoVbus,
    ClockTimeout,
}

/// Bring up the HFXO/USBD from a clean state without waiting for a cable forever.
///
/// Raw registers avoid taking the PAC singleton and support both reset and bootloader
/// handoff. The caller samples VBUS again after this bounded sequence before publishing
/// any connected state.
unsafe fn peripheral_clean_start() -> StartAttempt {
    if !nrf_vbus_present() {
        return StartAttempt::NoVbus;
    }

    // USB needs the external 32 MHz crystal. Check live clock status because clearing
    // and re-waiting for the edge event is not reliable when a bootloader left it running.
    if !hfxo_ready() {
        wr(EVENTS_HFCLKSTARTED, 0);
        wr(CLOCK, 1); // TASKS_HFCLKSTART
        if !poll_until(HFXO_START_POLL_LIMIT, || hfxo_ready()) {
            return StartAttempt::ClockTimeout;
        }
    }

    if !nrf_vbus_present() {
        return StartAttempt::NoVbus;
    }

    // This backend is polled. Mask and unpend a bootloader-owned USBD IRQ before the
    // PAC-derived transaction disconnects, disables, and clears every latched event,
    // endpoint status, interrupt source, and persistent session configuration.
    NVIC::mask(Interrupt::USBD);
    sanitize_handoff::<Nrf52840Usbd>();
    NVIC::unpend(Interrupt::USBD);
    for _ in 0..400_000u32 {
        cortex_m::asm::nop();
    }
    StartAttempt::Ready
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

    unsafe fn initialize(&mut self) -> StartAttempt {
        let start = peripheral_clean_start();
        if start != StartAttempt::Ready {
            self.fault = match start {
                StartAttempt::ClockTimeout => Some(UsbBackendError::StartupTimeout),
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
            let serial = SerialPort::new(alloc);
            let strings = StringDescriptors::default()
                .manufacturer(cfg.manufacturer)
                .product(cfg.product)
                .serial_number(cfg.serial);
            let dev = UsbDeviceBuilder::new(alloc, UsbVidPid(cfg.vid, cfg.pid))
                .strings(&[strings])
                .unwrap()
                .device_class(usbd_serial::USB_CLASS_CDC)
                .build();
            self.serial = Some(serial);
            self.dev = Some(dev);
        }
        self.fault = reported_driver_fault();
        StartAttempt::Ready
    }

    fn observe_disconnect(&mut self) -> CdcState {
        if self.saw_vbus {
            // Do not carry class buffers or control-line state into a later host session.
            if let Some(serial) = self.serial.as_mut() {
                serial.reset();
            }
            self.awaiting_fresh_reset = self.dev.is_some();
        }
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

fn visible_state(vbus: bool, awaiting_fresh_reset: bool, state: UsbDeviceState) -> CdcState {
    if !vbus || (awaiting_fresh_reset && state != UsbDeviceState::Default) {
        return CdcState::Disconnected;
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
            if let Some(Err(error)) = reset {
                self.fault = Some(error);
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
        self.state = visible_state(self.saw_vbus, self.awaiting_fresh_reset, dev.state());
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
    use core::cell::Cell;

    use usb_device::{device::UsbDeviceState, UsbError};

    use super::{
        decode_driver_fault, encode_driver_fault, poll_until, translate_io, visible_state,
        MountClaim,
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
    fn startup_poll_has_an_exact_bound() {
        let calls = Cell::new(0);
        assert!(!poll_until(3, || {
            calls.set(calls.get() + 1);
            false
        }));
        assert_eq!(calls.get(), 3);

        let calls = Cell::new(0);
        assert!(poll_until(4, || {
            calls.set(calls.get() + 1);
            calls.get() == 2
        }));
        assert_eq!(calls.get(), 2);
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
    }

    #[test]
    fn live_vbus_overrides_a_stale_configured_state() {
        assert_eq!(
            visible_state(false, false, UsbDeviceState::Configured),
            CdcState::Disconnected
        );
        assert_eq!(
            visible_state(true, false, UsbDeviceState::Suspend),
            CdcState::Suspended
        );
        assert_eq!(
            visible_state(true, true, UsbDeviceState::Configured),
            CdcState::Disconnected
        );
        assert_eq!(
            visible_state(true, true, UsbDeviceState::Default),
            CdcState::Default
        );
    }
}
