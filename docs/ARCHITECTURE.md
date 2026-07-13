# Architecture

How the system is layered, the engineering rules that keep it maintainable,
and the two modularity mechanisms everything mounts through: backend features
(USB, sensors) and the Universal Driver Interface.

## System architecture

NobroRTOS is a layered embedded RTOS architecture focused on maintainability,
board compatibility, modular growth, bounded memory, AI robotics integration,
and diagnosable recovery.

### Architectural Principles

1. Keep the kernel small and contractual.
2. Keep hardware facts in board data, not application code.
3. Keep device integration behind SAL adapters.
4. Keep hot paths static and bounded.
5. Keep recovery decisions visible through reports.
6. Keep every new hardware-facing feature backed by a software gate.

### Layers

| Layer | Crate or path | Responsibility |
| ----- | ------------- | -------------- |
| App | `core/apps/<use-case>/*` | Compose board, adapters, manifest, startup graph, and runtime |
| Adapter | `core/adapters/<domain>/*` | Translate devices or libraries into SAL traits |
| Domain | `core/crates/nobro_<domain>` | Shared bounded contracts; no board or external-library ownership |
| SAL | `nobro-sal` | Stable service traits for hardware, communication, AI, and edge services |
| Kernel | `nobro-kernel` | Admission, quota, IPC, alarms, recovery, health, reports |
| HAL | `nobro-hal` | Board profiles, platform traits, leases, timers, PWM, bus, capture |
| Host | `nobro-host`, `host/nobro-host-contract.json` | Report decoding and external contracts |

### Compatibility Strategy

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
With the `nobro-kernel/hal-profile` feature, apps can derive `SystemProfile`
from `BoardPackage`, which keeps manifest and admission budgets aligned with
the selected board package.

### AI And Robotics Bridges

AI workloads are treated as RTOS-managed modules, not as private background
runtimes. A local TinyML model, an attached accelerator, a companion computer,
or a third-party API should enter the system through adapter descriptors,
capability bits, fixed budgets, caller-owned buffers, timeout policy, and
host-readable compatibility reports.

`AiInferenceSal` is the first SAL contract for this direction. It models a
bounded inference request and result without requiring heap ownership inside the
adapter. Hard-realtime control loops should consume the last valid inference
state or a degraded fallback state instead of blocking on inference.

AI invocation preflight sits before route execution. Rust SAL code and host
tooling use the same contract shape to reject oversized input/output buffers,
excess scratch or arena RAM, stale snapshot policy violations, degraded
fallback, unavailable routes, and open endpoint circuits before a model or
remote API is contacted. Host contract checks additionally verify module AI
capability declarations because they can see the full application bundle.

`AiRoutePolicy` adds a small RTOS-side decision layer for local, edge, remote,
and hybrid inference. The policy compares timeout against the caller's budget,
tracks endpoint readiness and consecutive endpoint failures, allows fresh
snapshot reuse, and chooses degraded fallback when the route is not safe for the
current control cycle. Stale snapshot reuse is bounded by the stricter of the
model contract and runtime policy, so cloud APIs and companion-computer
inference remain compatible with real-time control instead of letting network
latency or outdated results leak into critical paths.

ROS and micro-ROS compatibility belongs at the bridge layer. NobroRTOS should
absorb ROS 2's topic, service, action, and parameter concepts, but map them into
bounded queues, fixed request/response records, action state records, and
kernel-owned configuration. DDS, XRCE-DDS, agents, and custom transports should
stay behind `StreamSal` or `RadioSal` adapters rather than becoming kernel
dependencies.

`RosBridgeSal` is the bounded Rust contract for that bridge layer. It reports a
fixed `RosBridgeContract` summary, uses stable hashes instead of dynamic names
inside realtime paths, and keeps topic publication plus service calls on
caller-owned buffers. DDS, micro-ROS agents, serial bridges, and companion
computer bridges can share this contract without becoming kernel dependencies.
ROS bridge preflight checks topic payloads, service/action response capacity,
queue depth, parameter value size, and timeout budgets before a transport or
agent is contacted.

### Static Async Direction

