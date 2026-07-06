# NobroRTOS modular hardware compatibility

NobroRTOS keeps board and vendor differences behind **mountable backends**: a
board selects one implementation of a common trait by a Cargo feature, and app
code never names the concrete stack. This is how NobroRTOS stays compatible
with the ArduinoNRF ecosystem and, per board, with other vendor libraries
without forking the apps.

## Reference: ArduinoNRF Layer 0

The nRF52840 profiles run the ArduinoNRF core's Layer 0 by default: native
`NrfUsbd`, NimBLE, GDB stub, and peripheral drivers. NobroRTOS mirrors that
default so boards can behave like stock ArduinoNRF targets, while still moving
to ArduinoNRF's own stacks by swapping a feature.

## USB - implemented (`crates/nobro_usb`)

`UsbStack` trait + `mount()`; a board picks one backend:

| feature | backend | status |
| --- | --- | --- |
| `backend-nrf-usbd` (default) | vendored `nrf-usbd` + `usbd-serial` CDC | working on the S140-compatible profile |
| `backend-tinyusb` | TinyUSB (C) via FFI | mountable scaffold; `tud_*` FFI glue is the follow-up |
| `backend-taichiusb` | ArduinoNRF's TaichiUSB (Layer 0) | mountable scaffold; C-ABI shim to the Arduino core is the follow-up |

`usb_stack_demo` consumes only `mount()` + `UsbStack` and builds for all three
backends. Swapping the whole USB stack is a one-line feature change, no app
edits.

## Radio / BLE / Zigbee - same pattern, planned

The mountable-backend shape extends to wireless, each behind its own trait:

- **BLE**: a `BleStack` trait with backends `nimble` (ArduinoNRF default) and
  `nrf-softdevice` (S140-compatible layout). The existing nRF `Radio` driver is
  the raw-radio backend.
- **Zigbee / 802.15.4**: a `RadioCoprocessor` trait with backends such as UART
  co-processors and, later, the nRF on-chip RADIO running Nordic's official
  Zigbee sidecar firmware.

Each backend is `no_std`, feature-selected, and swappable per
`core/boards/*/board.json`, so a board's wireless identity is data plus one
feature, not scattered `#[cfg]`s.

## Why mountable, not `#[cfg]` sprinkled

One trait + one `mount()` per subsystem means apps are backend-agnostic and
portable; a new board is a data drop plus a backend choice; and vendor stacks
are integrated once, behind the subsystem boundary, instead of leaking into
every app. This is the same discipline the kernel already applies to leases and
capabilities, extended to the USB and wireless vendor layer.
