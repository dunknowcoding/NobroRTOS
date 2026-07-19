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
| Scheduling | Cooperative fixed-priority execution is the default. An opt-in `preemptive` kernel feature admits bounded P-ISR interference and owns P-SLICE state; the bare-nRF-only `cortex-m-slice` port provides PSP/PendSV frame switching | P-ISR permits bounded acknowledgement/ready/event handoff, not arbitrary callbacks. P-SLICE is not a portable default or an isolation boundary. PendSV must remain at or below the process-wide BASEPRI ceiling so it cannot split a critical-section transaction; an overrun that never leaves such a section still requires watchdog recovery. The build rejects `cortex-m-slice` with `board-nicenano-s140` until a SoftDevice-NVIC integration exists; repeated-switch, lazy-FPU, and other-architecture support remains unpromoted |
| Executor accounting | Poll bookkeeping and idle-safety accounting run inline in the executor cycle | No admitted maintenance-service reserve or saturated accounting-debt model exists yet, so NobroRTOS cannot claim deferred/background accounting or lower jitter from such a service |
| Timing | Admission uses declared or measured budgets and pessimistic interference | Blocking terms and unmeasured compiler/platform paths are not formal WCET bounds |
| Release shape | Tasks may declare `phase < period` and `deadline <= period`; admission retains both while conservatively keeping worst-case interference | A useful phase reduces avoidable release bursts but does not by itself prove lower jitter or schedulability. Nano periods are limited to the wrap-safe 32-bit half-range (`0x7fff_ffff` us) |
| Critical sections | Native nRF board packages use one BASEPRI ceiling for kernel, HAL, USB, adapter, and portable-atomic critical sections; the deadline/watchdog priorities stay unmasked. The SAMD21 Cortex-M0+ port supplies a SysTick-instrumented PRIMASK provider and reports `mask_max_cycles`, `mask_max_us`, its bound, wrap state, and pass state | High-priority handlers must use lock-free handoff only. The SAMD21 measurement owns SysTick, is target-built, and has no physical result in the public evidence set; a future SAMD timebase must use another timer or replace the instrument. Other CM0(+), Xtensa, RISC-V, and platform ports retain their platform behavior until they have an equivalent measured contract |
| Async | No allocation; fixed task, timer, waiter, and channel capacities | Capacity is explicit and exhaustion is reported instead of allocating dynamically |
| Composition | One graph derives manifest, startup, task metadata, labels, and mailbox grants | Capability kinds remain a closed bit set and stable numeric module codes remain on wire formats |
| Project workflow | `nobro project` creates, explains, builds, simulates, and reports; `nobro firmware` emits nRF firmware from one declaration | Firmware generation currently covers explicit nRF52840 layouts and does not infer WCET or interrupt/DMA ownership |
| Arduino authoring | `NobroApp` declares fixed-capacity tasks/wires and previews admission | The facade does not embed the Rust executor or prove device timing |

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
| Provider | RP2350, ESP32-C3, ESP32-S3, RA4M1 | Selected typed providers; ESP32-C3 includes timebase, fixed USB-Serial-JTAG, and a separate configuration-priced Arduino persistent ADC-DMA composition; ESP32-S3 has required native build/USB host evidence plus one exact Arduino full-duplex ES8311 audio binding at 16 kHz mono signed-16, while its other time/deadline/I2C/SPI/PWM paths remain experimental; native RA4M1 includes timebase, deadline, USB, and an opt-in event-paced DMAC completion future | Full lease/event/fault parity, native physical recovery evidence, calibrated ADC accuracy, other audio configurations, and unimplemented peripherals |
| Core | ESP32-P4, SAMD21, AVR subset | Target startup/status integration where present; ESP32-P4 additionally has a configuration-priced Arduino persistent ADC-DMA composition | Native ESP32-P4 providers and portable peripheral providers for the remaining core-tier targets |

A provider row is not interchangeable with deep support. In particular, event routing
and PWM construction differ between MCU families, and a generic bus adapter still needs
a concrete board application to exercise the selected pins and peripheral instance.

The Arduino compatibility facades are separate compositions, not additions to native
tiers. They delegate clock, deadline, ADC, generic PWM, I2C, SPI, and byte I/O to the
selected installed board core. A hosted facade build proves source compatibility only;
it does not establish native-provider parity or physical timing. Generic `analogWrite`
does not establish servo-period semantics. On the native RA4M1 port, the
48 MHz 32-bit DWT clock must be sampled during active execution within approximately 89
seconds and may stop in low-power modes; the 24-bit SysTick alarm rejects one-shot delays
above approximately 349 milliseconds. Stronger long-running/sleep timing needs an
always-on timebase and chained alarm.

