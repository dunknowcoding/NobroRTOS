//! Modular, mountable USB device stack for NobroRTOS.
//!
//! A board mounts exactly one backend behind the [`UsbStack`] trait, chosen at build time
//! by a `backend-*` cargo feature. The app never names a concrete stack - it calls
//! [`mount`] and talks CDC bytes. Only implemented backends are selectable; placeholder
//! stacks are deliberately not advertised as working features.
//!
//! ```
//! use nobro_usb::{CdcState, UsbConfig, UsbStack};
//!
//! fn usb_stack_demo() {
//!     let cfg = UsbConfig::new(0x1209, 0x0001, "NobroRTOS", "NobroRTOS CDC", "NBRO1");
//!     let mut usb = nobro_usb::mount(&cfg);
//!     let mut bytes = [0u8; 64];
//!     loop {
//!         if usb.poll() == CdcState::Configured {
//!             match usb.read_available(&mut bytes) {
//!                 Ok(_count) => {
//!                     // Process the received prefix; retain outbound data until a later
//!                     // error-aware write accepts it completely.
//!                 }
//!                 Err(_) => break,
//!             }
//!         }
//!     }
//! }
//! ```
#![no_std]

use portable_atomic::{AtomicBool, Ordering};

#[cfg(not(any(
    feature = "backend-nrf-usbd",
    feature = "backend-usb-serial-jtag-esp32c3",
    feature = "backend-usb-serial-jtag-esp32s3",
    feature = "backend-ra-usbfs"
)))]
compile_error!("exactly one USB backend feature must be enabled");

#[cfg(any(
    all(
        feature = "backend-nrf-usbd",
        any(
            feature = "backend-usb-serial-jtag-esp32c3",
            feature = "backend-usb-serial-jtag-esp32s3",
            feature = "backend-ra-usbfs"
        )
    ),
    all(
        feature = "backend-ra-usbfs",
        any(
            feature = "backend-usb-serial-jtag-esp32c3",
            feature = "backend-usb-serial-jtag-esp32s3"
        )
    ),
    all(
        feature = "backend-usb-serial-jtag-esp32c3",
        feature = "backend-usb-serial-jtag-esp32s3"
    )
))]
compile_error!("USB backend features are mutually exclusive");

/// Enumeration progress of the CDC device, backend-agnostic.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CdcState {
    /// No host / VBUS, or the stack is not up.
    Disconnected,
    /// Powered but not yet addressed.
    Default,
    /// The host assigned an address.
    Addressed,
    /// Fully enumerated - the CDC pipe is usable.
    Configured,
    /// The host suspended an otherwise attached device; data pipes are not usable.
    Suspended,
}

/// Maximum transfer accepted by the exact-write convenience API.
///
/// All currently selectable backends expose full-speed CDC-sized 64-byte packets.
/// Larger messages must be split by the caller so backpressure can be handled between
/// packets without allocating a hidden queue.
pub const CDC_PACKET_SIZE: usize = 64;

/// A controller or class-driver failure that is distinct from ordinary endpoint
/// backpressure and an empty receive queue.
///
/// This enum deliberately does not include a `WouldBlock` variant: selectable backends
/// report that expected non-blocking condition as `Ok(0)` (or `Ok(false)` for flush).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbBackendError {
    /// Device or class input could not be parsed.
    Parse,
    /// A caller or class buffer could not hold the transfer.
    BufferOverflow,
    /// The controller has no endpoint left for the requested class layout.
    EndpointOverflow,
    /// The controller has insufficient packet memory for the requested endpoints.
    EndpointMemoryOverflow,
    /// An endpoint address is invalid or is not owned by this class.
    InvalidEndpoint,
    /// The controller does not support the requested operation.
    Unsupported,
    /// The operation is invalid in the controller's current state.
    InvalidState,
    /// Bounded peripheral startup did not complete before its retry limit.
    StartupTimeout,
    /// A bounded controller/FIFO operation did not complete before its runtime limit.
    ControllerTimeout,
    /// An IN endpoint transfer did not complete within the backend's hardware budget.
    InTransferTimeout { endpoint: u8 },
    /// An OUT endpoint transfer did not complete within the backend's hardware budget.
    OutTransferTimeout { endpoint: u8 },
    /// The selected backend is not currently available for I/O.
    Unavailable,
}

