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
| Scheduling | Cooperative fixed-priority execution is the default. An opt-in `preemptive` kernel feature admits bounded P-ISR interference and owns P-SLICE state; the bare-nRF-only `cortex-m-slice` port provides PSP/PendSV frame switching and routes a committed forced suspension to module-scoped recovery | Cooperative mode cannot forcibly suspend a non-yielding poll; it still needs an external watchdog. P-ISR permits bounded acknowledgement/ready/event handoff, not arbitrary callbacks. Privileged P-SLICE is not an isolation boundary, and every port without an explicitly verified MPU capability admission-rejects unprivileged slices. PendSV must remain at or below the process-wide BASEPRI ceiling; an overrun that never leaves such a section still requires watchdog recovery. The build rejects `cortex-m-slice` with `board-promicro-s140`; repeated-switch, lazy-FPU, and other-architecture support remains unpromoted |
| Executor accounting | Poll bookkeeping and idle-safety accounting run inline in the executor cycle | No admitted maintenance-service reserve or saturated accounting-debt model exists yet, so NobroRTOS cannot claim deferred/background accounting or lower jitter from such a service |
| Timing | Admission uses declared or measured budgets and pessimistic interference | Blocking terms and unmeasured compiler/platform paths are not formal WCET bounds |
| Release shape | Tasks may declare `phase < period` and `deadline <= period`; admission retains both while conservatively keeping worst-case interference | A useful phase reduces avoidable release bursts but does not by itself prove lower jitter or schedulability. Nano periods are limited to the wrap-safe 32-bit half-range (`0x7fff_ffff` us) |
| Critical sections | Native nRF board packages use one BASEPRI ceiling for kernel, HAL, USB, adapter, and portable-atomic critical sections; the deadline/watchdog priorities stay unmasked. The SAMD21 Cortex-M0+ port supplies a SysTick-instrumented PRIMASK provider and reports `mask_max_cycles`, `mask_max_us`, its bound, wrap state, and pass state; its separate native timebase uses RTC MODE0 | High-priority handlers must use lock-free handoff only. The SAMD21 measurement owns SysTick, is target-built, and has no physical result in the public evidence set. Other CM0(+), Xtensa, RISC-V, and platform ports retain their platform behavior until they have an equivalent measured contract |
| Async | No allocation; fixed task, timer, waiter, and channel capacities | Capacity is explicit and exhaustion is reported instead of allocating dynamically |
| Composition | One graph derives manifest, startup, task metadata, labels, and mailbox grants | Capability kinds remain a closed bit set and stable numeric module codes remain on wire formats |
| Project workflow | `nobro project` creates, explains, builds, simulates, and reports; `nobro firmware` emits native firmware from one declaration | Standalone image generation covers explicit nRF52840, RA4M1, and SAMD21 layouts. RP2350 and ESP32-C3 retain maintained-port startup because their mandatory image/runtime machinery is not duplicated. No execution budget, WCET, clock, interrupt, or DMA ownership is inferred |
| Arduino authoring | `NobroApp` declares fixed-capacity tasks/wires and previews admission | The facade does not embed the Rust executor or prove device timing |

## Resources

Admission, identity, quotas, recovery, and diagnostics consume flash, RAM, stack, and
CPU. Small applications that do not need those controls may be smaller with a direct
loop. Size and timing depend on the selected profile, compiler, target, and workload;
measure the final application. Static RAM is not total RAM. A deployment receipt must
account once, without overlap, for static sections, task stacks, provider stacks,
arenas/pools, retained heap, and vendor/controller reservations; dimensions already
contained in `.data`/`.bss` are declared as zero rather than charged twice. Current,
energy, and joule estimates still require calibrated electrical measurements and are
not inferred from software coefficients alone.

## Platform support