The plain-C Tier-C task facade is currently fixed at eight tasks and eight wire
relationships. It validates declarations through the shared Rust `AppGraph`, but the
nRF Tier-C drive loop checks releases with a tight microsecond-clock busy poll rather
than a compare/WFE sleep path. Late releases are phase-preserving and counted, not
replayed in a burst. A wire declaration derives the graph/mailbox relationship and
validates capacity metadata; it is not a payload transport. Portable tests plus Arm
target linking do not establish physical jitter, idle residence, current, or energy.

The opt-in RA4M1 event-DMA provider is a fixed GPT0 -> ICU/ELC -> DMAC0 path with
GPT1 as its independent timeout/residence counter. It supports at most 64 staged
words and exclusively claims GPT0, GPT1, DMAC0, DELSR0, and ICU/NVIC slots 30-31
for one operation. It is not a general scatter/gather engine or proof that every
RA4M1 event route works. Its status image enables configurable interrupts after
the stock bootloader handoff; ordinary integrations that leave `PRIMASK` or
`FAULTMASK` masked receive `InterruptsMasked`.

The nRF USB backend advances controller-ready, regulator `OUTPUTRDY`, detach, and wake
authorization as poll-driven lifecycle states. Only bounded register transitions are made
inside a critical section; an optional board monotonic clock supplies elapsed-time limits,
with an explicitly weaker poll-count fallback when no clock is available. Suspend makes
the data path unavailable, VBUS loss invalidates the session, and reconnect starts a fresh
controller attach rather than reusing suspended state. `UsbStack::force_reenumeration()`
exposes an explicit application detach/reattach for rate-limited recovery. EasyDMA
completion still uses finite iteration budgets inside a critical section, quarantines its
staging storage after a terminal timeout, and has no measured target interrupt-blackout
bound. Host tests and target linking do not establish initial enumeration, unplug/replug,
suspend/resume, or fault recovery on silicon; those lifecycle claims require physical
evidence for the selected silicon and bootloader combination.

The nRF deadline timer applies cadence changes at an ISR boundary, and the timer-power
sleep edge uses SEVONPEND/WFE instead of globally masking interrupts. BASEPRI ceiling
contracts are available for bounded shared-state work and reject configurations that
would mask the deadline or watchdog priority. This does not remove the separate USB
critical-section limitation above.

USB configuration is backend-dependent. `UsbConfig` is only a request: nRF generates
its descriptors from it, RA4M1 accepts only the exported fixed descriptor value, and the
ESP32-C3/S3 fixed-function controller ignores it. ESP configured-state host tests reject
the reset-high serial-empty flag and require post-probe EP1 token/OUT activity, but that
model is not physical enumeration, suspend, disconnect, or recovery evidence. Pre-probe
OUT data is boundedly discarded rather than reinterpreted, and transmit flush waits for
a post-write empty event rather than treating FIFO capacity as completion; those register
semantics still need silicon fault/recovery evidence. The public identity policy must be
checked when host-visible VID/PID/string identity matters.

Bootloader and application USB identities are independent. `UsbConfig` and forced
re-enumeration apply only to the mounted application backend; they do not select a
SoftDevice/no-SoftDevice flash layout, modify bootloader descriptors, or enter recovery
firmware. Host-visible absence alone cannot distinguish failure to execute a bootloader
from failure to enumerate its USB identity.

`nobro-wireless` supplies a bounded data plane, selected concrete transports,
and portable WiFi/BLE lifecycle contracts. UNO R4 WiFiS3 now has a
compile-only association facade with an exact unpriced binding and
zero-disabled proof; this is not a promoted physical backend. WiFiS3 uses
synchronous modem calls and vendor-managed dynamic strings, so Nobro records
an overrun only after a call returns and does not claim hard cancellation.
IP/socket integration, shared-radio arbitration, and measured vendor-stack
resource prices remain absent. Different board technologies must declare
per-instance backend selection, radio ownership, coexistence, and
vendor-managed resources before support is promoted. `ManagedLink::send_at`
checks the deadline for one immediate attempt; scheduling priority and retry
execution remain outside it. The CC2530 backend is a raw initialized 127-byte
IEEE 802.15.4 PSDU transport, while `ZIGBEE_APS` is metadata rather than an
implemented Zigbee APS stack.

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