/// Failures reported by [`MountedUsb::write_all`], [`MountedUsb::read_available`],
/// and [`MountedUsb::flush_pending`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbIoError {
    /// The selected backend has not reached the configured state, or has disconnected.
    NotConfigured,
    /// The request exceeds the bounded single-packet contract.
    Oversize { requested: usize, maximum: usize },
    /// The endpoint accepted no bytes because its bounded buffer is busy.
    Backpressure,
    /// The endpoint accepted a non-zero prefix rather than the complete request.
    ShortWrite { requested: usize, accepted: usize },
    /// A backend violated the write contract by reporting more bytes than were offered.
    InvalidWriteCount { requested: usize, reported: usize },
    /// The selected controller or class driver reported a real fault.
    Backend(UsbBackendError),
}

/// Requested device identity and strings supplied at mount.
///
/// This is the advertised identity only when [`identity_policy`] returns
/// [`UsbIdentityPolicy::Requested`]. Fixed-function controllers may ignore it, while
/// flash-resident descriptor backends may accept only one exact value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbConfig {
    pub vid: u16,
    pub pid: u16,
    pub manufacturer: &'static str,
    pub product: &'static str,
    pub serial: &'static str,
}

impl UsbConfig {
    pub const fn new(
        vid: u16,
        pid: u16,
        manufacturer: &'static str,
        product: &'static str,
        serial: &'static str,
    ) -> Self {
        Self {
            vid,
            pid,
            manufacturer,
            product,
            serial,
        }
    }
}

/// How the selected backend relates the requested [`UsbConfig`] to its advertised
/// descriptor identity.
///
/// Call [`identity_policy`] before mount when software needs to distinguish a generated
/// identity from a controller- or flash-fixed identity. Acceptance alone does not mean
/// that the requested fields will appear on the bus; use [`config_supported`] for the
/// separate preflight question.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbIdentityPolicy {
    /// The backend generates descriptors from the requested configuration.
    Requested,
    /// The backend has one fixed descriptor identity and accepts only that exact request.
    Exact(UsbConfig),
    /// Silicon owns the descriptors. The request is accepted for API uniformity but is
    /// not the identity advertised to the host.
    ControllerFixed,
}

/// A failure to acquire the selected USB backend.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbMountError {
    /// This firmware instance has already permanently claimed its USB backend.
    AlreadyMounted,
    /// The selected backend cannot represent the requested descriptor configuration.
    UnsupportedConfig,
}

fn policy_supports_config(policy: UsbIdentityPolicy, cfg: &UsbConfig) -> bool {
    match policy {
        UsbIdentityPolicy::Requested | UsbIdentityPolicy::ControllerFixed => true,
        UsbIdentityPolicy::Exact(required) => required == *cfg,
    }
}

/// Descriptor-identity policy of the one backend selected for this build.
#[cfg(feature = "backend-nrf-usbd")]
pub const fn identity_policy() -> UsbIdentityPolicy {
    UsbIdentityPolicy::Requested
}

/// Descriptor-identity policy of the one backend selected for this build.
#[cfg(any(
    feature = "backend-usb-serial-jtag-esp32c3",
    feature = "backend-usb-serial-jtag-esp32s3"
))]
pub const fn identity_policy() -> UsbIdentityPolicy {
    UsbIdentityPolicy::ControllerFixed
}

/// Descriptor-identity policy of the one backend selected for this build.
#[cfg(feature = "backend-ra-usbfs")]
pub const fn identity_policy() -> UsbIdentityPolicy {
    UsbIdentityPolicy::Exact(RA4M1_USB_CONFIG)
}

/// Preflight whether the selected backend accepts `cfg`, without claiming or touching
/// the USB controller.
pub fn config_supported(cfg: &UsbConfig) -> bool {
    policy_supports_config(identity_policy(), cfg)
}

