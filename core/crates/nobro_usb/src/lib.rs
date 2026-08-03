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

use nobro_power::{
    PowerHookError, PowerMode, PowerParticipant, PowerVetoMask, PowerVetoReason, SystemOffWake,
};
use portable_atomic::{AtomicBool, Ordering};

#[cfg(not(any(
    feature = "backend-nrf-usbd",
    feature = "backend-usb-serial-jtag-esp32c3",
    feature = "backend-usb-serial-jtag-esp32p4",
    feature = "backend-usb-serial-jtag-esp32s3",
    feature = "backend-ra-usbfs"
)))]
compile_error!("exactly one USB backend feature must be enabled");

#[cfg(any(
    all(
        feature = "backend-nrf-usbd",
        any(
            feature = "backend-usb-serial-jtag-esp32c3",
            feature = "backend-usb-serial-jtag-esp32p4",
            feature = "backend-usb-serial-jtag-esp32s3",
            feature = "backend-ra-usbfs"
        )
    ),
    all(
        feature = "backend-ra-usbfs",
        any(
            feature = "backend-usb-serial-jtag-esp32c3",
            feature = "backend-usb-serial-jtag-esp32p4",
            feature = "backend-usb-serial-jtag-esp32s3"
        )
    ),
    all(
        feature = "backend-usb-serial-jtag-esp32c3",
        any(
            feature = "backend-usb-serial-jtag-esp32p4",
            feature = "backend-usb-serial-jtag-esp32s3"
        )
    ),
    all(
        feature = "backend-usb-serial-jtag-esp32p4",
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

/// Stable logical identity of a USB stack instance in an application composition.
///
/// Current device backends own one physical controller, so only one instance can be
/// mounted in a firmware image. Naming the instance still matters: provider registries
/// and diagnostics can bind the receipt to the same logical stack without guessing
/// from a board name or backend feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbInstanceId(pub u16);

impl UsbInstanceId {
    /// Conventional instance used by the compatibility [`try_mount`] API.
    pub const PRIMARY: Self = Self(0);
}

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

impl UsbBackendError {
    /// Stable public fault code for fixed host reports.
    pub const fn code(self) -> u32 {
        match self {
            Self::Parse => 1,
            Self::BufferOverflow => 2,
            Self::EndpointOverflow => 3,
            Self::EndpointMemoryOverflow => 4,
            Self::InvalidEndpoint => 5,
            Self::Unsupported => 6,
            Self::InvalidState => 7,
            Self::StartupTimeout => 8,
            Self::ControllerTimeout => 9,
            Self::InTransferTimeout { endpoint } => 0x100 | endpoint as u32,
            Self::OutTransferTimeout { endpoint } => 0x200 | endpoint as u32,
            Self::Unavailable => 10,
        }
    }
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
    /// The per-mount operation identity is exhausted; no I/O was attempted.
    ProvenanceExhausted,
}

/// Requested device identity and strings supplied at mount.
///
/// This is the advertised identity only when [`identity_policy`] returns
/// [`UsbIdentityPolicy::Requested`]. Fixed-function controllers accept only
/// [`UsbConfig::controller_owned`], while flash-resident descriptor backends accept
/// only their published exact value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbConfig {
    pub(crate) vid: u16,
    pub(crate) pid: u16,
    pub(crate) manufacturer: &'static str,
    pub(crate) product: &'static str,
    pub(crate) serial: &'static str,
}

impl UsbConfig {
    pub const fn new(
        vid: u16,
        pid: u16,
        manufacturer: &'static str,
        product: &'static str,
        serial: &'static str,
    ) -> Self {
        assert!(vid != 0, "USB vendor id must be nonzero");
        // A UTF-8 byte bound is conservative for the USB UTF-16 descriptor bound:
        // every Unicode scalar occupies no more UTF-16 units than UTF-8 bytes.
        assert!(
            manufacturer.len() <= 126,
            "USB manufacturer string is too long"
        );
        assert!(product.len() <= 126, "USB product string is too long");
        assert!(serial.len() <= 126, "USB serial string is too long");
        Self {
            vid,
            pid,
            manufacturer,
            product,
            serial,
        }
    }

    /// Explicit request for descriptors owned by fixed-function silicon.
    ///
    /// The zero/empty sentinel cannot be confused with a requested USB identity.
    pub const fn controller_owned() -> Self {
        Self {
            vid: 0,
            pid: 0,
            manufacturer: "",
            product: "",
            serial: "",
        }
    }

    pub const fn is_controller_owned(self) -> bool {
        self.vid == 0
            && self.pid == 0
            && self.manufacturer.is_empty()
            && self.product.is_empty()
            && self.serial.is_empty()
    }