Embassy demonstrates that embedded async can stay allocation-free and efficient.
NobroRTOS currently offers a bounded executor plus fixed task tables, explicit periods,
deadline budgets, mailbox backpressure, and no allocator on critical paths. Its async
authoring surface is less composable and more verbose than Embassy's: users often must
spell out manifest, admission, capability, and budget objects separately. Future work must
preserve bounded admission while making common async task graphs concise and flexible.

### Isolation And Mixed Criticality

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

### Firmware Trust Boundary

Fleet releases cross an asymmetric boundary before update policy can use them.
`nobro-secure` verifies a pinned Ed25519 key, signed image geometry and vectors,
the SHA-256 image measurement, and the rollback floor. Only that operation can
construct `VerifiedSignedImage`; both fleet rollout and persistent boot staging
consume this private-field token.

Boot trial, confirmation, and revert decisions are committed through a
monotonic storage contract before they take effect, and storage errors fail
closed. Platform ports own durable flash layout, protected-key implementation,
image writing, and the final unsafe jump. HMAC remains appropriate for
per-device authentication and authenticated report envelopes, but it is not the
fleet firmware-signing authority.

### Persistence Boundary

`nobro-storage` separates record-oriented KV persistence from transactional byte
images. Both use two flash pages, wrap-aware generations, integrity validation,
and a commit-last selection point. `nobro-database::PersistentTable` feeds its
stable schema image into the transactional blob path using caller-owned scratch
memory. Board ports define the reserved pages and implement fallible erase,
program, and readback verification.

### Recovery Model

Recovery is module-scoped first:

1. classify the fault
2. update health counters
3. record a bounded event
4. choose an action
5. build a fixed-capacity recovery plan
6. transition lifecycle state
7. export the result through reports

`RecoveryPlan` converts a recovery outcome into ordered, bounded steps such as
notify, retry, quiesce, restart, heartbeat verification, and resume. The plan
uses caller-provided capacity, reports capacity failures explicitly, and checks
the total recovery budget before a supervisor turns an action into work. This
keeps self-healing deterministic and reviewable without heap allocation.
`RecoveryPlanExecution` adds a fixed-capacity cursor over that plan so firmware
loops and host simulators can dispatch only time-ready steps, keep overdue work
visible when the caller-provided output buffer is full, and avoid replaying
steps that were already handed to board-specific adapters.
`StartupGraph::dependency_impact` lets the same recovery path ask which modules
transitively depend on a faulted root module. It returns the affected modules in
reverse startup order, which gives recovery adapters a deterministic
quiesce-before-restart order without heap allocation.
`RecoveryPlan::from_outcome_with_impact` consumes that impact directly, so a
dependency reboot can pause affected modules, restart and verify the root, and
resume the affected modules in startup order with explicit capacity and budget
checks.
Runtime impact-aware recovery entry points require the caller to pass the
dependency impact explicitly and reject mismatched impact roots, keeping startup
graph ownership outside the hot runtime state while still preserving misuse
detection.
`Runtime::apply_recovery_step` closes the execution and bookkeeping loop for
dispatched recovery plans. `ModuleLifecycleHooks` performs platform-owned
notify, retry, quiesce, stop/start, self-test, heartbeat, and resume work. The
runtime updates module state only after the corresponding hook succeeds.
`Runtime::reload_module` similarly requires `ModuleReloadHooks` to perform an
actual module-slot unmount/mount and verification; a failed replacement leaves
the module non-active.
Identical fault work is bounded by `RecoveryStormPolicy`: health counters still
record every occurrence, but duplicate event/lifecycle/recovery-plan dispatch is
coalesced during the cooldown. Action escalation, error changes, cooldown expiry,
and a healthy record re-open dispatch, preserving first-fault evidence without
hiding worsening health.

Disabled modules lose mailbox traffic, alarms, quota reservations, watchdog
registrations, and runtime authorization. Repeated disable commands are
idempotent at the runtime API boundary.

Foreign C/C++ modules use a narrower enforced boundary. `ForeignModuleRunner`
owns admission and callback state, while `ForeignHostContext` binds the admitted
identity to every host operation and combines capability authorization, call/byte
quota charging, execution, and bounded trace records. The foreign caller cannot
supply a `ModuleId`; denial or quota exhaustion prevents the protected operation.

### Memory Discipline

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

