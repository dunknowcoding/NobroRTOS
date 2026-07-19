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

`core/adapters/catalog.json` uses stable component IDs to relate contracts,
adapters, external libraries, and host products to domains. Deployment,
maturity, evidence class, supported targets, limitations, and provenance are
orthogonal fields. A component may belong to multiple domains by reference
without duplicating its source or provenance. Board profiles remain outside
this relationship catalog because platform tiers already own board support.

Board-varying engines use a second, data-only extension boundary in
`core/boards/feature_providers.json`. It separates the portable contract,
stack family, adapter backend, exact board/firmware binding, resource price,
coexistence policy, evidence, and report wiring. Candidate capability kinds do
not become platform claims: `platform_tiers.json` accepts a kind only when the
registry contains an exact binding and the ordinary scoped evidence gate.
Disabled bindings must declare the same-board baseline, zero flash/RAM delta,
and forbidden backend symbols. Resource prices share one typed contract across
audio, sensor, and pulse domains. Default numeric zeroes remain *unknown*;
every exact binding must classify each field as measured or explicitly
declared zero. Peripheral/controller channels are priced independently from
DMA channels and interrupt slots.

### Compatibility Strategy

Hardware is described as structured data that can be validated before driver code
relies on it. The current implementation starts with `BoardDesc`, board features, memory
scripts, and host-readable board profile reports.
`BOARD_PROFILES` and `BOARD_PACKAGES` keep the current board
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

NobroRTOS offers a bounded executor plus fixed task tables, explicit periods,
deadline budgets, mailbox backpressure, and no allocator on critical paths. The graph
builder derives the repetitive manifest, admission, capability, and budget wiring while
keeping bounded admission visible.

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

## Design principles

This document turns the project route into engineering rules that can survive
new boards, new adapters, and long maintenance windows.

### Core rules

- Board configuration is data: `BoardDesc`, `BusLayout`, and Cargo features are
  validated before applications depend on them.
- Hot paths avoid allocation through static pools and fixed-capacity structures.
- Kernel, HAL, SAL, adapters, and apps exchange public contracts rather than private
  state.
- Deadline slots, admission, and recovery policy stay in the kernel instead of being
  duplicated in drivers.

### Layer Boundaries

| Layer | Rule |
| ----- | ---- |
| App | Assembles features and owns policy wiring; it should not touch registers directly. |
| Adapter | Translates one device or library into SAL traits; no private scheduler or heap. |
| SAL | Stable capability surface: bus, stream, radio, actuator, sensor, crypto. |
| Kernel | Deadline slots, health, sample tickets, error policy, and admission gates. |
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
- `TaskDecl` and `DeadlineContract` carry explicit phase, period, and relative
  deadline. The release queue uses phase to avoid unnecessary bursts, while
  shared admission deliberately keeps pessimistic interference instead of
  treating offsets as free schedulability headroom.
- `AppGraph` is the fluent graph-authoring path. `GraphSpec` borrows const/static
  `TaskDecl` and `ChannelDecl` slices for firmware that cannot afford a
  capacity-sized graph builder on the startup stack; both paths expand through
  the same manifest/startup/profile validation. `GraphSpec::start_executor`
  additionally performs boot, task registration, and sealing with only the
  derived manifest/startup nodes as temporary scratch; task metadata is
  regenerated from the declaration, so applications that do not need runtime
  graph introspection avoid retaining `BuiltGraph` RAM or its label/reactor
  arrays on the startup stack. This is an explicit
  static-RAM-versus-startup-stack composition choice, not a claim that the
  temporary form always has the lower peak on every target.
- Opt-in P-ISR admission prices bounded interrupt execution, platform-reserved
  priorities, higher-priority interference, and nested exception-stack use.
  `InterruptHandoff` limits ISR work to lock-free ready/event publication.
