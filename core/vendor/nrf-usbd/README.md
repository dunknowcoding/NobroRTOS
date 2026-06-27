# nrf-usbd (vendored fork for NobroRTOS)

Vendored from upstream `nrf-usbd` 0.3.0 with a **single** behavioral change, needed to
run USB-CDC on **cloned nRF52840** silicon (some nice!nano-class boards — NobroRTOS
"board5").

**The fix** — `src/usbd.rs`, `UsbBus::write()`: the EP-IN busy guard reads `EPSTATUS`
and returns `WouldBlock` if the endpoint's bit is set. On cloned USBD silicon `EPSTATUS`
reads a **constant `0x00010001`** (the `EPIN0`/`EPOUT0` bits are permanently stuck set),
so for **EP0** the guard always tripped: the device descriptor was never written and
enumeration died at the first `GET_DESCRIPTOR(DEVICE)` ("unrecognized USB device"). The
fix skips the `EPSTATUS` check **for EP0 only** (`if i != 0 && ...`); EP0 is already
serialised by `busy_in_endpoints` + the inline `ENDEPIN` wait, so the check was
redundant there. Bulk/interrupt endpoints are unchanged. Search the source for
`Clone-safe` to find it.

Verified on hardware: genuine nRF52840 (board1, `who=0x71`) and clone (board5,
`who=0x70`) both enumerate a CDC port and stream the IMU eval line (`... PASS`).

---

[![crates.io](https://img.shields.io/crates/d/nrf-usbd.svg)](https://crates.io/crates/nrf-usbd)
[![Documentation](https://img.shields.io/docsrs/nrf-usbd)](https://docs.rs/nrf-usbd)

# `nrf-usbd`

[`usb-device`](https://github.com/rust-embedded-community/usb-device) implementation for Nordic
Semiconductor nRF microcontrollers.

## Supported microcontrollers

* `nrf52840`
* `nrf52833`
* `nrf52820`
* `nrf5340`, maybe?

## Usage

This driver is relatively low-level, and is intended for use through a HAL library.
Such HAL library should implement `UsbPeripheral` for the corresponding USB peripheral object.
This trait declares all the peripheral properties that may vary from one device family to the other.

## Examples

See the [`nrf-hal`](https://github.com/nrf-rs/nrf-hal) for the reference HAL implementation.

See the [`example`](./example) directory for an example on how to use it standalone without a HAL.
This is discouraged, the recommended usage is through `nrf-hal`.
