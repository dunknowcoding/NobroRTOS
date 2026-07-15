# nrf-usbd (NobroRTOS vendored fork)

This fork starts from upstream `nrf-usbd` 0.3.0. It is intentionally vendored because
NobroRTOS needs controller-lifecycle and diagnostics changes that are not present in the
published crate.

The maintained delta includes:

- the upstream Windows control-endpoint recovery from commit
  [`8ddc8d3`](https://github.com/nrf-rs/nrf-usbd/commit/8ddc8d3e815157c639b979e3ae2fb74d167d7281),
  which releases EP0 after the status-stage fallback so a second device-descriptor request
  is not rejected as busy;
- explicit VBUS, regulator-ready, suspend/wake, and firmware-handoff lifecycle handling,
  including post-READY session cleanup and release of inherited forced DP/DM drive before
  D+ is connected;
- nRF52840 revision-aware Nordic USBD anomaly workarounds and a factory-identity
  gate that fails closed before handoff or lifecycle writes on other silicon;
- bounded EasyDMA failure reporting and process-wide DMA-buffer ownership; and
- endpoint-allocation and control-endpoint validation used by the shared `nobro-usb`
  backend.

The Nordic power-up sequence and anomaly predicates are cross-checked against the current
[`nrfx` USBD driver](https://github.com/NordicSemiconductor/nrfx/blob/master/drivers/src/nrfx_usbd.c)
and the nRF52 product errata. Before `USBD.ENABLE`, the driver invokes a board clock-
provider hook and waits asynchronously until HFXO is running and selected;
`EVENTCAUSE.READY` only acknowledges the controller transition and is not an oscillator
request. The nRF board backend uses a request-only policy and never blindly writes
`TASKS_HFCLKSTOP`, so it cannot stop a clock that radio or SoftDevice code may share.
Host tests and target linking cover the software contracts;
successful enumeration, reconnect, and suspend/resume still require hardware validation for
each supported silicon and bootloader combination.

This fork makes `UsbBus::force_reset()` non-blocking: `Ok` means D+ detach was accepted,
and callers must continue polling the bus while the detach interval and reattachment finish.
For a one-way resident-bootloader transfer, `nobro-usb` uses the separate handoff request:
it keeps returning pending until D+ is off, odd cumulative EasyDMA parity is repaired,
`ENABLE` reads disabled, and all active errata ownership is closed. Application code must
not substitute ordinary re-enumeration before `SYSRESETREQ`.

---

[![crates.io](https://img.shields.io/crates/d/nrf-usbd.svg)](https://crates.io/crates/nrf-usbd)
[![Documentation](https://img.shields.io/docsrs/nrf-usbd)](https://docs.rs/nrf-usbd)

# `nrf-usbd`

[`usb-device`](https://github.com/rust-embedded-community/usb-device) implementation for Nordic
Semiconductor nRF microcontrollers.

## Supported microcontrollers

* `nrf52840`

The upstream README listed nRF52820/nRF52833, but this maintained fork intentionally
rejects them and unknown parts before touching USBD. Those two parts require Erratum 223's
first-enable double cycle; support must not be advertised until the fork has the matching
PAC and board integration, an asynchronous verified-disable phase, and silicon/bootloader
hardware validation. nRF5340 is not supported by this nRF52 register-block driver.

## Usage

This driver is relatively low-level, and is intended for use through a HAL library.
Such HAL library should implement `UsbPeripheral` for the corresponding USB peripheral object.
This trait declares all the peripheral properties that may vary from one device family to the other.

## Examples

See the [`nrf-hal`](https://github.com/nrf-rs/nrf-hal) for the reference HAL implementation.

See the [`example`](./example) directory for an example on how to use it standalone without a HAL.
This is discouraged, the recommended usage is through `nrf-hal`.
