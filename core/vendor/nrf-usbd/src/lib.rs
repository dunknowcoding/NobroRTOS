//! USB peripheral driver for nRF microcontrollers.

#![no_std]
#![allow(mismatched_lifetime_syntaxes)] // generated PAC API predates the renamed rustc lint

mod errata;
mod pac;
mod usbd;

#[doc(hidden)]
pub use usbd::sanitize_handoff;
pub use usbd::Usbd;

/// Fatal controller failures that make the current USB bus instance unusable.
///
/// The driver converts these failures to [`usb_device::UsbError::InvalidState`] at the
/// `UsbBus` boundary. [`UsbPeripheral::on_fault`] lets a board retain the more specific
/// cause for diagnostics without changing `usb-device` semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbdFault {
    /// The peripheral did not assert `EVENTCAUSE.READY` within the enable budget.
    EnableTimeout,
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

/// A trait for device-specific USB peripherals. Implement this to add support for a new hardware
/// platform. Peripherals that have this trait must have the same register block as NRF52 USBD
/// peripherals.
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

    /// Samples whether the USB power domain currently observes VBUS.
    ///
    /// The default preserves generic peripheral implementations that cannot lose VBUS.
    /// A detachable USB device must override this with a bounded, non-blocking register
    /// read. The hook may run inside the driver's critical section.
    fn vbus_present() -> bool {
        true
    }
}