/// Backend identity tags (surfaced in diagnostics / NOBRO reports).
pub mod backend_id {
    pub const NRF_USBD: u32 = 0x4E55_5246; // "NURF"
    pub const USB_SERIAL_JTAG: u32 = 0x4E55_534A; // "NUSJ" (ESP32-C3/S3 fixed-function)
    pub const RA_USBFS: u32 = 0x4E55_5241; // "NURA" (RA4M1 / UNO R4 USBFS)
}

/// The mountable USB device surface. One backend implements this per board.
pub trait UsbStack {
    /// Service the stack once (call frequently / from the USB IRQ) and report progress.
    fn poll(&mut self) -> CdcState;
    /// Write bytes to the CDC IN endpoint; returns how many were accepted.
    ///
    /// This compatibility method cannot return a controller fault. New generic code
    /// should call [`UsbStack::try_write`].
    fn write(&mut self, data: &[u8]) -> usize;
    /// Read bytes from the CDC OUT endpoint; returns how many were read (0 if none).
    ///
    /// This compatibility method cannot return a controller fault. New generic code
    /// should call [`UsbStack::try_read`].
    fn read(&mut self, buf: &mut [u8]) -> usize;
    /// Try to drain backend-owned transmit buffering into the USB controller.
    ///
    /// `true` means Nobro and the selected backend retain no pending bytes. Like the
    /// conventional serial `flush` contract, it does not prove that a host application
    /// has consumed the bytes after the controller transmitted them. The compatibility
    /// default is `false`: an older implementation still compiles, but cannot invent
    /// evidence that its private buffers are drained.
    fn flush(&mut self) -> bool {
        false
    }
    /// Error-aware non-blocking write.
    ///
    /// The default preserves compatibility for count-only backends. Implementations
    /// with a fallible controller API should override it instead of collapsing faults
    /// into a zero-byte result.
    fn try_write(&mut self, data: &[u8]) -> Result<usize, UsbBackendError> {
        Ok(self.write(data))
    }
    /// Error-aware non-blocking read. `Ok(0)` means that no bytes are currently ready.
    fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, UsbBackendError> {
        Ok(self.read(buf))
    }
    /// Error-aware flush. `Ok(false)` means bounded transmit data remains pending.
    fn try_flush(&mut self) -> Result<bool, UsbBackendError> {
        Ok(self.flush())
    }
    /// A persistent startup/controller fault that prevents an I/O attempt.
    fn backend_fault(&self) -> Option<UsbBackendError> {
        None
    }
    /// Explicitly detach and reattach the current device session.
    ///
    /// This is a recovery operation for a host/controller session that made no
    /// enumeration progress. Applications must rate-limit it; routine USB suspend is
    /// valid and should normally be left for [`UsbStack::poll`] to resume. Backends
    /// without a controllable pull-up return [`UsbBackendError::Unsupported`]. `Ok`
    /// means the detach was accepted; completion is asynchronous and must be driven
    /// by subsequent [`UsbStack::poll`] calls until the link enumerates again.
    fn force_reenumeration(&mut self) -> Result<(), UsbBackendError> {
        Err(UsbBackendError::Unsupported)
    }
    /// Begins or advances a one-way handoff to a resident bootloader.
    ///
    /// `Ok(false)` means teardown is still in progress and the caller must invoke this
    /// method again without performing endpoint I/O. `Ok(true)` proves the controller
    /// pull-up is off, cumulative EasyDMA parity is repaired, `ENABLE` reads disabled,
    /// and lifecycle errata ownership is closed. Backends without such a contract
    /// return [`UsbBackendError::Unsupported`].
    fn poll_bootloader_handoff(&mut self) -> Result<bool, UsbBackendError> {
        Err(UsbBackendError::Unsupported)
    }
    /// True only while the most recently observed link state is configured and usable.
    fn configured(&self) -> bool;
    /// Which backend is mounted (see [`backend_id`]).
    fn backend_id(&self) -> u32;
}

#[cfg(feature = "backend-nrf-usbd")]
mod nrf_usbd_backend;
#[cfg(feature = "backend-nrf-usbd")]
use nrf_usbd_backend::NrfUsbdCdc;