    pub const fn vid(self) -> u16 {
        self.vid
    }

    pub const fn pid(self) -> u16 {
        self.pid
    }

    pub const fn manufacturer(self) -> &'static str {
        self.manufacturer
    }

    pub const fn product(self) -> &'static str {
        self.product
    }

    pub const fn serial(self) -> &'static str {
        self.serial
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
    /// Silicon owns the descriptors. Only [`UsbConfig::controller_owned`] is accepted;
    /// a caller-supplied identity is rejected before controller ownership is claimed.
    ControllerFixed,
}

/// Identity that the selected backend will actually advertise.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbAdvertisedIdentity {
    /// Descriptors are generated from the mount request.
    Requested(UsbConfig),
    /// The backend accepts and advertises one exact flash-resident identity.
    Exact(UsbConfig),
    /// Silicon owns the descriptors; their strings are not represented by
    /// [`UsbConfig`] and therefore are deliberately not invented here.
    ControllerOwned,
}

/// Immutable limits and lifecycle operations of the selected backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbCapabilities {
    pub backend_id: u32,
    pub identity_policy: UsbIdentityPolicy,
    pub mtu_bytes: u16,
    pub rx_buffer_bytes: u16,
    pub tx_buffer_bytes: u16,
    pub service: UsbServiceLimits,
    pub lifecycle: UsbLifecycleSupport,
    pub force_reenumeration: bool,
    pub bootloader_handoff: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbResetSupport {
    Unsupported,
    ForceReenumeration,
    BoardManagedDisconnect,
}

/// Exact ownership boundary of the selected physical controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbLifecycleSupport {
    /// Current backends permanently own their singleton controller after mount.
    pub permanent_singleton_mount: bool,
    /// No current backend can release the global mount claim safely.
    pub unmount: bool,
    /// I/O is synchronous/non-blocking and retains no cancellable software queue.
    pub cancellable_operations: u8,
    pub reset: UsbResetSupport,
}

/// Bounded work performed by one call through the common mounted-stack surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbServiceLimits {
    /// At most one backend poll is performed by each mounted I/O convenience call.
    pub backend_polls_per_call: u8,
    /// At most one packet is offered to a backend read or write operation.
    pub packets_per_io_call: u8,
    /// The common exact-write API retains no hidden retry queue.
    pub hidden_retry_packets: u8,
}

const COMMON_SERVICE_LIMITS: UsbServiceLimits = UsbServiceLimits {
    backend_polls_per_call: 1,
    packets_per_io_call: 1,
    hidden_retry_packets: 0,
};

/// Exact, allocation-free receipt for one successful logical-stack mount.
///
/// A receipt is created only after configuration preflight and singleton ownership
/// succeed. It records the requested and effective descriptor policy separately so a
/// fixed-function controller can never be mistaken for a configurable CDC device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbMountReceipt {
    pub instance: UsbInstanceId,
    /// Numeric identity requested by the caller. Descriptor strings are bound by
    /// `requested_fingerprint` without leaking firmware addresses into the receipt.
    pub requested_vid: u16,
    pub requested_pid: u16,
    /// Stable FNV-1a fingerprint of every requested identity field.
    ///
    /// The caller retains the original configuration. Keeping only this binding avoids
    /// duplicating three fat string pointers in the mounted runtime object.
    pub requested_fingerprint: u32,
    pub advertised: UsbAdvertisedIdentity,
    pub capabilities: UsbCapabilities,
    /// Monotonic lifecycle generation within this firmware image.
    ///
    /// The current singleton controller can mount once, so the only valid successful
    /// generation is 1. Keeping it explicit makes remount/release evolution observable.
    pub lifecycle_generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum UsbOperationKind {
    Read = 1,
    Write = 2,
    Flush = 3,
    Reset = 4,
    BootloaderHandoff = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum UsbOperationStatus {
    Completed = 1,
    NoProgress = 2,
    Partial = 3,
    Rejected = 4,
    BackendFault = 5,
    ProvenanceExhausted = 6,
}

/// Stable provenance for one mounted-stack operation or fault.
///
/// It contains only logical ids and bounded numeric state, never host paths,
/// board nicknames, or backend-private pointers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbOperationReceipt {
    pub instance: UsbInstanceId,
    pub lifecycle_generation: u32,
    pub operation_sequence: u32,
    pub backend_id: u32,
    pub operation: UsbOperationKind,
    pub status: UsbOperationStatus,
    pub link_state: CdcState,
    pub requested_bytes: usize,
    pub completed_bytes: usize,
    pub fault: Option<UsbBackendError>,
}