- Opt-in P-SLICE uses task-owned PSP stacks and `SliceController`; the nRF
  `cortex-m-slice` port saves R4-R11, EXC_RETURN, BASEPRI, and the lazy-FPU
  extension in PendSV. PendSV is configured at or below the selected kernel
  BASEPRI ceiling so it cannot split a process-wide critical-section
  transaction. A queued forced switch is committed only after the port reports
  the PSP switch complete, so a ceiling-held overrun keeps the old task and
  sentinel attribution until the section exits; a section that never exits
  requires watchdog escalation.
  The current port is a bare-nRF profile: combining `cortex-m-slice` with
  `board-nicenano-s140` fails at compile time because it programs PendSV through
  CMSIS and does not yet integrate interrupt control through the SoftDevice API.
  Cooperative execution remains the default, and neither profile
  implies unprivileged execution or MPU isolation.
- Native nRF board features install one BASEPRI-backed implementation for the
  ecosystem-wide `critical-section` ABI. Bare builds mask logical priorities
  3-7 and S140 builds mask application priorities 6-7; deadline/watchdog/P-ISR
  domains above those ceilings remain live and use lock-free handoff only.
  The build gate rejects a simultaneous Cortex-M PRIMASK implementation.
- The nRF idle path uses SEVONPEND plus `SEV; WFE`, rechecks the deadline and
  lock-free ISR handoff, then performs the sleeping `WFE`. This consumes stale
  events without reopening the check-to-sleep race or counting a false sleep.
- Cortex-M0+ has no BASEPRI. The SAMD21 port therefore owns a measured PRIMASK
  provider: a free-running SysTick counter records each outer section, nested
  sections inherit that interval, and a counter wrap fails closed. Its serial
  report exposes maximum masked cycles/time and the configured bound; target
  compilation alone is not physical timing evidence. This measurement owns
  SysTick in the current SAMD21 core-only port.
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
  records with the same checksum discipline as health and admission reports.
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
- Alarm, KV, retained event-log, and capability-trace capacities are optional
  composition choices and may be zero. Their existing operations fail closed
  on zero capacity; mailbox IPC and per-module admission/health tables remain
  mandatory. `LeanRuntime` and `LeanKernelExecutorCell` expose this composition
  without asking users to spell seven or eight const parameters.

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
- optional async executors with static task allocation, deadline-guarded
  futures that turn fired compares into typed health faults, and graph-linked
  multi-priority reactor domains with one admitted driver task per domain plus
  explicit cross-domain channels
- health reports exported through the same host contract as runtime reports
- fixed-layout health reports with checksums for CDC, memory inspection, or another stream
  readers
- app assembly patterns that connect adapter preflight, board package reports,
  and `BootAssembly` without adding runtime plugin registries

The current executor support is deliberately small: `TaskTable` is a fixed-size
task registry that records period, budget, criticality, due time, and overrun
statistics. An intrusive sorted release queue makes the next deadline an O(1) lookup;
elapsed releases are transferred without a capacity-wide scan. A five-level
criticality bitmap selects a FIFO head in O(1), preserving fairness between peers.
Bounded reinsertion happens after a poll, outside the release-to-dispatch edge.
The ready-membership word supports at most 32 tasks and rejects a wider table.
Bookkeeping remains synchronous in the current executor: poll timing, overrun
handling, reinsertion, idle-safety checks, and instrumentation snapshots finish
inline before the cycle can report idle. NobroRTOS does not yet ship an admitted
maintenance-service reserve or saturated debt model, so no background
accounting task is claimed.
Compare providers can program the exact earliest release group and transfer its
bits from ISR context; early, duplicate, and stale bits fail closed.

Tickless admission charges a measured compare-wake-to-dispatch bound once in each
response-time calculation. The bound defaults to zero for compatibility and must
be set explicitly when the selected board/composition has measured evidence.

The current observability support is equally small: `EventLog` is a no-heap ring
buffer that preserves the latest records, tracks drops, and can be copied into a
host-readable report without exposing dynamic logging dependencies to ISR or
hard-real-time code.

## Portable Hardware Providers