| Tier | Platforms | Included | Missing |
| --- | --- | --- | --- |
| Deep | none currently | This tier now requires every capability exposed by the exact board to have compiled ownership and lifecycle coverage, with an empty applicable-gap set. | The earlier subset-based promotions were withdrawn by the completeness audit instead of being grandfathered. |
| Provider | nRF52840, RA4M1, SAMD21, RP2040, RP2350, ESP32, ESP32-C3, ESP32-P4, ESP32-S3 | Exact target-built native provider sets preserve silicon-specific GPIO, interrupt, DMA, power, cache, multicore, USB, bus, and lease limits. | Each row still names one or more applicable gaps in the HAL contract; target builds and scoped physical evidence remain valid but do not establish full-board parity. |
| Constrained | Nano V3 / ATmega328P and WeMos D1 Mini / ESP8266 Arduino compositions | Exact bounded Arduino-first providers cover their published clock, deadline, GPIO, interrupt, PWM, ADC, bus, UART, power/reset, and ESP8266 Wi-Fi subsets. | Some present capabilities remain board-core-owned or lack complete lifecycle contracts. Hardware-absent USB, DMA, multicore, or isolation features are not counted as gaps. Nano reset remains application re-entry because watchdog reset can loop in the exact old bootloader. |
| Candidate-only | 8051, STM8, PIC, MSP430, STM32C0, CH32V003, C2000 DSP, and FPGA/soft-core witnesses | Exact compiler images/objects or RTL simulation prove only that the bounded kernel-lite arithmetic/state shape is representable. | No board HAL, peripheral, interrupt, timing, power, isolation, debug, synthesis/timing-closure, or physical-support claim follows from these artifacts. |

The exact market-board intake is intentionally weaker than every row above.
`discovered` records a primary board source, `cataloged` additionally records an
upstream and recovery route, and `target-compiled` passes one pinned build. None of
those states implies a board profile, HAL provider, boot/recovery proof, or physical
support. The current compiled intake is limited to ESP32-C6-DevKitM-1 and
ESP32-H2-DevKitM-1 on Arduino-ESP32 3.3.10.

A provider row is not interchangeable with deep support. In particular, event routing
and PWM construction differ between MCU families, and a generic bus adapter still needs
a concrete board application to exercise the selected pins and peripheral instance.

The Arduino compatibility facades are separate compositions, not additions to native
tiers. They delegate clock, deadline, ADC, generic PWM, I2C, SPI, and byte I/O to the
selected installed board core. A hosted facade build proves source compatibility only;
it does not establish native-provider parity or physical timing. Generic `analogWrite`
does not establish servo-period semantics. On the native RA4M1 port, AGT0 advances
from LOCO through ordinary CPU sleep and AGT1 composes long deadlines from bounded
16-bit chunks. This does not cover standby or deep standby: the portable power provider
clears `SLEEPDEEP` and exposes only state-preserving CPU sleep.

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
exposes an explicit application detach/reattach for rate-limited recovery. EasyDMA setup
and completion bookkeeping use short critical sections; completion waits run with
interrupts enabled and use a 10 ms elapsed-time limit when the board supplies the required
microsecond clock. The no-clock fallback remains a poll count, not a time guarantee.
Permanent aligned storage is retained after a terminal timeout, so late DMA cannot touch
expired caller buffers. Physical evidence applies only to its exact silicon, bootloader,
clock, and firmware composition; it does not establish every unplug timing or electrical
fault case.

The nRF deadline timer applies cadence changes at an ISR boundary, and the timer-power
sleep edge uses SEVONPEND/WFE instead of globally masking interrupts. BASEPRI ceiling
contracts are available for bounded shared-state work and reject configurations that
would mask the deadline or watchdog priority.

USB configuration is backend-dependent. `UsbConfig` is only a request: nRF generates
its descriptors from it, RA4M1 accepts only the exported fixed descriptor value, and the
ESP32-C3/S3 fixed-function controller ignores it. ESP configured-state host tests reject
the reset-high serial-empty flag and require post-probe EP1 token/OUT activity, but that
model is not physical enumeration, suspend, disconnect, or recovery evidence. Pre-probe
OUT data is boundedly discarded rather than reinterpreted, and transmit flush waits for
a post-write empty event rather than treating FIFO capacity as completion; those register
semantics still need silicon fault/recovery evidence. The public identity policy must be
checked when host-visible VID/PID/string identity matters. A successful mount exposes an
exact logical-instance receipt with a request fingerprint, advertised-identity class,
backend id, MTU, buffer/service limits, lifecycle generation, and supported recovery
operations; it does not turn a controller-owned identity into configurable descriptors.