impl UsbOperationReceipt {
    /// Convert this receipt to the versioned fixed host ABI without exposing any
    /// machine-local identity. The caller supplies the owning module tag and time.
    #[cfg(feature = "host-reports")]
    pub fn to_host_report(
        self,
        module_tag: u32,
        occurred_at_us: u64,
    ) -> nobro_host::BackendOperationReport {
        let mut report = nobro_host::BackendOperationReport {
            module_tag,
            backend_id: self.backend_id,
            logical_instance: u32::from(self.instance.0),
            lifecycle_generation: self.lifecycle_generation,
            operation_sequence: self.operation_sequence,
            operation_kind: self.operation as u32,
            status: self.status as u32,
            fault_code: self.fault.map_or(0, UsbBackendError::code),
            ..nobro_host::BackendOperationReport::zeroed()
        };
        report.set_occurred_at_us(occurred_at_us);
        report.finalize_diagnostic();
        report
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbOperationReport<T> {
    pub result: Result<T, UsbIoError>,
    pub receipt: UsbOperationReceipt,
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
        // UsbConfig's private fields and checked constructors make this an invariant.
        UsbIdentityPolicy::Requested => true,
        UsbIdentityPolicy::ControllerFixed => cfg.is_controller_owned(),
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
    feature = "backend-usb-serial-jtag-esp32p4",
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
    pub const USB_SERIAL_JTAG_ESP32C3: u32 = 0x4E43_3355; // "NC3U"
    pub const USB_SERIAL_JTAG_ESP32P4: u32 = 0x4E50_3455; // "NP4U"
    pub const USB_SERIAL_JTAG_ESP32S3: u32 = 0x4E53_3355; // "NS3U"
    pub const RA_USBFS: u32 = 0x4E55_5241; // "NURA" (RA4M1 / UNO R4 USBFS)
}

#[cfg(feature = "backend-usb-serial-jtag-esp32c3")]
const fn selected_usb_serial_jtag_backend_id() -> u32 {
    backend_id::USB_SERIAL_JTAG_ESP32C3
}

#[cfg(feature = "backend-usb-serial-jtag-esp32p4")]
const fn selected_usb_serial_jtag_backend_id() -> u32 {
    backend_id::USB_SERIAL_JTAG_ESP32P4
}

#[cfg(feature = "backend-usb-serial-jtag-esp32s3")]
const fn selected_usb_serial_jtag_backend_id() -> u32 {
    backend_id::USB_SERIAL_JTAG_ESP32S3
}

fn config_fingerprint(config: UsbConfig) -> u32 {
    fn add(mut hash: u32, bytes: &[u8]) -> u32 {
        for byte in bytes {
            hash ^= u32::from(*byte);
            hash = hash.wrapping_mul(0x0100_0193);
        }
        hash
    }
    let mut hash = 0x811c_9dc5;
    hash = add(hash, &config.vid.to_le_bytes());
    hash = add(hash, &config.pid.to_le_bytes());
    hash = add(hash, config.manufacturer.as_bytes());
    hash = add(hash, &[0]);
    hash = add(hash, config.product.as_bytes());
    hash = add(hash, &[0]);
    add(hash, config.serial.as_bytes())
}

const fn mount_receipt(
    instance: UsbInstanceId,
    requested: UsbConfig,
    requested_fingerprint: u32,
    advertised: UsbAdvertisedIdentity,
    backend_capabilities: UsbCapabilities,
) -> UsbMountReceipt {
    UsbMountReceipt {
        instance,
        requested_vid: requested.vid,
        requested_pid: requested.pid,
        requested_fingerprint,
        advertised,
        capabilities: backend_capabilities,
        lifecycle_generation: 1,
    }
}

/// Exact capabilities of the backend selected for this firmware image.
pub const fn capabilities() -> UsbCapabilities {
    #[cfg(feature = "backend-nrf-usbd")]
    {
        UsbCapabilities {
            backend_id: backend_id::NRF_USBD,
            identity_policy: UsbIdentityPolicy::Requested,
            mtu_bytes: CDC_PACKET_SIZE as u16,
            rx_buffer_bytes: CDC_PACKET_SIZE as u16,
            tx_buffer_bytes: CDC_PACKET_SIZE as u16,
            service: COMMON_SERVICE_LIMITS,
            lifecycle: UsbLifecycleSupport {
                permanent_singleton_mount: true,
                unmount: false,
                cancellable_operations: 0,
                reset: UsbResetSupport::ForceReenumeration,
            },
            force_reenumeration: true,
            bootloader_handoff: true,
        }
    }
    #[cfg(any(
        feature = "backend-usb-serial-jtag-esp32c3",
        feature = "backend-usb-serial-jtag-esp32p4",
        feature = "backend-usb-serial-jtag-esp32s3"
    ))]
    {
        UsbCapabilities {
            backend_id: selected_usb_serial_jtag_backend_id(),
            identity_policy: UsbIdentityPolicy::ControllerFixed,
            mtu_bytes: CDC_PACKET_SIZE as u16,
            rx_buffer_bytes: CDC_PACKET_SIZE as u16,
            tx_buffer_bytes: CDC_PACKET_SIZE as u16,
            service: COMMON_SERVICE_LIMITS,
            lifecycle: UsbLifecycleSupport {
                permanent_singleton_mount: true,
                unmount: false,
                cancellable_operations: 0,
                reset: UsbResetSupport::Unsupported,
            },
            force_reenumeration: false,
            bootloader_handoff: false,
        }
    }
    #[cfg(feature = "backend-ra-usbfs")]
    {
        UsbCapabilities {
            backend_id: backend_id::RA_USBFS,
            identity_policy: UsbIdentityPolicy::Exact(RA4M1_USB_CONFIG),
            mtu_bytes: CDC_PACKET_SIZE as u16,
            rx_buffer_bytes: CDC_PACKET_SIZE as u16,
            tx_buffer_bytes: CDC_PACKET_SIZE as u16,
            service: COMMON_SERVICE_LIMITS,
            lifecycle: UsbLifecycleSupport {
                permanent_singleton_mount: true,
                unmount: false,
                cancellable_operations: 0,
                reset: UsbResetSupport::BoardManagedDisconnect,
            },
            force_reenumeration: false,
            bootloader_handoff: false,
        }
    }
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
    /// Identity actually advertised by this backend.
    ///
    /// The compatibility default is controller-owned, which makes no configurable
    /// descriptor claim for older third-party implementations.
    fn advertised_identity(&self) -> UsbAdvertisedIdentity {
        UsbAdvertisedIdentity::ControllerOwned
    }
    /// Fingerprint of the accepted mount request.
    ///
    /// The compatibility default is zero (unknown), which avoids inventing a binding
    /// for older third-party implementations.
    fn requested_fingerprint(&self) -> u32 {
        0
    }
}