`PlatformHal` identifies a platform and board package; it does not require a
monolithic set of peripherals. Timebase, scheduling, deadline, capture, PWM,
lease, I2C, and SPI behavior are independent provider traits.
Portable leases use a neutral class plus instance number, and each platform
adapter performs the concrete peripheral mapping. Bus providers also declare
whether transfers are polling or DMA.

Capability rows describe one firmware composition, not the union of every API that can
compile for a board. For RA4M1, the native Rust composition implements timebase,
deadline, and USB. The separate Arduino facade delegates clock, deadline, ADC, PWM,
I2C, SPI, and byte I/O to the installed board core and is recorded separately in the
platform matrix. Generic Arduino `analogWrite` is PWM, not a servo-period provider.

The current RA4M1 clock extends a 48 MHz 32-bit DWT counter, so active firmware must
sample it within every approximately 89-second wrap; it does not preserve elapsed time
when DWT stops in low-power modes. The 24-bit SysTick deadline provider accepts one-shot
delays only through approximately 349 milliseconds. Longer active runs and deadlines
need a future always-on/chained provider before stronger timing claims are made.

## Mountable stacks (HAL modularity)

NobroRTOS uses **mountable backends** where the subsystem implements the complete
selection contract: a firmware composition chooses one implementation of a common
trait and application code consumes the trait. USB is the implemented reference.
Sensor categories use the Universal Driver Interface described below. Wireless has a
shared bounded data-plane trait today, but its vendor-stack selection layer is planned,
not present.

### Reference: ArduinoNRF Layer 0

Arduino sketches use the stacks and peripheral ownership supplied by the installed
ArduinoNRF board package. Native Rust firmware uses the providers compiled into its
selected composition. These are distinct compositions; NobroRTOS does not currently
offer a feature that swaps a running native firmware to an Arduino-managed BLE stack.

### USB - implemented (`crates/nobro_usb`)

`UsbStack` trait + typed `try_mount()` (with panic-compatible `mount()`); a board picks
one backend:

| feature | backend | status |
| --- | --- | --- |
| `backend-nrf-usbd` (default) | vendored `nrf-usbd` + `usbd-serial` CDC | implemented |
| `backend-usb-serial-jtag-esp32c3` | ESP32-C3 fixed-function USB serial/JTAG register map | implemented; physical recovery evidence pending |
| `backend-usb-serial-jtag-esp32s3` | ESP32-S3 fixed-function USB serial/JTAG register map | implemented; physical recovery evidence pending |
| `backend-ra-usbfs` | USBFS CDC device backend | implemented |

The Cortex-M `usb_stack_demo` consumes only `try_mount()` + `UsbStack` and selects nRF
USBD or RA4M1 USBFS. ESP32-C3/S3 use architecture-specific demos in their port crates;
the Cortex-M package does not advertise impossible RISC-V/Xtensa features. Compile-time
guards allow exactly one implemented backend. Its RA feature uses the exact exported
identity and the 0x4000 RA4M1 application link map, but it is a compile/link contract;
complete UNO R4 clock/mux startup remains in the RA4M1 port executable. Mount preflight
validates fixed descriptor requirements before a process-wide permanent claim and before
hardware access; the panic-compatible `mount()` wrapper is retained for existing callers.
Nonfunctional placeholder stacks are not published as features.

`UsbConfig` is the requested identity. The nRF backend generates descriptors from it,
the RA4M1 backend accepts only `RA4M1_USB_CONFIG`, and ESP USB-Serial-JTAG descriptors
are fixed by the controller and ignore the request. The public `identity_policy()` and
`config_supported()` facts keep accepted configuration distinct from host-visible
identity.

