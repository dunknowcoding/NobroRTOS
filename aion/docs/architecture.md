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
- Each board exposes a `BoardCapacity` so flash, RAM, sample-pool, and module
  limits can be checked before hardware bring-up.
- Each bootloader layout has an explicit Cargo feature and linker script.
- HAL feature selection must enable exactly one `platform-*` feature and one
  `board-*` feature.
- App and adapter crates must disable default features on HAL dependencies and
  re-enable board features explicitly. This keeps `board-promicro-nosd` from
  leaking into `board-nicenano-s140` builds through dependency defaults.
- Hardware parity checks read registers back into snapshot structs.
- Host scripts consume `airon-host` constants or `host/airon-host-contract.json`
  rather than duplicating magic values.
- Host tools should summarize boot diagnostics in this order: board profile,
  manifest, adapter compatibility, admission, then runtime. This keeps
  first-fault guidance stable as more reports are added.

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

The kernel owns the static `SystemManifest` model. A manifest describes each
module's criticality, capability requirements, capability ownership, memory
budget, deadline contract, and fault thresholds. It is intentionally a no-heap
data structure so host tests can validate partitioning before any firmware is
flashed.

Each manifest can produce a stable fingerprint over module IDs, criticality,
capability contracts, memory budgets, deadline contracts, and fault thresholds.
That gives host tools a compact way to compare static system graphs without
serializing the full manifest.

`SystemProfile` adds board-class limits for flash, RAM, sample-pool slots, and
module count. This lets AIRON reject a feature set that does not fit the target
board before a linker script or flashing step gets involved.

Apps should use `kernel_module_spec` when assembling manifests so kernel-owned
capabilities stay consistent across demos and board ports. Apps can seed
`StartupGraph` directly from a `SystemManifest`, then add only the dependency
edges that are specific to the application boot path.

## Fault Handling And Self-Recovery

Fault handling is intentionally small:

- `KernelError` classifies failures.
- `Action` describes recovery without allocating memory.
- `HealthMonitor` tracks per-module consecutive failures.
- `FaultThresholds` escalates from local retry, to user notification, to module
  reboot.
- `EventLog` keeps a fixed-size ring of health, recovery, overrun, manifest,
  and host events for post-fault inspection.
- `Supervisor` ties health counters and event records together so every
  recovery decision leaves a bounded audit trail.
- `Watchdog` tracks module heartbeats in software so liveness rules can be
  tested without binding AIRON to one hardware watchdog block.
- `Lifecycle` defines legal boot, running, degraded, recovering, and halted
  transitions so recovery paths are explicit and testable.
- `DegradePlanner` keeps System and HardRealtime modules enabled while shedding
  lower-criticality modules to fit board-class budgets.
- `RetryPolicy` and `RetryState` make bounded retry behavior explicit instead
  of embedding ad hoc loops in adapters.
- `FaultInjector` provides deterministic host-side failure scenarios for
  recovery tests without requiring hardware faults.
- `StartupGraph` and `StartupPlanner` make module dependency order explicit,
  map module IDs to compact dependency bits, and detect cycles before firmware
  boot logic is involved.
- `QuotaLedger` converts manifest budgets into fixed-capacity runtime
  accounting, so modules can reserve and release RAM, flash, and pool slots
  without heap allocation.
- `CapabilityGrantTable` derives runtime authorization from manifest
  requirements and ownership, keeping module access checks fixed-capacity and
  testable.
- `Mailbox` provides fixed-capacity control-message IPC; data payloads still
  move through `Sample` tickets and static pools.
- `AlarmQueue` provides no-heap one-shot and periodic software timers without
  binding app logic to a specific hardware timer block.
- `AlarmDispatch` summarizes due-alarm delivery, including partial progress and
  the first alarm blocked by mailbox backpressure, without dropping the alarm.
  Runtime code can route that blocked alarm through recovery as a deadline
  fault, keeping timer backpressure visible to health reports.
