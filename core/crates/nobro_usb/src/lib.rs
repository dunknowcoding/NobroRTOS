//! Modular, mountable USB device stack for NobroRTOS.
//!
//! A board mounts exactly one backend behind the [`UsbStack`] trait, chosen at build time
//! by a `backend-*` cargo feature. The app never names a concrete stack - it calls
//! [`mount`] and talks CDC bytes. Only implemented backends are selectable; placeholder
//! stacks are deliberately not advertised as working features.
//!
//! ```ignore
//! let cfg = UsbConfig::new(0x1209, 0x0001, "NiusRobotLab", "NobroRTOS CDC", "NBRO1");
//! let mut usb = nobro_usb::mount(&cfg);
//! loop {
//!     if usb.poll() == CdcState::Configured {
//!         let mut b = [0u8; 64];
//!         let n = usb.read(&mut b);
//!         usb.write(&b[..n]);
//!     }
//! }
//! ```
#![no_std]

use portable_atomic::{AtomicBool, Ordering};

#[cfg(not(any(
    feature = "backend-nrf-usbd",
    feature = "backend-usb-serial-jtag",
    feature = "backend-ra-usbfs"
)))]
compile_error!("exactly one USB backend feature must be enabled");

#[cfg(any(
    all(
        feature = "backend-nrf-usbd",
        any(feature = "backend-usb-serial-jtag", feature = "backend-ra-usbfs")
    ),
    all(feature = "backend-usb-serial-jtag", feature = "backend-ra-usbfs")
))]
compile_error!("USB backend features are mutually exclusive");

/// Enumeration progress of the CDC device, backend-agnostic.
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
}

/// Device identity + strings a backend advertises during enumeration.
#[derive(Clone, Copy)]
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

/// Backend identity tags (surfaced in diagnostics / NOBRO reports).
pub mod backend_id {
    pub const NRF_USBD: u32 = 0x4E55_5246; // "NURF"
    pub const TINYUSB: u32 = 0x4E55_5449; // "NUTI"
    pub const TAICHIUSB: u32 = 0x4E55_5443; // "NUTC"
    pub const USB_SERIAL_JTAG: u32 = 0x4E55_534A; // "NUSJ" (ESP32-C3/S3 fixed-function)
    pub const RA_USBFS: u32 = 0x4E55_5241; // "NURA" (RA4M1 / UNO R4 USBFS)
}

/// The mountable USB device surface. One backend implements this per board.
pub trait UsbStack {
    /// Service the stack once (call frequently / from the USB IRQ) and report progress.
    fn poll(&mut self) -> CdcState;
    /// Write bytes to the CDC IN endpoint; returns how many were accepted.
    fn write(&mut self, data: &[u8]) -> usize;
    /// Read bytes from the CDC OUT endpoint; returns how many were read (0 if none).
    fn read(&mut self, buf: &mut [u8]) -> usize;
    /// True once the device has reached [`CdcState::Configured`] at least once.
    fn configured(&self) -> bool;
    /// Which backend is mounted (see [`backend_id`]).
    fn backend_id(&self) -> u32;
}

#[cfg(feature = "backend-nrf-usbd")]
mod nrf_usbd_backend;
#[cfg(feature = "backend-nrf-usbd")]
pub use nrf_usbd_backend::NrfUsbdCdc;

#[cfg(feature = "backend-usb-serial-jtag")]
mod usb_serial_jtag_backend;
#[cfg(feature = "backend-usb-serial-jtag")]
pub use usb_serial_jtag_backend::UsbSerialJtagCdc;

#[cfg(feature = "backend-ra-usbfs")]
mod ra_usbfs_backend;
#[cfg(feature = "backend-ra-usbfs")]
pub use ra_usbfs_backend::{RaUsbfsCdc, Stage};

/// Mount the USB stack backend selected for this board and return it as a `UsbStack`.
/// Exactly one `backend-*` feature must be enabled.
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

fn claim_mount() {
    static MOUNTED: MountClaim = MountClaim::new();
    assert!(MOUNTED.claim(), "a USB backend can only be mounted once");
}

#[cfg(test)]
mod tests {
    use super::MountClaim;

    #[test]
    fn global_mount_contract_is_permanent() {
        let claim = MountClaim::new();
        assert!(claim.claim());
        assert!(!claim.claim());
    }
}

#[cfg(feature = "backend-nrf-usbd")]
pub fn mount(cfg: &UsbConfig) -> impl UsbStack {
    claim_mount();
    NrfUsbdCdc::mount(cfg)
}

#[cfg(all(feature = "backend-usb-serial-jtag", not(feature = "backend-nrf-usbd")))]
pub fn mount(cfg: &UsbConfig) -> impl UsbStack {
    claim_mount();
    UsbSerialJtagCdc::mount(cfg)
}

#[cfg(all(
    feature = "backend-ra-usbfs",
    not(feature = "backend-nrf-usbd"),
    not(feature = "backend-usb-serial-jtag")
))]
pub fn mount(cfg: &UsbConfig) -> impl UsbStack {
    claim_mount();
    RaUsbfsCdc::mount(cfg)
}