The ESP link state fails closed: SOF establishes only a live bus. The backend clears
reset-high `SERIAL_IN_EMPTY` without treating it as enumeration evidence, waits for a
free IN FIFO before issuing a zero-length EP1 probe, and reports `Configured` only after
a later EP1 IN token or OUT packet. Bus reset and the SOF watchdog invalidate that state.
Reset/pre-probe OUT evidence is cleared and its packet FIFO is drained with a 64-byte
per-poll bound before probing, so stale data cannot strand the FIFO or become configured
evidence. `IN_EP_DATA_FREE` means only that the FIFO is not full; nonempty writes clear
the old empty event and retain a pending flag until a later `SERIAL_IN_EMPTY` event.
Therefore flush never infers completion from free capacity. Host state-machine tests do
not replace physical disconnect/reconnect evidence.

The nRF backend bounds controller-ready and EasyDMA completion polling, retains late-DMA
storage in permanently claimed aligned buffers, and propagates terminal direction/endpoint
faults through the common error lane. Those finite waits still run inside a critical
section and their limits count iterations, not elapsed time. Until target timing and a
poll-driven transfer state machine close that gap, they are a liveness containment—not an
interrupt-blackout or deadline guarantee. Unsupported nRF isochronous endpoints are
rejected during allocation rather than reaching the regular endpoint arrays.

### Audio - bounded contract over board-owned I2S

`nobro-audio` defines allocation-free codec configuration, lifecycle,
capture/playback, an optional measured-I/O deadline extension, explicit
admission price, and a fixed-capacity backpressuring frame ring. It does not
reimplement a vendor DMA engine.

The first concrete bridge is `audio/esp32s3-es8311`. Its Rust side validates
signed-16 formats, frame bounds, state transitions, recovery, and resource
accounting over a mountable transport. Its Arduino side,
`NobroNiusAudio.h`, wraps the pinned NiusAudio ES8311 driver with a
compile-time queue. Arduino-ESP32 owns I2S/DMA and codec control; Nobro owns
what the application can submit, how much it retains, the completion budget,
backpressure, and diagnostics.

NiusAudio is a member of `nobro-audio`, not a parallel ecosystem or copied
source tree. A new codec remains a module/library implementation under the
same contract. A board claim appears only when the feature registry has an
exact backend, composition binding, price, disabled-symbol proof, report
wiring, and executable evidence.

### Continuous ADC and pulse engines

Continuous sampling and hardware pulse generation reuse the existing
`nobro-sensors` and `nobro-servo` domains. They do not create another
ecosystem or a board-specific application hierarchy:

- `sensors/esp32-adc-continuous` mounts the Arduino-ESP32 process-wide
  continuous ADC/DMA service behind bounded channel/frame, deadline,
  lifecycle, recovery, and complete resource-price contracts.
- `servo/esp32-ledc` mounts fixed-frequency duty output.
- `servo/esp32-rmt` mounts bounded pulse-symbol output.

`NobroEsp32Peripherals.h` is the corresponding beginner-facing composition.
Its objects allocate no heap. Arduino-ESP32 may allocate ADC/DMA, channel, and
driver state internally. The ADC facade reports the aligned conversions per
channel required by the board core and rejects a frame shape that Arduino-
ESP32 would silently widen, keeping averaging and deadline semantics exact.
State-restoring classic ESP32, single-core ESP32-C3, and dual-core ESP32-P4
campaigns verify continuous sampling, LEDC frequency/duty, RMT pulse timing,
lifecycle recovery/release, and immediate runtime reservations. C3 measured
19,999 conversions/s, 1,002 Hz at 249 permille, and 499-500 us RMT levels.
P4 measured 19,795 conversions/s with an exact aligned frame, 1,002 Hz at
249 permille, and 499-500 us RMT levels. Unreferenced ADC inputs are not
calibration evidence. S3 remains target-build evidence only. No exact binding
is promoted until stack, CPU, interrupt, DMA, peripheral-channel, coexistence,
and other registry price dimensions are measured; compilation never turns an
unknown price into zero.

Provider lifecycle distinguishes temporary quiescence from unmount:
`quiesce` preserves logical configuration for `recover`, while `release`
stops/deinitializes or detaches the transport, forgets configuration, and
returns `Down`. Rust adapters and the Arduino facade share this rule so a
detachable module cannot retain vendor resources by API accident.