### References

- Zephyr devicetree documentation: https://docs.zephyrproject.org/latest/build/dts/index.html
- Embassy project: https://embassy.dev/
- Tock design documentation: https://www.tockos.org/documentation/design/
- seL4 MCS tutorial: https://docs.sel4.systems/Tutorials/mcs.html

## Design principles

This document turns the project route into engineering rules that can survive
new boards, new adapters, and long maintenance windows.

### External Lessons

NobroRTOS borrows selectively from established and modern embedded systems:

- Zephyr uses devicetree to describe hardware and provide initial device
  configuration. NobroRTOS keeps the same lesson, but starts smaller with
  `BoardDesc`, `BusLayout`, and explicit Cargo board features.
- Embassy shows that embedded async can be no-heap and statically allocated.
  NobroRTOS follows the same direction: no allocator on hot paths, static sample
  pools, and compile-time feature selection.
- Tock uses Rust isolation boundaries to keep kernel components mutually
  distrustful with low overhead. NobroRTOS applies that idea at crate and trait
  boundaries: kernel, HAL, SAL, adapters, and apps do not share private state.
- seL4 mixed-criticality work emphasizes bounded kernel operations and clear
  criticality separation. NobroRTOS keeps deadline slots and recovery policy in the
  kernel instead of scattering them across drivers.

### Layer Boundaries

| Layer | Rule |
| ----- | ---- |
| App | Assembles features and owns policy wiring; it should not touch registers directly. |
| Adapter | Translates one device or library into SAL traits; no private scheduler or heap. |
| SAL | Stable capability surface: bus, stream, radio, actuator, sensor, crypto. |
| Kernel | Deadline slots, health, sample tickets, error policy, and eval gates. |
| HAL | Board layout, register access, event capture, PWM, bus, and leases. |

### Multi-Board Compatibility

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
- Host tools consume `nobro-host` constants or `host/nobro-host-contract.json`
  rather than duplicating magic values.
- Host tools should decode module tags and capability bits through the shared
  `nobro-host` helpers or the JSON contract, so reports stay readable as more
  boards and adapters are added.
- Host tools should summarize boot diagnostics in this order: board profile,
  board package, manifest, adapter compatibility, admission, then runtime. This keeps
  first-fault guidance stable as more reports are added.

The current nRF52840 backend uses PPI. The portable HAL term is
`HalEventCapture`; do not leak "PPI" into app or adapter APIs unless the code is
nRF-specific.

### Module Partitioning

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
module count. This lets NobroRTOS reject a feature set that does not fit the target
board before a linker script or flashing step gets involved.

Apps should use `kernel_module_spec` when assembling manifests so kernel-owned
capabilities stay consistent across demos and board ports. Apps can seed
`StartupGraph` directly from a `SystemManifest`, then add only the dependency
edges that are specific to the application boot path.

### Fault Handling And Self-Recovery

Fault handling is intentionally small:

- `KernelError` classifies failures; `HealthFault` adds source, subsystem code,
  and two bounded detail words.
- `Action` describes recovery without allocating memory.
- `HealthMonitor` tracks per-module consecutive failures and their full latest
  context; `FaultPolicy` may retain state and decide per module.
- `FaultThresholds` escalates from local retry, to user notification, to module
  reboot.
- `EventLog` keeps a fixed-size ring of health, recovery, overrun, manifest,
  and host events for post-fault inspection.
- `Supervisor` ties health counters and event records together so every
  recovery decision leaves a bounded audit trail.
- `Watchdog` tracks module heartbeats in software so liveness rules can be
  tested without binding NobroRTOS to one hardware watchdog block. Disabling a
  module removes its watchdog registration so later sweeps cannot revive stale
  liveness faults.
- `ModuleRuntimeGuard` tracks fixed-slot module states across Active,
  Suspended, Faulted, Recovering, and Disabled paths so recovery and later
  device-power policy share one control-plane model.
- `KernelExecutor` owns `ExecutorPower`: measured poll duration feeds the
  per-module energy ledger, and authoritative next-activity time drives a
  fallible wake-program/sleep-entry hook. Executor suspension and resume call
  peripheral power hooks before committing module state.