#[cfg(feature = "backend-nrf-usbd")]
mod nrf_usbd_backend;
#[cfg(feature = "backend-nrf-usbd")]
use nrf_usbd_backend::NrfUsbdCdc;
#[cfg(all(feature = "backend-nrf-usbd", feature = "nrf-timing-diagnostics"))]
pub use nrf_usbd_backend::{nrf_dma_timing, NrfDmaTiming};

#[cfg(any(
    feature = "backend-usb-serial-jtag-esp32c3",
    feature = "backend-usb-serial-jtag-esp32p4",
    feature = "backend-usb-serial-jtag-esp32s3"
))]
mod usb_serial_jtag_backend;
#[cfg(any(
    feature = "backend-usb-serial-jtag-esp32c3",
    feature = "backend-usb-serial-jtag-esp32p4",
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
    feature = "backend-usb-serial-jtag-esp32p4",
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
    state_observed: bool,
    power_restore_state: Option<CdcState>,
}

fn usb_power_veto(observed: bool, state: CdcState) -> Option<PowerVetoReason> {
    (!observed || state != CdcState::Disconnected).then_some(PowerVetoReason::UsbActive)
}

fn operation_status<T>(
    result: &Result<T, UsbIoError>,
) -> (UsbOperationStatus, Option<UsbBackendError>) {
    match result {
        Ok(_) => (UsbOperationStatus::Completed, None),
        Err(UsbIoError::Backpressure) => (UsbOperationStatus::NoProgress, None),
        Err(UsbIoError::ShortWrite { .. }) => (UsbOperationStatus::Partial, None),
        Err(UsbIoError::Backend(error)) => (UsbOperationStatus::BackendFault, Some(*error)),
        Err(UsbIoError::ProvenanceExhausted) => (UsbOperationStatus::ProvenanceExhausted, None),
        Err(_) => (UsbOperationStatus::Rejected, None),
    }
}

impl MountedUsb {
    #[inline(always)]
    fn new(backend: ActiveBackend) -> Self {
        Self {
            backend,
            state: CdcState::Disconnected,
            state_observed: false,
            power_restore_state: None,
        }
    }

    /// Last state observed by [`UsbStack::poll`].
    pub fn state(&self) -> CdcState {
        self.state
    }

