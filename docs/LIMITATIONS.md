# Limitations and support boundaries

A successful build does not imply full peripheral support, declared timing is not a
formal WCET proof, and host simulation is not device execution. The machine-readable
platform matrix is `core/boards/platform_tiers.json`; adapter/library membership is
listed separately in `core/adapters/catalog.json`. Platform claims are scoped to an
exact platform, composition, and capability before they may cite a named CI gate. Host
tests establish only their modeled contracts, target builds establish only compilation,
and neither is recorded as physical evidence. Local receipts bind a session to Git HEAD,
the tracked diff, and nonignored untracked source content, but they are unsigned freshness
and omission checks rather than an adversarial attestation.

## Application model

| Boundary | Current behavior | Practical consequence |
| --- | --- | --- |
| Scheduling | Cooperative fixed-priority execution with response-time admission, fuel-bounded async, and overrun containment | A callback that never yields still needs a deadline/watchdog path to recover it; there is no general preemptive time-slicing profile |
| Timing | Admission uses declared or measured budgets and pessimistic interference | Blocking terms and unmeasured compiler/platform paths are not formal WCET bounds |
| Async | No allocation; fixed task, timer, waiter, and channel capacities | Capacity is explicit and exhaustion is reported instead of allocating dynamically |
| Composition | One graph derives manifest, startup, task metadata, labels, and mailbox grants | Capability kinds remain a closed bit set and stable numeric module codes remain on wire formats |
| Project workflow | `nobro project` creates, explains, builds, simulates, and reports; `nobro firmware` emits nRF firmware from one declaration | Firmware generation currently covers explicit nRF52840 layouts and does not infer WCET or interrupt/DMA ownership |
| Arduino authoring | `NobroApp` declares fixed-capacity tasks/channels and previews admission | The facade does not embed the Rust executor or prove device timing |

## Resources

Admission, identity, quotas, recovery, and diagnostics consume flash, RAM, stack, and
CPU. Small applications that do not need those controls may be smaller with a direct
loop. Size and timing depend on the selected profile, compiler, target, and workload;
measure the final application. Static RAM is not total RAM, so deployment review must
also include stacks and bounded arenas. Current or energy estimates require calibrated
hardware and are not inferred from software coefficients alone.

## Platform support

| Tier | Platforms | Included | Missing |
| --- | --- | --- | --- |
| Deep | nRF52840 | One native composition with implemented time, deadline, event, PWM, I2C, SPI, USB, and lease providers | Broader deep board families and physical USB fault/recovery evidence |
| Provider | RP2350, ESP32-C3, ESP32-S3, RA4M1 | Selected typed providers; ESP32-C3 includes timebase plus the fixed USB-Serial-JTAG backend; ESP32-S3 has a required hosted Xtensa build while its time/deadline/I2C/SPI/PWM paths remain experimental, and its USB state machine has required host evidence; native RA4M1 includes timebase, deadline, and USB only | Full lease/event/fault parity, physical recovery evidence, and unimplemented peripherals |
| Core | SAMD21, AVR subset | Target startup and status integration | Portable peripheral providers |

A provider row is not interchangeable with deep support. In particular, event routing
and PWM construction differ between MCU families, and a generic bus adapter still needs
a concrete board application to exercise the selected pins and peripheral instance.

The UNO R4 and ArduinoNRF facades are separate compositions, not additions to native
tiers. They delegate clock, deadline, ADC, generic PWM, I2C, SPI, and byte I/O to their
installed Arduino board cores. Hosted ArduinoNRF compilation runs on Windows against the
pinned 0.3.11 core and the exact Pro Micro nRF52840 `usbcdc=enabled` selection. Generic
`analogWrite` does not establish servo-period semantics. On the native RA4M1 port, the
48 MHz 32-bit DWT clock must be sampled during active execution within approximately 89
seconds and may stop in low-power modes; the 24-bit SysTick alarm rejects one-shot delays
above approximately 349 milliseconds. Stronger long-running/sleep timing needs an
always-on timebase and chained alarm.

The nRF USB backend has finite iteration budgets for controller-ready and EasyDMA
completion, quarantines staging storage after a terminal timeout, and reports typed
faults. The bounded waits still execute inside a critical section; their target-specific
interrupt blackout has not been measured, and host tests do not establish disconnect or
fault-recovery behavior on silicon. A future poll-driven transfer state machine plus
target timing and fault evidence are required for a stronger deadline claim.

USB configuration is backend-dependent. `UsbConfig` is only a request: nRF generates
its descriptors from it, RA4M1 accepts only the exported fixed descriptor value, and the
ESP32-C3/S3 fixed-function controller ignores it. ESP configured-state host tests reject
the reset-high serial-empty flag and require post-probe EP1 token/OUT activity, but that
model is not physical enumeration, suspend, disconnect, or recovery evidence. Pre-probe
OUT data is boundedly discarded rather than reinterpreted, and transmit flush waits for
a post-write empty event rather than treating FIFO capacity as completion; those register
semantics still need silicon fault/recovery evidence. The public identity policy must be
checked when host-visible VID/PID/string identity matters.

`nobro-wireless` currently supplies a common bounded data plane and selected concrete
transports; it does not yet supply WiFi/BLE lifecycle traits or vendor-stack feature
selection. Future stacks for different board technologies will extend that existing
domain and must declare backend exclusivity, shared-radio ownership, and vendor-managed
resources before support is promoted. `ManagedLink::send_at` checks the deadline for one
immediate attempt; scheduling priority and retry execution remain outside it. The CC2530
backend is a raw initialized 127-byte IEEE 802.15.4 PSDU transport, while `ZIGBEE_APS`
is metadata rather than an implemented Zigbee APS stack.

## Isolation, boot, and recovery

- Rust module identity is dispatcher-owned, but modules share one privileged address
  space. Per-module MPU switching and unprivileged execution are not present.
- The signed boot/update state machine is fail-closed and persistent, but production
  bootloader slots, protected keys, and provisioning transports are board integrations.
- Kernel-object cleanup is reconciled and leak-checked. The nRF HAL invalidates
  generation-tagged sessions and quiesces peripheral activity before lease reassignment;
  equivalent behavior is incomplete on other ports.
- Application-owned static state still needs explicit lifecycle cleanup.

## Validation boundary

Hosted checks cover portable tests, formatting, dependency policy, package builds, and
cross-compilation. Device behavior remains target- and application-specific; a quiet or
absent device is never interpreted as success.
