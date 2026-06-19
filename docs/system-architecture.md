# NobroRTOS System Architecture

NobroRTOS is a layered embedded RTOS architecture focused on maintainability,
board compatibility, modular growth, bounded memory, and diagnosable recovery.

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
| SAL | `airon-sal` | Stable service traits |
| Kernel | `airon-kernel` | Admission, quota, IPC, alarms, recovery, health, reports |
| HAL | `airon-hal` | Board profiles, platform traits, leases, timers, PWM, bus, capture |
| Host | `airon-host`, `host/nobro-host-contract.json` | Report decoding and external contracts |

## Compatibility Strategy

NobroRTOS follows the same broad lesson as Zephyr devicetree: hardware should be
described as structured data that can be validated before driver code relies on
it. The current implementation starts with `BoardDesc`, board features, memory
scripts, and host-readable board profile reports.

Future board ports should add:

- a board descriptor
- capacity budgets
- critical pin declarations
- exactly one board feature
- a linker layout
- host report coverage

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

Any future allocator must be feature-gated, documented, and excluded from
hard-realtime paths.

## References

- Zephyr devicetree documentation: https://docs.zephyrproject.org/latest/build/dts/index.html
- Embassy project: https://embassy.dev/
- Tock design documentation: https://www.tockos.org/documentation/design/
- seL4 MCS tutorial: https://docs.sel4.systems/Tutorials/mcs.html