    /// Typed veto to hold while the USB link is mounted and not proven
    /// disconnected. Acquire the matching executor lease before a cycle that
    /// does not use [`PowerParticipant`] composition directly.
    pub fn power_veto(&self) -> Option<PowerVetoReason> {
        usb_power_veto(self.state_observed, self.state)
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

/// Opt-in operation provenance around one mounted stack.
///
/// Keeping the tracker outside [`MountedUsb`] means compatibility mounts pay no
/// RAM or stack price for host-report attribution they do not request.
pub struct ReportedUsb {
    mounted: MountedUsb,
    instance: UsbInstanceId,
    lifecycle_generation: u32,
    operation_sequence: u32,
}

impl ReportedUsb {
    fn new(mounted: MountedUsb, receipt: UsbMountReceipt) -> Self {
        Self {
            mounted,
            instance: receipt.instance,
            lifecycle_generation: receipt.lifecycle_generation,
            operation_sequence: 0,
        }
    }

    pub const fn instance(&self) -> UsbInstanceId {
        self.instance
    }

    pub const fn lifecycle_generation(&self) -> u32 {
        self.lifecycle_generation
    }

    pub fn mounted(&self) -> &MountedUsb {
        &self.mounted
    }

    pub fn mounted_mut(&mut self) -> &mut MountedUsb {
        &mut self.mounted
    }

    pub fn into_inner(self) -> MountedUsb {
        self.mounted
    }

    fn begin_operation(&mut self) -> Option<u32> {
        let next = self.operation_sequence.checked_add(1)?;
        self.operation_sequence = next;
        Some(next)
    }

    fn operation_receipt<T>(
        &self,
        operation_sequence: u32,
        operation: UsbOperationKind,
        requested_bytes: usize,
        result: &Result<T, UsbIoError>,
        completed_bytes: usize,
    ) -> UsbOperationReceipt {
        let (status, fault) = operation_status(result);
        UsbOperationReceipt {
            instance: self.instance,
            lifecycle_generation: self.lifecycle_generation,
            operation_sequence,
            backend_id: self.mounted.backend.backend_id(),
            operation,
            status,
            link_state: self.mounted.state,
            requested_bytes,
            completed_bytes,
            fault,
        }
    }

    pub fn write_all_reported(&mut self, data: &[u8]) -> UsbOperationReport<()> {
        let Some(sequence) = self.begin_operation() else {
            let result = Err(UsbIoError::ProvenanceExhausted);
            return UsbOperationReport {
                receipt: self.operation_receipt(0, UsbOperationKind::Write, data.len(), &result, 0),
                result,
            };
        };
        let result = self.mounted.write_all(data);
        let completed = match result {
            Ok(()) => data.len(),
            Err(UsbIoError::ShortWrite { accepted, .. }) => accepted,
            _ => 0,
        };
        UsbOperationReport {
            receipt: self.operation_receipt(
                sequence,
                UsbOperationKind::Write,
                data.len(),
                &result,
                completed,
            ),
            result,
        }
    }

    pub fn read_available_reported(&mut self, buffer: &mut [u8]) -> UsbOperationReport<usize> {
        let Some(sequence) = self.begin_operation() else {
            let result = Err(UsbIoError::ProvenanceExhausted);
            return UsbOperationReport {
                receipt: self.operation_receipt(
                    0,
                    UsbOperationKind::Read,
                    buffer.len(),
                    &result,
                    0,
                ),
                result,
            };
        };
        let result = self.mounted.read_available(buffer);
        let completed = result.unwrap_or(0);
        UsbOperationReport {
            receipt: self.operation_receipt(
                sequence,
                UsbOperationKind::Read,
                buffer.len(),
                &result,
                completed,
            ),
            result,
        }
    }

    pub fn reset_reported(&mut self) -> UsbOperationReport<()> {
        let Some(sequence) = self.begin_operation() else {
            let result = Err(UsbIoError::ProvenanceExhausted);
            return UsbOperationReport {
                receipt: self.operation_receipt(0, UsbOperationKind::Reset, 0, &result, 0),
                result,
            };
        };
        let result = self
            .mounted
            .force_reenumeration()
            .map_err(UsbIoError::Backend);
        UsbOperationReport {
            receipt: self.operation_receipt(sequence, UsbOperationKind::Reset, 0, &result, 0),
            result,
        }
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
        self.state_observed = true;
    }

    /// Re-arm the existing RA4M1 controller after the board-level mux is routed back to
    /// native USB. The next poll observes the new enumeration state.
    pub fn reconnect_link(&mut self) {
        self.backend.reconnect();
        self.state = CdcState::Disconnected;
        self.state_observed = false;
    }
}

impl PowerParticipant for MountedUsb {
    fn constrain_mode(&self, requested: PowerMode) -> PowerMode {
        if self.power_veto().is_some() {
            requested.shallower(PowerMode::Idle)
        } else {
            requested
        }
    }

    fn vetoes(&self, requested: PowerMode) -> PowerVetoMask {
        if requested.depth() > PowerMode::Idle.depth() && self.power_veto().is_some() {
            PowerVetoMask::from_reason(PowerVetoReason::UsbActive)
        } else {
            PowerVetoMask::default()
        }
    }

    fn prepare_power(
        &mut self,
        mode: PowerMode,
        _system_off_wake: Option<SystemOffWake>,
    ) -> Result<(), PowerHookError> {
        if mode.depth() > PowerMode::Idle.depth() && self.power_veto().is_some() {
            return Err(PowerHookError {
                source: 0x5553,
                code: 1,
            });
        }
        self.power_restore_state = Some(self.state);
        Ok(())
    }

    fn rollback_power(&mut self, _mode: PowerMode) -> Result<(), PowerHookError> {
        self.power_restore_state = None;
        Ok(())
    }

    fn restore_power(&mut self, _effective: PowerMode) -> Result<(), PowerHookError> {
        if self.power_restore_state.take().is_none() {
            return Err(PowerHookError {
                source: 0x5553,
                code: 2,
            });
        }
        // Polling is the Wave 80U restoration path: it observes VBUS, completes
        // the nRF LOWPOWER exit/controller reset when needed, and publishes the
        // resulting CDC state only after that backend work has run.
        let _ = self.poll();
        if self.backend.backend_fault().is_some() {
            return Err(PowerHookError {
                source: 0x5553,
                code: 3,
            });
        }
        Ok(())
    }
}

impl UsbStack for MountedUsb {
    fn poll(&mut self) -> CdcState {
        self.state = self.backend.poll();
        self.state_observed = true;
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
            self.state_observed = false;
        }
        result
    }

    fn poll_bootloader_handoff(&mut self) -> Result<bool, UsbBackendError> {
        let result = self.backend.poll_bootloader_handoff();
        self.state = CdcState::Disconnected;
        self.state_observed = false;
        result
    }

    fn configured(&self) -> bool {
        self.state == CdcState::Configured
    }

    fn backend_id(&self) -> u32 {
        self.backend.backend_id()
    }

    fn advertised_identity(&self) -> UsbAdvertisedIdentity {
        self.backend.advertised_identity()
    }

    fn requested_fingerprint(&self) -> u32 {
        self.backend.requested_fingerprint()
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

#[inline(never)]
fn try_claim_mount(cfg: &UsbConfig) -> Result<(), UsbMountError> {
    if !config_supported(cfg) {
        return Err(UsbMountError::UnsupportedConfig);
    }
    if !MOUNTED.claim() {
        return Err(UsbMountError::AlreadyMounted);
    }
    Ok(())
}

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
#[inline(always)]
fn mount_backend(cfg: &UsbConfig) -> ActiveBackend {
    NrfUsbdCdc::mount(cfg)
}

#[cfg(any(
    feature = "backend-usb-serial-jtag-esp32c3",
    feature = "backend-usb-serial-jtag-esp32p4",
    feature = "backend-usb-serial-jtag-esp32s3"
))]
#[inline(always)]
fn mount_backend(cfg: &UsbConfig) -> ActiveBackend {
    UsbSerialJtagCdc::mount(cfg)
}

