# AIRON Architecture Principles

This document turns the project route into engineering rules that can survive
new boards, new adapters, and long maintenance windows.

## External Lessons

AIRON borrows selectively from established and modern embedded systems:

- Zephyr uses devicetree to describe hardware and provide initial device
  configuration. AIRON keeps the same lesson, but starts smaller with
  `BoardDesc`, `BusLayout`, and explicit Cargo board features.
- Embassy shows that embedded async can be no-heap and statically allocated.
  AIRON follows the same direction: no allocator on hot paths, static sample
  pools, and compile-time feature selection.
- Tock uses Rust isolation boundaries to keep kernel components mutually
  distrustful with low overhead. AIRON applies that idea at crate and trait
  boundaries: kernel, HAL, SAL, adapters, and apps do not share private state.
- seL4 mixed-criticality work emphasizes bounded kernel operations and clear
  criticality separation. AIRON keeps deadline slots and recovery policy in the
  kernel instead of scattering them across drivers.

## Layer Boundaries

| Layer | Rule |
| ----- | ---- |
| App | Assembles features and owns policy wiring; it should not touch registers directly. |
| Adapter | Translates one device or library into SAL traits; no private scheduler or heap. |
| SAL | Stable capability surface: bus, stream, radio, actuator, sensor, crypto. |
| Kernel | Deadline slots, health, sample tickets, error policy, and eval gates. |
| HAL | Board layout, register access, event capture, PWM, bus, and leases. |

## Multi-Board Compatibility

Board compatibility must be data-first:

- Each board exposes a `BoardDesc`.
- Each bootloader layout has an explicit Cargo feature and linker script.
- Hardware parity checks read registers back into snapshot structs.
- Host scripts consume `airon-host` constants or `host/airon-host-contract.json`
  rather than duplicating magic values.

The current nRF52840 backend uses PPI. The portable HAL term is
`HalEventCapture`; do not leak "PPI" into app or adapter APIs unless the code is
nRF-specific.

## Module Partitioning

Every module should have one of these roles:

- time-critical kernel primitive
- portable HAL capability
- SAL trait definition
- thin adapter
- app composition
- host contract or validation helper

If a module wants to do two of these at once, split it before adding features.

## Fault Handling And Self-Recovery

Fault handling is intentionally small:

- `KernelError` classifies failures.
- `Action` describes recovery without allocating memory.
- `HealthMonitor` tracks per-module consecutive failures.
- `FaultThresholds` escalates from local retry, to user notification, to module
  reboot.

Recovery is module-scoped by default. Full chip reset is a last resort and
should remain outside hot-path adapters.

## Memory Discipline

The default rule is no allocator:

- use static pools for payloads
- pass `Sample` tickets instead of raw buffers across crates
- use `LeaseGuard` to avoid resource leaks
- keep ISR work bounded and defer parsing to cooperative tasks
- prefer compile-time features over runtime plugin registries

Any future heap use must be feature-gated, documented, and excluded from
hard-real-time paths.

## Maintainability Gates

Before adding a hardware-dependent feature, add at least one software gate:

- a host unit test
- a checksumed host-readable report
- a register snapshot comparison
- a board feature/linker validation
- a no-hardware stub adapter

This keeps AIRON usable even when no lab board is connected.
