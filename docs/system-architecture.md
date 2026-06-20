# NobroRTOS System Architecture

NobroRTOS is a layered embedded RTOS architecture focused on maintainability,
board compatibility, modular growth, bounded memory, AI robotics integration,
and diagnosable recovery.

## Architectural Principles

1. Keep the kernel small and contractual.
2. Keep hardware facts in board data, not application code.
3. Keep device integration behind SAL adapters.
4. Keep hot paths static and bounded.
5. Keep recovery decisions visible through reports.
6. Keep every new hardware-facing feature backed by a software gate.

## Layers

| Layer | Crate or path | Responsibility |
| ----- | ------------- | -------------- |
| App | `core/apps/*` | Compose board, adapters, manifest, startup graph, and runtime |
| Adapter | `core/adapters/*` | Translate devices or libraries into SAL traits |
| SAL | `airon-sal` | Stable service traits for hardware, communication, AI, and edge services |
| Kernel | `airon-kernel` | Admission, quota, IPC, alarms, recovery, health, reports |
| HAL | `airon-hal` | Board profiles, platform traits, leases, timers, PWM, bus, capture |
| Host | `airon-host`, `host/nobro-host-contract.json` | Report decoding and external contracts |

## Compatibility Strategy

NobroRTOS follows the same broad lesson as Zephyr devicetree: hardware should be
described as structured data that can be validated before driver code relies on
it. The current implementation starts with `BoardDesc`, board features, memory
scripts, and host-readable board profile reports.
`BOARD_PROFILE_FIXTURES` and `BOARD_PACKAGE_FIXTURES` keep the current board
features reviewable from one host build, which makes new board ports easier to
compare before hardware-specific validation begins.

Future board ports should add:

- a board descriptor
- a valid board package
- capacity budgets
- critical pin declarations
- a `HardwareCapabilitySet` through `HalCompatibility`
- exactly one board feature
- a linker layout
- host report coverage

`BoardPackage` is the software gate for those facts. It validates non-empty
identifiers, aligned flash origin, non-empty flash/RAM regions, usable capacity
budgets, and distinct critical pins before a port becomes a recommended target.
Firmware can export `NOBRO_BOARD_PACKAGE_REPORT` so host tooling can inspect the
same contract before manifest and adapter diagnostics.
With the `airon-kernel/hal-profile` feature, apps can derive `SystemProfile`
from `BoardPackage`, which keeps manifest and admission budgets aligned with
the selected board package.

## AI And Robotics Bridges

AI workloads are treated as RTOS-managed modules, not as private background
runtimes. A local TinyML model, an attached accelerator, a companion computer,
or a third-party API should enter the system through adapter descriptors,
capability bits, fixed budgets, caller-owned buffers, timeout policy, and
host-readable compatibility reports.

`AiInferenceSal` is the first SAL contract for this direction. It models a
bounded inference request and result without requiring heap ownership inside the
adapter. Hard-realtime control loops should consume the last valid inference
state or a degraded fallback state instead of blocking on inference.

ROS and micro-ROS compatibility belongs at the bridge layer. NobroRTOS should
absorb ROS 2's topic, service, action, and parameter concepts, but map them into
bounded queues, fixed request/response records, action state records, and
kernel-owned configuration. DDS, XRCE-DDS, agents, and custom transports should
stay behind `StreamSal` or `RadioSal` adapters rather than becoming kernel
dependencies.

## Static Async Direction

Embassy demonstrates that embedded async can stay allocation-free and efficient.
NobroRTOS keeps this direction by using fixed task tables, explicit periods,
deadline budgets, mailbox backpressure, and no allocator on critical paths.

## Isolation And Mixed Criticality

Tock's component isolation and seL4 MCS's mixed-criticality work both reinforce
the same rule: critical work needs explicit boundaries and bounded operations.
NobroRTOS maps that rule into:

- module criticality
- capability requirements and ownership
- quota-ledger accounting
- deadline contracts
- degraded-mode planning
- fixed event and health reports
- bounded AI and robotics bridge contracts

## Recovery Model

Recovery is module-scoped first:

1. classify the fault
2. update health counters
3. record a bounded event
4. choose an action
5. transition lifecycle state
6. export the result through reports

Disabled modules lose mailbox traffic, alarms, quota reservations, watchdog
registrations, and runtime authorization. Repeated disable commands are
idempotent at the runtime API boundary.

## Memory Discipline

Default rules:

- no heap on hot paths
- fixed-capacity manifests, graphs, quota ledgers, mailboxes, alarms, logs, and
  reports
- `Sample` tickets instead of cross-crate heap buffers
- compile-time feature selection instead of runtime plugin loading
- explicit cleanup when modules are disabled
- caller-owned or pool-owned buffers for AI input/output
- fixed message history for ROS-style bridge queues

Any future allocator must be feature-gated, documented, and excluded from
hard-realtime paths.

## References

- Zephyr devicetree documentation: https://docs.zephyrproject.org/latest/build/dts/index.html
- Embassy project: https://embassy.dev/
- Tock design documentation: https://www.tockos.org/documentation/design/
- seL4 MCS tutorial: https://docs.sel4.systems/Tutorials/mcs.html