- `KvStore` defines the kernel-owned configuration contract as a fixed-capacity
  table; future flash persistence can keep the same API without adding a
  seventh SAL.
- `AdmissionController` composes manifest validation, startup ordering, and
  quota seeding into one boot-time software gate before board-specific startup
  code runs.
- `AdmissionReport` provides a fixed host-readable admission result so startup
  failures can be diagnosed without dynamic logging. It can be built from the
  same admission result used by boot code, reducing report-path drift.
- `RecoveryCoordinator` composes health, lifecycle transitions, watchdog-style
  deadline faults, and event logging into one testable recovery path.
- `HealthReport` turns supervisor snapshots into fixed-layout host-readable
  records with the same checksum discipline as eval and admission reports.
- `EventLogReport` summarizes the fixed event ring for host tools, including
  capacity, drops, and the latest event's module, severity, kind, and payload.
- `RuntimeReport` summarizes runtime control-plane state, including lifecycle
  state, mailbox pressure, alarm schedule, KV writes, quota usage, and event-log
  pressure.
- `BoardProfileReport` exports the selected platform, board package, flash
  origin, board-class budgets, and critical pins as a fixed host-readable
  record before any hardware-specific probe is needed.
- `ManifestReport` exports manifest validity, static graph fingerprint,
  required and owned capability bits, budget use, and validation error context.
- `AdapterCompatibilityReport` provides an admission-before-admission gate for
  SAL adapters. It records adapter count, required and owned capability bits,
  static budget use, and compatibility error context in a host-readable layout.
- `AdapterPreflight` keeps the first adapter assembly error so duplicate module
  IDs or fixed-capacity overflow can still be exported as compatibility reports.
- `Runtime` assembles an admitted plan with mailbox IPC, alarms, kernel KV, and
  recovery into one fixed-capacity control plane for apps and adapters. It can
  be constructed from an admitted plan or directly from a manifest plus startup
  graph, and routes software watchdog expiry through the same recovery and
  health-report path as explicit module faults. Runtime quota helpers keep
  reserve/release accounting on the admitted `QuotaLedger` so memory discipline
  continues after boot. Module recovery completion is also explicit: the
  runtime returns through driver initialization, records a healthy heartbeat,
  and only then resumes `Running`.

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

## Next RTOS Direction

The next step is not a larger kernel; it is stronger contracts:

- board manifests generated from board descriptions
- board profile reports exported by apps so host tools can verify the selected
  board class before interpreting adapter and runtime reports
- adapter manifests generated by feature selection
- adapters expose `AdapterManifest` data so app assembly can feed the kernel
  admission controller without hand-written module budgets
- adapters expose `AdapterDescriptor` summaries derived from their manifest so
  host or app compatibility checks can inspect module ID, capability bits, and
  budget without parsing adapter internals
- adapter descriptor sets can be checked before admission for duplicate module
  IDs, exclusive capability ownership conflicts, board-class budget fit, and
  module-count limits
- adapter descriptor sets should export a fixed-layout compatibility report
  before app admission so board bring-up can diagnose adapter/profile mismatch
  without hardware-specific probes
- compile-time or host-time checks for RAM, flash, capabilities, and criticality
- optional async executors with static task allocation
- health reports exported through the same host contract as eval reports
- fixed-layout health reports with checksums for J-Link, CDC, or future stream
  readers
- a small runtime facade that reduces app boot wiring while preserving explicit
  manifest, quota, capability, and recovery contracts

The current executor support is deliberately small: `TaskTable` is a fixed-size
task registry that records period, budget, criticality, due time, and overrun
statistics. It lets AIRON validate scheduling contracts before choosing a full
async executor surface.

The current observability support is equally small: `EventLog` is a no-heap ring
buffer that preserves the latest records, tracks drops, and can be copied into a
host-readable report without exposing dynamic logging dependencies to ISR or
hard-real-time code.
