//! USB peripheral driver for nRF microcontrollers.

#![no_std]
#![allow(mismatched_lifetime_syntaxes)] // generated PAC API predates the renamed rustc lint

mod errata;
mod pac;
mod usbd;

pub use usbd::Usbd;
#[doc(hidden)]
pub use usbd::{request_bootloader_handoff, sanitize_handoff, HandoffSanitization};

/// Fatal controller failures that make the current USB bus instance unusable.
///
/// The driver converts these failures to [`usb_device::UsbError::InvalidState`] at the
/// `UsbBus` boundary. [`UsbPeripheral::on_fault`] lets a board retain the more specific
/// cause for diagnostics without changing `usb-device` semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbdFault {
    /// Factory identity does not name an nRF52840 revision maintained by this fork.
    UnsupportedSilicon,
    /// The peripheral did not assert `EVENTCAUSE.READY` within the enable budget.
    EnableTimeout,
    /// The board clock provider did not make the external high-frequency oscillator
    /// available within the startup budget. USBD cannot signal correctly from HFINT.
    HfclkTimeout,
    /// The USB regulator did not assert its output-ready signal after enable.
    PowerReadyTimeout,
    /// The USB MAC did not become operational after leaving low-power mode.
    WakeTimeout,
    /// `ENABLE` did not read back disabled before its asynchronous cleanup budget expired.
    DisableTimeout,
    /// Cumulative EasyDMA parity was odd outside a controller state where Nordic's
    /// pre-disable one-byte repair transaction can be issued safely.
    ParityRepairUnavailable,
    /// An IN endpoint EasyDMA transfer did not assert `ENDEPIN[n]` within its budget.
    InDmaTimeout {
        /// Endpoint number whose transfer timed out.
        endpoint: u8,
    },
    /// An OUT endpoint EasyDMA transfer did not assert `ENDEPOUT[n]` within its budget.
    OutDmaTimeout {
        /// Endpoint number whose transfer timed out.
        endpoint: u8,
    },
    /// The process-wide EasyDMA staging storage was already claimed by another instance.
    DmaStorageAlreadyClaimed,
}

/// Board hooks for the nRF52840 USBD peripheral maintained by this fork.
///
/// A matching register layout is not sufficient to support another nRF52 part: the driver
/// rejects non-nRF52840 factory identities before touching the controller. In particular,
/// nRF52820/nRF52833 need Erratum 223, a matching PAC/board integration, and hardware
/// validation before they can be added safely.
pub unsafe trait UsbPeripheral: Send {
    /// Pointer to the register block
    const REGISTERS: *const ();

    /// Receives the first fatal fault latched by this peripheral instance.
    ///
    /// The default keeps existing `UsbPeripheral` implementations source-compatible.
    /// Implementations may retain the copy in an atomic for board-level diagnostics.
    /// This hook runs inside the driver's critical section, so it must be bounded,
    /// non-blocking, and must not attempt to enter another critical section.
    fn on_fault(_fault: UsbdFault) {}

    /// Reports whether endpoint registers and transfers are currently operational.
    ///
    /// A board backend can use this to avoid publishing a stale Configured state
    /// while the controller is enabling, suspended, waking, detaching, or faulted.
    fn on_operational_change(_operational: bool) {}

    /// Samples whether the USB power domain currently observes VBUS.
    ///
    /// Implementations must provide a bounded, non-blocking physical-status read;
    /// build-time policy must never be allowed to authorize the D+ pull-up. The hook
    /// may run inside the driver's critical section.
    fn vbus_present() -> bool;

    /// Samples whether the USB signalling regulator is ready for pull-up engagement.
    ///
    /// nRF52 board implementations must provide the non-blocking
    /// `USBREGSTATUS.OUTPUTRDY` register read required by the hardware power-up
    /// sequence. The hook may run inside the driver's critical section.
    fn power_ready() -> bool;

    /// Requests the external high-frequency oscillator required by nRF USBD.
    ///
    /// This is a board/provider hook rather than a raw driver register write so a
    /// SoftDevice or shared radio clock broker can own the actual request. It must be
    /// bounded and non-blocking; readiness is observed separately.
    fn request_hfclk() {}

    /// Reports that the external high-frequency oscillator is running and selected.
    ///
    /// The default preserves source compatibility for integrations whose platform
    /// constructor already proves this invariant. nRF board backends should override
    /// it with hardware/provider readback.
    fn hfclk_running() -> bool {
        true
    }

    /// Returns a wrapping 32-bit microsecond timestamp when one is available.
    ///
    /// The driver uses this optional clock only to bound asynchronous controller
    /// transitions. Implementations must be monotonic modulo `u32` wrapping and
    /// cheap to sample. When absent, the driver falls back to a documented count
    /// of failed `poll()` observations; it does not claim a wall-clock deadline.
    /// This hook may run inside the driver's critical section.
    fn monotonic_us_32() -> Option<u32> {
        None
    }
}