### Radio / BLE / WiFi / Zigbee / RFID - current boundary and planned shape

Implemented today in `nobro-wireless`:

- `WirelessBackend` is the bounded application data plane, and `ManagedLink` adds
  resource accounting plus a deadline check for one immediate send attempt.
  `TxContract` does not schedule priority or execute retries: priority belongs to the
  scheduler and retry state belongs to the caller. Implementations are constructed
  explicitly; the crate does not yet select a vendor stack from a board profile.
- `Mfrc522<SpiIo>` implements bounded ISO 14443A UID polling, and `Cc2530<ByteIo>`
  implements an initialized raw IEEE 802.15.4 PSDU transport behind `WirelessBackend`,
  bounded by the 127-byte PHY frame limit. It is not a Zigbee join/network/APS stack;
  `ZIGBEE_APS` is catalog descriptor metadata only.
- `BleAdvBuilder` constructs advertising packets. It is not a BLE controller/host
  stack, and a protocol descriptor is not proof that a board implements that protocol.

WiFi join/socket control, BLE scan/connect/GATT control, Zigbee co-processor lifecycle,
shared-radio arbitration, and vendor backend selection remain future work. They will
extend the existing `nobro-wireless` domain rather than create a parallel link crate.
Each protocol control trait will sit beneath `ManagedLink`, each logical instance will
select exactly one backend, and board/firmware composition will state vendor-managed
memory, interrupts, coexistence, and radio ownership explicitly. Concrete names and
features become public only when their implementations and exclusivity gates exist.

### Why mountable, not `#[cfg]` sprinkled

One trait plus one selected implementation keeps apps backend-agnostic when the whole
selection path exists. USB demonstrates that rule now. Future wireless control stacks
must earn the same property through explicit composition, ownership, and conformance
gates; adding a board profile or a catalog descriptor alone is insufficient.

## The Universal Driver Interface

NobroRTOS treats drivers the way Adafruit Unified Sensor treats sensors: **one
category, one trait, many mountable backends.** A part is catalog data; a backend
is a compile-time feature that plugs a concrete library or transport behind the
same SAL trait.

This is the public rule behind the `ImuSal` backend example (`udi_imu_demo`) and
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
3. Carries a stable **`backend_id`** in the health report so the selected transport
   remains visible without the diagnostic function naming a driver.
4. Runs through the **same diagnostic body** — only the mount changes.

### What transfers vs what you re-express

| From your existing code | UDI answer |
| --- | --- |
| Arduino sensor library | `backend-arduino` shim behind the category trait |
| `embedded-hal` driver crate | `backend-eh` adapter |
| Register-level C driver | `backend-c-module` via `nobro_app.h` |
| In-tree Nobro driver | `backend-native` |
| Task / loop / executor | NobroRTOS module + manifest (see cookbooks) |

### Proven today: `ImuSal`

`core/apps/imu/udi_imu_demo` shares one `app.rs` diagnostic body across three binaries:

| Backend | Feature | `backend_id` | Transport |
| --- | --- | --- | --- |
| Native HAL | `backend-native` | 1 | SPI via `nobro_hal` |
| embedded-hal | `backend-eh` | 2 | SPI via `SpiDevice` |
| Arduino shim | `backend-arduino` | 3 | SPI via `NobroArduinoShim` + stock MPU9250 class |

The three feature-selected binaries share the same application body and report contract.
Each backend must preserve the same public status fields and backend identifier.

### Adding a new category

1. Define a **category trait** in `nobro_sal` with bounded return types (no heap).
2. Add a **catalog entry** in `nobro_device` (part id, bus, who-am-i, ranges).
3. Ship at least **two backends** to preserve the swappable contract.
4. Add a **swap demo app** with one shared diagnostic body and feature-gated mounts.
5. Add portable contract checks and exercise the selected backend before claiming support.

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