#[cfg(any(
    feature = "backend-usb-serial-jtag-esp32c3",
    feature = "backend-usb-serial-jtag-esp32s3"
))]
mod usb_serial_jtag_backend;
#[cfg(any(
    feature = "backend-usb-serial-jtag-esp32c3",
    feature = "backend-usb-serial-jtag-esp32s3"
))]
use usb_serial_jtag_backend::UsbSerialJtagCdc;

#[cfg(feature = "backend-ra-usbfs")]
mod ra_usbfs_backend;
#[cfg(feature = "backend-ra-usbfs")]
use ra_usbfs_backend::RaUsbfsCdc;
#[cfg(feature = "backend-ra-usbfs")]
pub use ra_usbfs_backend::Stage;
#[cfg(feature = "backend-ra-usbfs")]
pub use ra_usbfs_backend::RA4M1_USB_CONFIG;

#[cfg(feature = "backend-nrf-usbd")]
type ActiveBackend = NrfUsbdCdc;
#[cfg(any(
    feature = "backend-usb-serial-jtag-esp32c3",
    feature = "backend-usb-serial-jtag-esp32s3"
))]
type ActiveBackend = UsbSerialJtagCdc;
#[cfg(feature = "backend-ra-usbfs")]
type ActiveBackend = RaUsbfsCdc;

/// The single backend selected for this build, owned behind the common stack surface.
///
/// Construct this only with [`try_mount`] (or its panic-compatible [`mount`] wrapper).
/// The wrapper applies the process-wide mount claim, remembers the current link state,
/// and provides exact-write error reporting without exposing a board-specific backend
/// to application/provider code.
pub struct MountedUsb {
    backend: ActiveBackend,
    state: CdcState,
}

impl MountedUsb {
    fn new(backend: ActiveBackend) -> Self {
        Self {
            backend,
            state: CdcState::Disconnected,
        }
    }

    /// Last state observed by [`UsbStack::poll`].
    pub fn state(&self) -> CdcState {
        self.state
    }

    /// Accept the complete request or report why it must be retried/split.
    ///
    /// This method never reports success after accepting only a prefix.
    pub fn write_all(&mut self, data: &[u8]) -> Result<(), UsbIoError> {
        let configured = self.poll() == CdcState::Configured;
        if let Some(error) = self.backend.backend_fault() {
            return Err(UsbIoError::Backend(error));
        }
        write_exact_with(configured, data, CDC_PACKET_SIZE, |packet| {
            self.backend.try_write(packet)
        })
    }

    /// Read currently available bytes without blocking.
    pub fn read_available(&mut self, buf: &mut [u8]) -> Result<usize, UsbIoError> {
        let configured = self.poll() == CdcState::Configured;
        if let Some(error) = self.backend.backend_fault() {
            return Err(UsbIoError::Backend(error));
        }
        if !configured {
            return Err(UsbIoError::NotConfigured);
        }
        self.backend.try_read(buf).map_err(UsbIoError::Backend)
    }

    /// Service the selected backend and drain its bounded transmit buffer.
    ///
    /// [`UsbIoError::Backpressure`] means bytes are still pending and the caller must
    /// poll and retry. Success does not mean that the host application has read them.
    pub fn flush_pending(&mut self) -> Result<(), UsbIoError> {
        let configured = self.poll() == CdcState::Configured;
        if let Some(error) = self.backend.backend_fault() {
            return Err(UsbIoError::Backend(error));
        }
        if !configured {
            return Err(UsbIoError::NotConfigured);
        }
        let idle = self.backend.try_flush().map_err(UsbIoError::Backend)?;
        flush_with(true, idle)
    }
}

#[cfg(feature = "backend-ra-usbfs")]
impl MountedUsb {
    /// RA4M1 enumeration stage for probe-less status indication.
    pub fn stage(&self) -> Stage {
        self.backend.stage()
    }

    /// Drop the RA4M1 D+ pull-up and reset its USB session before the board-level mux
    /// returns the connector to the upload bridge.
    pub fn disconnect_link(&mut self) {
        self.backend.disconnect();
        self.state = CdcState::Disconnected;
    }