#[cfg(feature = "backend-ra-usbfs")]
#[inline(always)]
fn mount_backend(cfg: &UsbConfig) -> ActiveBackend {
    RaUsbfsCdc::mount(cfg)
}

/// Try to mount the USB stack selected for this board.
///
/// Configuration support is checked before the permanent process-wide claim and before
/// any backend touches hardware. Exactly one `backend-*` feature must be enabled.
/// The receipt is returned beside, rather than stored inside, the mounted stack so
/// applications that need a named composition can retain it while compatibility callers
/// pay no runtime-state cost.
pub fn try_mount_instance(
    instance: UsbInstanceId,
    cfg: &UsbConfig,
) -> Result<(MountedUsb, UsbMountReceipt), UsbMountError> {
    let policy = identity_policy();
    try_mount_with(policy, cfg, &MOUNTED, || {
        let backend = mount_backend(cfg);
        let receipt = mount_receipt(
            instance,
            *cfg,
            backend.requested_fingerprint(),
            backend.advertised_identity(),
            capabilities(),
        );
        (MountedUsb::new(backend), receipt)
    })
}

/// Mount one named logical stack with opt-in operation provenance.
///
/// The returned wrapper owns its tracker, so receipts cannot accidentally be
/// attributed to a different mounted controller. Compatibility mounts retain
/// the smaller [`MountedUsb`] runtime object.
pub fn try_mount_reported_instance(
    instance: UsbInstanceId,
    cfg: &UsbConfig,
) -> Result<(ReportedUsb, UsbMountReceipt), UsbMountError> {
    let (mounted, receipt) = try_mount_instance(instance, cfg)?;
    Ok((ReportedUsb::new(mounted, receipt), receipt))
}