Bootloader and application USB identities are independent. `UsbConfig` and forced
re-enumeration apply only to the mounted application backend; they do not select a
SoftDevice/no-SoftDevice flash layout, modify bootloader descriptors, or enter recovery
firmware. Host-visible absence alone cannot distinguish failure to execute a bootloader
from failure to enumerate its USB identity.

On UNO R4 WiFi, native RA4M1 USB temporarily owns the connector that normally
exposes the onboard upload/debug bridge. The port retains a bounded
never-configured fallback and accepts the session-scoped `NOBRO_BRIDGE`
command for an explicit in-band return. It does not impose an arbitrary
timeout on a healthy configured native session. The bridge-restoration mux
pulse is board-specific and is not exposed as a generic `UsbStack` operation.

`nobro-wireless` supplies a bounded data plane, selected concrete transports,
portable WiFi/BLE lifecycle contracts, and an independently mounted native
TCP/UDP/DNS contract. The Arduino implementation owns fixed socket slots and
caller-lent payload buffers, but its selected vendor stack still owns internal
heap, tasks, and controller buffers. One exact UNO R4/WiFiS3 0.6.0
workload has zero-disabled proof, state-restoring association, DNS, TCP,
leave, quiesce, and recovery evidence, and a complete RA-side/controller-image
price. The pinned Arduino-ESP32 3.3.10 backend has C3 zero-disabled proof plus
state-restoring association, DNS, TCP, leave, quiesce, and recovery evidence,
and one exact no-debug C3 workload is completely configuration-priced at
four HTTP transactions/s. Classic ESP32 additionally has two independent
state-restoring Bluedroid GATT/lifecycle campaigns under admitted WiFi traffic
and a conservative whole-composition price; no standalone WiFi baseline or
BLE-only increment is inferred. Synchronous vendor calls and managed heap/tasks
still prevent Nobro from claiming hard cancellation or allocation-free
operation. The pinned controller artifact establishes 1,180,064 B application
flash / 64,628 B static RAM and 22,288 B / three-task source minima, but its
retained/transient heap, complete task/stack reservations, and CPU remain
unmeasured. Other rates, boards,
firmware versions, socket workloads, and other shared-radio compositions
remain absent. Different board
technologies must declare
per-instance backend selection, radio ownership, coexistence, and
vendor-managed resources before support is promoted. `ManagedLink::send_at`
checks the deadline for one immediate attempt; scheduling priority and retry
execution remain outside it. The CC2530 backend is a raw initialized 127-byte
IEEE 802.15.4 PSDU transport. The optional APS data service requires an already
joined NWK backend and deliberately omits formation, joining, ZDO, security,
and fragmentation; CC2530 raw mode does not satisfy that boundary.

The exact UNO R4 ArduinoBLE 2.1.0 adapter is implemented and physically
verified. Its disabled facade has zero flash/RAM delta, and exact BLE-only plus
WiFi+BLE compositions build through ArduinoBLE's official
`HCIVirtualTransportAT` over the installed WiFiS3 board driver. Three cycles
passed GATT write/read/notify, provider disconnect, remount/recovery, stable
RA-side heap, and a connected subscribed BLE link across 15 DNS/TCP
transactions. The facade supplies the controller `HCIEND` omitted by
ArduinoBLE 2.1.0 and repairs its cleared-service retain conservatively.

This is not a hard-concurrency claim: synchronous WiFiS3 modem calls serialize
RA-side GATT servicing, so the campaign requires post-WiFi notification and
readback rather than pretending those calls are preemptible. Controller-
internal retained/transient heap, complete task/stack reservations, CPU, and
a complete shared-controller configuration price remain unmeasured.
ArduinoBLE owns process-wide HCI/GATT objects and dynamic
allocation, so Nobro admits only one mounted facade.

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