- `Lifecycle` defines legal boot, running, degraded, recovering, and halted
  transitions so recovery paths are explicit and testable.
- `DegradePlanner` keeps System and HardRealtime modules enabled while shedding
  lower-criticality modules to fit board-class budgets.
- `RetryPolicy` and `RetryState` make bounded retry behavior explicit instead
  of embedding ad hoc loops in adapters.
- `FaultInjector` provides deterministic host-side failure scenarios for
  recovery tests without requiring hardware faults.
- `StartupGraph` and `StartupPlanner` make module dependency order explicit,
  map module IDs to compact dependency bits, and reject duplicate dependency
  edges or cycles before firmware boot logic is involved.
- `QuotaLedger` converts manifest budgets into fixed-capacity runtime
  accounting, so modules can reserve and release RAM, flash, and pool slots
  without heap allocation. Disabling a module resets its runtime quota usage so
  degraded mode returns resources to the system profile immediately. Runtime
  quota mutations are rejected for disabled modules.
- `CapabilityGrantTable` derives runtime authorization from manifest
  requirements and ownership, keeping module access checks fixed-capacity and
  testable. Runtime authorization is still gated by module state, so disabled
  modules cannot keep using previously admitted capabilities.
- `Mailbox` provides fixed-capacity control-message IPC with accountable module
  quotas, reserved recovery/shutdown capacity, and priority ahead of ordinary
  FIFO traffic; data payloads still
  move through `Sample` tickets and static pools. Runtime IPC validates both
  message endpoints against the admitted and enabled module set before messages
  enter the queue. Disabling a module purges queued messages to or from that
  module so stale control traffic cannot outlive the module state transition.
- `AlarmQueue` provides no-heap one-shot and periodic software timers without
  binding app logic to a specific hardware timer block. Disabling a module also
  removes its queued alarms so disabled modules cannot be reawakened by stale
  timer events, and new alarm scheduling is rejected for disabled modules.
- `AlarmDispatch` summarizes due-alarm delivery, including partial progress and
  the first alarm blocked by mailbox backpressure, without dropping the alarm.
  Runtime code can route that blocked alarm through recovery as a deadline
  fault, keeping timer backpressure visible to health reports.
- `KvStore` is the kernel's volatile fixed-capacity configuration table. Durable
  records and typed database images use `nobro-storage` and `PersistentTable`; a port
  still has to reserve flash pages and implement fallible erase/program/readback.
- `AdmissionController` composes manifest validation, startup ordering, and
  quota seeding into one boot-time software gate before board-specific startup
  code runs.
- `AdmissionReport` provides a fixed host-readable admission result so startup
  failures can be diagnosed without dynamic logging. It can be built from the
  same admission result used by boot code, reducing report-path drift.
- `BootAssembly` is a no-heap startup facade for small applications. It builds
  a manifest from static module specs, applies explicit startup dependencies,
  runs admission, constructs the runtime, boots it to `Running`, and preserves
  manifest/admission reports without hiding the failing phase.
- `RecoveryCoordinator` composes health, lifecycle transitions, watchdog-style
  deadline faults, and event logging into one testable recovery path.
- `HealthReport` turns supervisor snapshots into fixed-layout host-readable
  records with the same checksum discipline as eval and admission reports.
- `EventLogReport` summarizes the fixed event ring for host tools, including
  capacity, drops, and the latest event's module, severity, kind, and payload.
- `ModuleRuntimeReport` summarizes module runtime states for host tools,
  including Active, Suspended, Faulted, Recovering, Disabled, and the latest
  changed module.
- `DegradeApplicationReport` summarizes the latest runtime degraded-mode
  application, including requested disables, newly disabled modules, modules
  that were already disabled, the budget reason, and the application timestamp.
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
  and only then resumes `Running`; disabled modules are rejected before any
  lifecycle transition is attempted. Degraded-mode decisions are validated
  before module state changes and the last successful application is retained
  as a fixed-layout host report. Runtime assembly from startup plans is
  fallible, so fixed-capacity module registration errors are reported instead
  of being silently ignored. Manual runtime disable is idempotent at the
  runtime API boundary, keeping repeated recovery commands safe while the lower
  module state machine remains strict.

Recovery is module-scoped by default. Full chip reset is a last resort and
should remain outside hot-path adapters.