    /// Re-arm the existing RA4M1 controller after the board-level mux is routed back to
    /// native USB. The next poll observes the new enumeration state.
    pub fn reconnect_link(&mut self) {
        self.backend.reconnect();
        self.state = CdcState::Disconnected;
    }
}

impl UsbStack for MountedUsb {
    fn poll(&mut self) -> CdcState {
        self.state = self.backend.poll();
        self.state
    }

    fn write(&mut self, data: &[u8]) -> usize {
        self.backend.write(data)
    }

    fn read(&mut self, buf: &mut [u8]) -> usize {
        self.backend.read(buf)
    }

    fn flush(&mut self) -> bool {
        self.backend.flush()
    }

    fn try_write(&mut self, data: &[u8]) -> Result<usize, UsbBackendError> {
        self.backend.try_write(data)
    }

    fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, UsbBackendError> {
        self.backend.try_read(buf)
    }

    fn try_flush(&mut self) -> Result<bool, UsbBackendError> {
        self.backend.try_flush()
    }

    fn backend_fault(&self) -> Option<UsbBackendError> {
        self.backend.backend_fault()
    }

    fn force_reenumeration(&mut self) -> Result<(), UsbBackendError> {
        let result = self.backend.force_reenumeration();
        if result.is_ok() {
            self.state = CdcState::Disconnected;
        }
        result
    }

    fn poll_bootloader_handoff(&mut self) -> Result<bool, UsbBackendError> {
        let result = self.backend.poll_bootloader_handoff();
        self.state = CdcState::Disconnected;
        result
    }

    fn configured(&self) -> bool {
        self.state == CdcState::Configured
    }

    fn backend_id(&self) -> u32 {
        self.backend.backend_id()
    }
}

fn write_exact_with(
    configured: bool,
    data: &[u8],
    maximum: usize,
    write: impl FnOnce(&[u8]) -> Result<usize, UsbBackendError>,
) -> Result<(), UsbIoError> {
    if !configured {
        return Err(UsbIoError::NotConfigured);
    }
    if data.len() > maximum {
        return Err(UsbIoError::Oversize {
            requested: data.len(),
            maximum,
        });
    }
    if data.is_empty() {
        return Ok(());
    }

    let accepted = write(data).map_err(UsbIoError::Backend)?;
    if accepted == data.len() {
        Ok(())
    } else if accepted == 0 {
        Err(UsbIoError::Backpressure)
    } else if accepted < data.len() {
        Err(UsbIoError::ShortWrite {
            requested: data.len(),
            accepted,
        })
    } else {
        Err(UsbIoError::InvalidWriteCount {
            requested: data.len(),
            reported: accepted,
        })
    }
}

fn flush_with(configured: bool, backend_idle: bool) -> Result<(), UsbIoError> {
    if !configured {
        Err(UsbIoError::NotConfigured)
    } else if backend_idle {
        Ok(())
    } else {
        Err(UsbIoError::Backpressure)
    }
}

struct MountClaim(AtomicBool);

impl MountClaim {
    const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    fn claim(&self) -> bool {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

static MOUNTED: MountClaim = MountClaim::new();

fn try_mount_with<T>(
    policy: UsbIdentityPolicy,
    cfg: &UsbConfig,
    claim: &MountClaim,
    construct: impl FnOnce() -> T,
) -> Result<T, UsbMountError> {
    if !policy_supports_config(policy, cfg) {
        return Err(UsbMountError::UnsupportedConfig);
    }
    if !claim.claim() {
        return Err(UsbMountError::AlreadyMounted);
    }
    Ok(construct())
}

#[cfg(feature = "backend-nrf-usbd")]
fn mount_backend(cfg: &UsbConfig) -> ActiveBackend {
    NrfUsbdCdc::mount(cfg)
}

#[cfg(any(
    feature = "backend-usb-serial-jtag-esp32c3",
    feature = "backend-usb-serial-jtag-esp32s3"
))]
fn mount_backend(cfg: &UsbConfig) -> ActiveBackend {
    UsbSerialJtagCdc::mount(cfg)
}