/// Try to mount the conventional primary logical stack.
pub fn try_mount(cfg: &UsbConfig) -> Result<MountedUsb, UsbMountError> {
    try_claim_mount(cfg)?;
    Ok(MountedUsb::new(mount_backend(cfg)))
}

/// Mount the USB stack selected for this board.
///
/// This panic-on-error wrapper preserves the original API. New firmware should prefer
/// [`try_mount`] so an unsupported fixed descriptor or duplicate mount is explicit.
#[track_caller]
#[inline(always)]
pub fn mount(cfg: &UsbConfig) -> MountedUsb {
    if !config_supported(cfg) {
        panic!("the selected USB backend does not support the requested UsbConfig");
    }
    if !MOUNTED.claim() {
        panic!("a USB backend can only be mounted once");
    }
    MountedUsb::new(mount_backend(cfg))
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use super::{
        capabilities, config_fingerprint, config_supported, flush_with, identity_policy,
        mount_receipt, operation_status, policy_supports_config, try_mount_with, usb_power_veto,
        write_exact_with, ActiveBackend, CdcState, MountClaim, MountedUsb, ReportedUsb,
        UsbAdvertisedIdentity, UsbBackendError, UsbConfig, UsbIdentityPolicy, UsbInstanceId,
        UsbIoError, UsbMountError, UsbOperationStatus, UsbResetSupport, UsbStack, CDC_PACKET_SIZE,
    };
    #[cfg(feature = "host-reports")]
    use super::{UsbOperationKind, UsbOperationReceipt};
    use nobro_power::PowerVetoReason;

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
    fn usb_power_veto_fails_closed_until_disconnect_is_observed() {
        assert_eq!(
            usb_power_veto(false, CdcState::Disconnected),
            Some(PowerVetoReason::UsbActive)
        );
        for state in [
            CdcState::Default,
            CdcState::Addressed,
            CdcState::Configured,
            CdcState::Suspended,
        ] {
            assert_eq!(
                usb_power_veto(true, state),
                Some(PowerVetoReason::UsbActive)
            );
        }
        assert_eq!(usb_power_veto(true, CdcState::Disconnected), None);
    }

    #[test]
    fn identity_policy_preflight_distinguishes_requested_exact_and_fixed() {
        let expected = UsbConfig::new(1, 2, "maker", "product", "serial");
        let other = UsbConfig::new(1, 3, "maker", "product", "serial");
        assert!(policy_supports_config(UsbIdentityPolicy::Requested, &other));
        assert!(policy_supports_config(
            UsbIdentityPolicy::Requested,
            &UsbConfig::new(1, 3, "", "product", "serial")
        ));
        assert!(!policy_supports_config(
            UsbIdentityPolicy::ControllerFixed,
            &other
        ));
        assert!(policy_supports_config(
            UsbIdentityPolicy::ControllerFixed,
            &UsbConfig::controller_owned()
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
    #[should_panic(expected = "USB vendor id must be nonzero")]
    fn requested_config_rejects_reserved_vendor_id_at_construction() {
        let _ = UsbConfig::new(0, 3, "maker", "product", "serial");
    }

    #[test]
    #[should_panic(expected = "USB manufacturer string is too long")]
    fn requested_config_rejects_oversize_descriptor_at_construction() {
        let _ = UsbConfig::new(
            1,
            3,
            "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            "product",
            "serial",
        );
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
            feature = "backend-usb-serial-jtag-esp32p4",
            feature = "backend-usb-serial-jtag-esp32s3"
        ))]
        {
            assert_eq!(identity_policy(), UsbIdentityPolicy::ControllerFixed);
            assert!(!config_supported(&arbitrary));
            assert!(config_supported(&UsbConfig::controller_owned()));
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
    fn selected_backend_capability_receipt_is_exact() {
        let caps = capabilities();
        assert_eq!(caps.identity_policy, identity_policy());
        assert_eq!(usize::from(caps.mtu_bytes), CDC_PACKET_SIZE);
        assert!(caps.rx_buffer_bytes >= caps.mtu_bytes);
        assert!(caps.tx_buffer_bytes >= caps.mtu_bytes);
        assert_eq!(caps.service.backend_polls_per_call, 1);
        assert_eq!(caps.service.packets_per_io_call, 1);
        assert_eq!(caps.service.hidden_retry_packets, 0);
        assert!(caps.lifecycle.permanent_singleton_mount);
        assert!(!caps.lifecycle.unmount);
        assert_eq!(caps.lifecycle.cancellable_operations, 0);
        #[cfg(feature = "backend-nrf-usbd")]
        assert_eq!(caps.lifecycle.reset, UsbResetSupport::ForceReenumeration);
        #[cfg(feature = "backend-ra-usbfs")]
        assert_eq!(
            caps.lifecycle.reset,
            UsbResetSupport::BoardManagedDisconnect
        );
        #[cfg(any(
            feature = "backend-usb-serial-jtag-esp32c3",
            feature = "backend-usb-serial-jtag-esp32p4",
            feature = "backend-usb-serial-jtag-esp32s3"
        ))]
        assert_eq!(caps.lifecycle.reset, UsbResetSupport::Unsupported);

        let requested = match identity_policy() {
            UsbIdentityPolicy::Requested => {
                UsbConfig::new(0x1234, 0x5678, "maker", "product", "serial")
            }
            UsbIdentityPolicy::Exact(exact) => exact,
            UsbIdentityPolicy::ControllerFixed => UsbConfig::controller_owned(),
        };
        let advertised = match identity_policy() {
            UsbIdentityPolicy::Requested => UsbAdvertisedIdentity::Requested(requested),
            UsbIdentityPolicy::Exact(exact) => UsbAdvertisedIdentity::Exact(exact),
            UsbIdentityPolicy::ControllerFixed => UsbAdvertisedIdentity::ControllerOwned,
        };

        let receipt = mount_receipt(
            UsbInstanceId(7),
            requested,
            config_fingerprint(requested),
            advertised,
            capabilities(),
        );
        assert_eq!(receipt.instance, UsbInstanceId(7));
        assert_eq!(receipt.requested_vid, requested.vid);
        assert_eq!(receipt.requested_pid, requested.pid);
        assert_eq!(receipt.requested_fingerprint, config_fingerprint(requested));
        assert_ne!(
            receipt.requested_fingerprint,
            config_fingerprint(UsbConfig::new(0x1234, 0x5678, "maker", "other", "serial"))
        );
        assert_eq!(receipt.advertised, advertised);
        assert_eq!(receipt.capabilities, caps);
        assert_eq!(receipt.lifecycle_generation, 1);
    }

    #[test]
    fn compatibility_mount_has_no_operation_provenance_storage_tax() {
        assert!(core::mem::size_of::<MountedUsb>() <= core::mem::size_of::<ActiveBackend>() + 8);
        assert!(core::mem::size_of::<ReportedUsb>() > core::mem::size_of::<MountedUsb>());
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
    fn operation_status_preserves_fault_and_progress_truth() {
        assert_eq!(
            operation_status(&Err::<(), _>(UsbIoError::Backend(
                UsbBackendError::Unsupported
            ))),
            (
                UsbOperationStatus::BackendFault,
                Some(UsbBackendError::Unsupported)
            )
        );
        assert_eq!(
            operation_status(&Err::<(), _>(UsbIoError::ShortWrite {
                accepted: 1,
                requested: 2
            })),
            (UsbOperationStatus::Partial, None)
        );
        assert_eq!(
            operation_status(&Err::<(), _>(UsbIoError::ProvenanceExhausted)),
            (UsbOperationStatus::ProvenanceExhausted, None)
        );

        #[cfg(feature = "host-reports")]
        {
            let host = UsbOperationReceipt {
                instance: UsbInstanceId(4),
                lifecycle_generation: 3,
                operation_sequence: 9,
                backend_id: 0x5542_0004,
                operation: UsbOperationKind::Read,
                status: UsbOperationStatus::BackendFault,
                link_state: CdcState::Configured,
                requested_bytes: 64,
                completed_bytes: 0,
                fault: Some(UsbBackendError::InTransferTimeout { endpoint: 2 }),
            }
            .to_host_report(7, 0x0000_0002_0000_0001);
            assert_eq!(host.backend_id, 0x5542_0004);
            assert_eq!(host.logical_instance, 4);
            assert_eq!(host.operation_sequence, 9);
            assert_eq!(host.fault_code, 0x102);
            assert_eq!(host.occurred_at_us(), 0x0000_0002_0000_0001);
            assert_eq!(host.status(), nobro_host::ReportStatus::Pass);
        }
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