### Memory Discipline

The default rule is no allocator:

- use static pools for payloads
- pass `Sample` tickets instead of raw buffers across crates
- use `LeaseGuard` to avoid resource leaks
- keep ISR work bounded and defer parsing to cooperative tasks
- prefer compile-time features over runtime plugin registries

Any future heap use must be feature-gated, documented, and excluded from
hard-real-time paths.

### Maintainability Gates

Before adding a hardware-dependent feature, add at least one software gate:

- a host unit test
- a checksumed host-readable report
- a register snapshot comparison
- a board feature/linker validation
- a no-hardware stub adapter

This keeps NobroRTOS useful throughout design, review, and bring-up.

### Next RTOS Direction

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
- adapter descriptor sets expose fixed-buffer descriptor copy and module lookup
  APIs so host/app tooling can inspect adapter inventory without heap
  allocation
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
- app assembly patterns that connect adapter preflight, board package reports,
  and `BootAssembly` without adding runtime plugin registries

The current executor support is deliberately small: `TaskTable` is a fixed-size
task registry that records period, budget, criticality, due time, and overrun
statistics. It lets NobroRTOS validate scheduling contracts before choosing a full
async executor surface.

The current observability support is equally small: `EventLog` is a no-heap ring
buffer that preserves the latest records, tracks drops, and can be copied into a
host-readable report without exposing dynamic logging dependencies to ISR or
hard-real-time code.

## Portable Hardware Providers

`PlatformHal` identifies a platform and board package; it does not require a
monolithic set of peripherals. Timebase, scheduling, deadline, capture, PWM,
lease, I2C, SPI, and self-test behavior are independent provider traits.
Portable leases use a neutral class plus instance number, and each platform
adapter performs the concrete peripheral mapping. Bus providers also declare
whether transfers are polling or DMA.

## Mountable stacks (HAL modularity)

NobroRTOS keeps board and vendor differences behind **mountable backends**: a
board selects one implementation of a common trait by a Cargo feature, and app
code never names the concrete stack. This is how NobroRTOS stays compatible
with the ArduinoNRF ecosystem and, per board, with other vendor libraries
without forking the apps.

### Reference: ArduinoNRF Layer 0

The nRF52840 profiles run the ArduinoNRF core's Layer 0 by default: native
`NrfUsbd`, NimBLE, GDB stub, and peripheral drivers. NobroRTOS mirrors that
default so boards can behave like stock ArduinoNRF targets, while still moving
to ArduinoNRF's own stacks by swapping a feature.

### USB - implemented (`crates/nobro_usb`)

`UsbStack` trait + `mount()`; a board picks one backend:

| feature | backend | status |
| --- | --- | --- |
| `backend-nrf-usbd` (default) | vendored `nrf-usbd` + `usbd-serial` CDC | implemented |
| `backend-usb-serial-jtag` | fixed-function USB serial/JTAG peripheral | implemented |
| `backend-ra-usbfs` | USBFS CDC device backend | implemented |

`usb_stack_demo` consumes only `mount()` + `UsbStack`. Compile-time guards allow
exactly one implemented backend, and a process-wide permanent claim rejects a
second mount before hardware access. Nonfunctional placeholder stacks are not
published as features.

### Radio / BLE / Zigbee / RFID - same pattern

The mountable-backend shape extends to wireless and proximity links, each behind its own trait:

- **BLE**: a `BleStack` trait with backends `nimble` (ArduinoNRF default) and
  `nrf-softdevice` (S140-compatible layout). The existing nRF `Radio` driver is
  the raw-radio backend.
- **Zigbee / 802.15.4**: a `RadioCoprocessor` trait with backends such as UART
  co-processors and, later, the nRF on-chip RADIO running Nordic's official
  Zigbee sidecar firmware.
- **RFID / NFC**: `SpiIo` adapts a board SPI driver to the no-heap `Mfrc522` backend, which exposes ISO 14443A UID polling through `WirelessBackend`.

Each backend is `no_std`, feature-selected, and swappable per
`core/boards/<platform>/*/board.json`, so a board's wireless identity is data plus one
feature, not scattered `#[cfg]`s.

### Why mountable, not `#[cfg]` sprinkled