#[cfg(feature = "backend-ra-usbfs")]
fn mount_backend(cfg: &UsbConfig) -> ActiveBackend {
    RaUsbfsCdc::mount(cfg)
}

/// Try to mount the USB stack selected for this board.
///
/// Configuration support is checked before the permanent process-wide claim and before
/// any backend touches hardware. Exactly one `backend-*` feature must be enabled.
pub fn try_mount(cfg: &UsbConfig) -> Result<MountedUsb, UsbMountError> {
    try_mount_with(identity_policy(), cfg, &MOUNTED, || {
        MountedUsb::new(mount_backend(cfg))
    })
}

/// Mount the USB stack selected for this board.
///
/// This panic-on-error wrapper preserves the original API. New firmware should prefer
/// [`try_mount`] so an unsupported fixed descriptor or duplicate mount is explicit.
#[track_caller]
pub fn mount(cfg: &UsbConfig) -> MountedUsb {
    match try_mount(cfg) {
        Ok(usb) => usb,
        Err(UsbMountError::AlreadyMounted) => {
            panic!("a USB backend can only be mounted once")
        }
        Err(UsbMountError::UnsupportedConfig) => {
            panic!("the selected USB backend does not support the requested UsbConfig")
        }
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use super::{
        config_supported, flush_with, identity_policy, policy_supports_config, try_mount_with,
        write_exact_with, CdcState, MountClaim, UsbBackendError, UsbConfig, UsbIdentityPolicy,
        UsbIoError, UsbMountError, UsbStack,
    };

    struct PreFlushCompatibilityBackend;

    // Regression fixture for third-party implementations written before `flush` and
    // the error-aware methods were added to UsbStack.
    impl UsbStack for PreFlushCompatibilityBackend {
        fn poll(&mut self) -> CdcState {
            CdcState::Configured
        }

        fn write(&mut self, data: &[u8]) -> usize {
            data.len()
        }

        fn read(&mut self, _buf: &mut [u8]) -> usize {
            0
        }

        fn configured(&self) -> bool {
            true
        }

        fn backend_id(&self) -> u32 {
            0
        }
    }

    #[test]
    fn pre_flush_trait_implementation_uses_compatible_defaults() {
        let mut backend = PreFlushCompatibilityBackend;
        // The compatibility default fails closed: it compiles without claiming that a
        // backend written before the method existed has no retained bytes.
        assert!(!backend.flush());
        assert_eq!(backend.try_flush(), Ok(false));
        assert_eq!(backend.try_write(b"abc"), Ok(3));
        assert_eq!(backend.try_read(&mut [0; 1]), Ok(0));
        assert_eq!(
            backend.force_reenumeration(),
            Err(UsbBackendError::Unsupported)
        );
        assert_eq!(
            backend.poll_bootloader_handoff(),
            Err(UsbBackendError::Unsupported)
        );
    }

    #[test]
    fn identity_policy_preflight_distinguishes_requested_exact_and_fixed() {
        let expected = UsbConfig::new(1, 2, "maker", "product", "serial");
        let other = UsbConfig::new(1, 3, "maker", "product", "serial");
        assert!(policy_supports_config(UsbIdentityPolicy::Requested, &other));
        assert!(policy_supports_config(
            UsbIdentityPolicy::ControllerFixed,
            &other
        ));
        assert!(policy_supports_config(
            UsbIdentityPolicy::Exact(expected),
            &expected
        ));
        assert!(!policy_supports_config(
            UsbIdentityPolicy::Exact(expected),
            &other
        ));
    }

    #[test]
    fn selected_backend_wires_its_public_identity_policy() {
        let arbitrary = UsbConfig::new(0x1234, 0x5678, "maker", "product", "serial");

        #[cfg(feature = "backend-nrf-usbd")]
        {
            assert_eq!(identity_policy(), UsbIdentityPolicy::Requested);
            assert!(config_supported(&arbitrary));
        }

        #[cfg(any(
            feature = "backend-usb-serial-jtag-esp32c3",
            feature = "backend-usb-serial-jtag-esp32s3"
        ))]
        {
            assert_eq!(identity_policy(), UsbIdentityPolicy::ControllerFixed);
            assert!(config_supported(&arbitrary));
        }

        #[cfg(feature = "backend-ra-usbfs")]
        {
            assert_eq!(
                identity_policy(),
                UsbIdentityPolicy::Exact(super::RA4M1_USB_CONFIG)
            );
            assert!(config_supported(&super::RA4M1_USB_CONFIG));
            assert!(!config_supported(&arbitrary));
        }
    }

    #[test]
    fn rejected_or_duplicate_mount_never_constructs_backend() {
        let expected = UsbConfig::new(1, 2, "maker", "product", "serial");
        let other = UsbConfig::new(1, 3, "maker", "product", "serial");
        let claim = MountClaim::new();
        let constructed = Cell::new(0);

        assert_eq!(
            try_mount_with(UsbIdentityPolicy::Exact(expected), &other, &claim, || {
                constructed.set(constructed.get() + 1)
            }),
            Err(UsbMountError::UnsupportedConfig)
        );
        assert_eq!(constructed.get(), 0);

        assert_eq!(
            try_mount_with(
                UsbIdentityPolicy::Exact(expected),
                &expected,
                &claim,
                || { constructed.set(constructed.get() + 1) }
            ),
            Ok(())
        );
        assert_eq!(constructed.get(), 1);

        assert_eq!(
            try_mount_with(UsbIdentityPolicy::Requested, &other, &claim, || {
                constructed.set(constructed.get() + 1)
            }),
            Err(UsbMountError::AlreadyMounted)
        );
        assert_eq!(constructed.get(), 1);
    }

    #[test]
    fn global_mount_contract_is_permanent() {
        let claim = MountClaim::new();
        assert!(claim.claim());
        assert!(!claim.claim());
    }

    #[test]
    fn exact_write_rejects_preflight_failures_without_touching_backend() {
        let calls = Cell::new(0);

        assert_eq!(
            write_exact_with(false, b"x", 64, |_| {
                calls.set(calls.get() + 1);
                Ok(1)
            }),
            Err(UsbIoError::NotConfigured)
        );
        assert_eq!(
            write_exact_with(true, &[0; 65], 64, |_| {
                calls.set(calls.get() + 1);
                Ok(1)
            }),
            Err(UsbIoError::Oversize {
                requested: 65,
                maximum: 64
            })
        );
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn exact_write_distinguishes_complete_busy_and_partial_acceptance() {
        assert_eq!(write_exact_with(true, b"abc", 64, |_| Ok(3)), Ok(()));
        assert_eq!(
            write_exact_with(true, b"abc", 64, |_| Ok(0)),
            Err(UsbIoError::Backpressure)
        );
        assert_eq!(
            write_exact_with(true, b"abc", 64, |_| Ok(2)),
            Err(UsbIoError::ShortWrite {
                requested: 3,
                accepted: 2
            })
        );
        assert_eq!(
            write_exact_with(true, b"abc", 64, |_| Ok(4)),
            Err(UsbIoError::InvalidWriteCount {
                requested: 3,
                reported: 4
            })
        );
    }

    #[test]
    fn empty_exact_write_succeeds_without_touching_backend() {
        assert_eq!(
            write_exact_with(true, b"", 64, |_| panic!("empty write reached backend")),
            Ok(())
        );
    }

    #[test]
    fn exact_write_preserves_backend_faults() {
        assert_eq!(
            write_exact_with(true, b"abc", 64, |_| {
                Err(UsbBackendError::InvalidEndpoint)
            }),
            Err(UsbIoError::Backend(UsbBackendError::InvalidEndpoint))
        );
    }

    #[test]
    fn flush_distinguishes_link_loss_from_pending_transmit_data() {
        assert_eq!(flush_with(false, true), Err(UsbIoError::NotConfigured));
        assert_eq!(flush_with(true, false), Err(UsbIoError::Backpressure));
        assert_eq!(flush_with(true, true), Ok(()));
    }
}