One trait + one `mount()` per subsystem means apps are backend-agnostic and
portable; a new board is a data drop plus a backend choice; and vendor stacks
are integrated once, behind the subsystem boundary, instead of leaking into
every app. This is the same discipline the kernel already applies to leases and
capabilities, extended to the USB and wireless vendor layer.

## The Universal Driver Interface

NobroRTOS treats drivers the way Adafruit Unified Sensor treats sensors: **one
category, one trait, many mountable backends.** A part is catalog data; a backend
is a compile-time feature that plugs a concrete library or transport behind the
same SAL trait.

This is the public rule behind the `ImuSal` hardware proof (`udi_imu_demo`) and
the pattern to extend to other sensor categories.

### The rule

```
Category trait (SAL)     e.g. ImuSal::sample()
    ├─ backend-native      register driver in-tree (mpu9250-imu)
    ├─ backend-eh          any embedded-hal driver crate
    ├─ backend-c-module    C/C++ module via nobro_app.h
    └─ backend-arduino     stock Arduino library via NobroArduinoShim
```

Every backend:

1. Implements the **same category trait** (`ImuSal`, `TempSal`, more to come).
2. Is selected by **exactly one** `backend-*` Cargo feature (mutual exclusion).
3. Carries a stable **`backend_id`** in the hardware eval report so you can prove
   which transport sealed the PASS without the evaluation function naming a driver.
4. Runs through the **same eval body** — only the mount changes.

### What transfers vs what you re-express

| From your existing code | UDI answer |
| --- | --- |
| Arduino sensor library | `backend-arduino` shim behind the category trait |
| `embedded-hal` driver crate | `backend-eh` adapter |
| Register-level C driver | `backend-c-module` via `nobro_app.h` |
| In-tree Nobro driver | `backend-native` |
| Task / loop / executor | NobroRTOS module + manifest (see cookbooks) |

### Proven today: `ImuSal`

`core/apps/imu/udi_imu_demo` shares one `app.rs` evaluation across three binaries:

| Backend | Feature | `backend_id` | Transport |
| --- | --- | --- | --- |
| Native HAL | `backend-native` | 1 | SPI via `nobro_hal` |
| embedded-hal | `backend-eh` | 2 | SPI via `SpiDevice` |
| Arduino shim | `backend-arduino` | 3 | SPI via `NobroArduinoShim` + stock MPU9250 class |

The three feature-selected binaries share the same application body and report contract.
Maintainer HIL must obtain `all_pass=1` with the expected `backend_id`; endpoint and
restoration details are intentionally not part of the public repository.

### Adding a new category

1. Define a **category trait** in `nobro_sal` with bounded return types (no heap).
2. Add a **catalog entry** in `nobro_device` (part id, bus, who-am-i, ranges).
3. Ship at least **two backends** (native + eh is the minimum credible proof).
4. Add a **swap demo app** with one shared diagnostic body and feature-gated mounts.
5. Add portable contract tests; request maintainer HIL before claiming physical support.

### Adding a new backend to an existing category

1. Implement the category trait in a new adapter crate or C/C++ module.
2. Add a `backend-*` feature with `compile_error!` if more than one is enabled.
3. Wire the mount in the demo app's `main.rs` (thin — only constructs the backend).
4. Flash and read the report; `backend_id` must be unique and documented.

### Related docs

- [PORTING.md](PORTING.md) — migration cookbooks and board-port workflow
- [GETTING_STARTED.md](GETTING_STARTED.md) — public host and image-deployment workflow


### Second category: `TempSal` (hardware-proven)

The rule generalizes: `TempSal::read_temp_centi_c()` reports centi-degrees Celsius from
whatever part a backend wraps. All three `udi_imu_demo` backends implement it against the
same die-temperature register, sealed on the same board back to back:

| Backend | `backend_id` | temp reading |
| --- | --- | --- |
| native HAL | 1 | 31.24 C |
| embedded-hal | 2 | 31.20 C |
| Arduino-library shim | 3 | 31.15 C |

Three transports, one silicon, answers within 0.1 C - the category abstraction costs
nothing in fidelity. The report's `temp_centi_c` field and its 10-60 C plausibility
check are part of `all_pass`.
