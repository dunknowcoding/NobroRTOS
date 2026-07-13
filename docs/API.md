# API Reference

The complete public surface: Rust crates, the C ABI, the Python package, and
the host contract. The generated per-crate index is [api-index.md](api-index.md)
(regenerate with `python tools/gen_api_index.py`).

### Crate roles at a glance (ml vs ai vs nn)

| Crate | Role | Typical caller |
| --- | --- | --- |
| `nobro_nn` | from-scratch NN *inference blocks* (dense/conv/LSTM/attention, int8 kernels) | firmware running a model |
| `nobro_ml` | classic/TinyML *utilities* (quantization, filters, anomaly, federated averaging) | firmware feature pipelines |
| `nobro_ai` | model *governance*: manifests + checksums for already-trained models, cloud-session state machines | deployment + host tooling |

## Crates and contracts

This manual summarizes the public crates and the core contracts used by
applications, adapters, and host tooling.

### Crate Overview

| Crate | Purpose |
| ----- | ------- |
| `nobro-kernel` | Manifest, admission, runtime, quota, capability, scheduler, IPC, alarms, recovery, health, and reports |
| `nobro-hal` | Board profiles, platform traits, nRF52840 implementation, leases, timers, PWM, bus, and event capture |
| `nobro-sal` | Stable service traits for adapters, apps, AI inference, and edge bridges |
| `nobro-nn` | Heap-free MCU inference blocks: dense, int8 dense, 1-D/2-D convolution, pooling, recurrent cells, and attention |
| `nobro-ml` | Heap-free DSP/ML utilities: anomaly stats, fusion, gesture detection, KWS audio features, model scheduling |
| `nobro-secure` | Secure boot decisions, attestation, rollback guard, key store, tamper seal, and audit log |
| `nobro-net` | Mesh routing, secure links, OTA chunking, store-forward queues, fleet OTA rollout planning |
| `nobro-wireless` | Bounded wireless contracts and mountable BLE, WiFi, Zigbee, Thread, RFID, and proprietary backends |
| `nobro-camera` | Frame leases, capture admission, stream backpressure, recovery, and diagnostics |
| `nobro-host` | Host-side constants, report layouts, labels, and status helpers |

### Neural Network API

`nobro-nn` is inference-side and `no_std`: callers pass all input, output, and
scratch buffers explicitly. Dense layers use `[OUT][IN]` weights. 2-D
convolution and pooling use NHWC tensors without a batch dimension, which keeps
camera and sensor-grid models easy to inspect on small MCUs:

```rust
let mut out = [0.0f32; 4];
nobro_nn::conv2d_valid(
    &image_3x3x1,
    3, 3, 1,
    &kernel_2x2x1,
    2, 2, 1,
    &[0.0],
    &mut out,
);
```

For quantized models, `dense_int8` and `conv2d_valid_i8` accumulate into i32.
The caller can then fuse activation, argmax, or requantization according to the
model contract.

`nobro-ml` provides the fixed keyword-spotting feature contract used by the
yes/no example model. `kws_log_energy_features` turns up to one second of
16 kHz mono PCM into 120 normalized features (`15 x 8`) without heap allocation:

```rust
let mut features = [0.0f32; nobro_ml::KWS_FEATURES];
assert!(nobro_ml::kws_log_energy_features(&pcm_i16, &mut features));
```

### Wireless Domain API

`nobro-wireless` keeps link identity as data and physical radios behind small traits.
Apps consume `WirelessBackend` for descriptor, link-state, send, and receive calls,
so a BLE advertisement backend, a Zigbee co-processor, or an RFID reader can be
mounted without changing application logic.

RFID readers use the same discipline. `SpiIo` is the board-supplied SPI byte
adapter, `rfid_readers::MFRC522_SPI` describes a common ISO 14443A reader, and
`Mfrc522<S>` provides bounded request, anticollision, UID polling, and an
`WirelessBackend` implementation:

```rust
let mut reader = nobro_wireless::Mfrc522::new(board_spi);
reader.init()?;
let uid = reader.poll_uid(4)?;
assert!(!uid.is_empty());
```

The backend allocates no heap memory, bounds polling, validates ISO 14443A BCC,
and returns explicit `RfidError` values for bus, timeout, collision, protocol,
and buffer failures.

### Camera Domain API

`nobro-camera` keeps camera code small and composable: a backend lends a frame,
while `CameraPipeline` enforces the caller's deadline, maximum frame size,
frames/bytes per window, and in-flight limit. Storage, AI, and transport share the
same diagnostics without owning the sensor driver. The matching C ABI is
`nobro_camera.h`; Arduino users mount NiusCam through `NobroNiusCam.h`.

### Security API

`nobro-secure` keeps the secure-boot decision separate from the unsafe,
board-specific jump. The recommended fleet boundary is `verify_signed_boot`,
which verifies a pinned Ed25519 key, signed image metadata, SHA-256 measurement,
anti-rollback floor, and entry/stack policy before returning a
`VerifiedSignedImage`. Its fields are private, so update and boot controllers
cannot manufacture an unverified release token.

`PersistentBootController` commits stage, first-trial, confirm, and revert state
through `MonotonicBootStore`; storage failures stop the boot decision. A board
port supplies the durable store and the final jump. `ProtectedKeyBackend`
similarly lets a platform authenticate without exporting key bytes, while
`AuthenticatedReportEnvelope` is the integrity boundary for reports used in
trust decisions.

The older `SecureBoot::boot_plan` HMAC path remains for per-device authentication
and compatibility. It verifies image length, address range, entry/stack vector
sanity, SHA-256 measurement, and anti-rollback version before returning a
`VerifiedBootPlan`:

```rust
let policy = nobro_secure::BootVectorPolicy::cortex_m(
    0x1000,
    0x80000,
    0x2000_0000,
    0x2004_0000,
);
let plan = secure_boot.boot_plan(&boot_key, image, &manifest, policy)?;
```

`tools/sign_firmware.py` can emit a matching legacy HMAC manifest JSON:

```powershell
python tools/sign_firmware.py app.bin --version 8 --load-addr 0x1000 --entry-addr 0x1101 --stack-top 0x20010000 --manifest-out _work\app.manifest.json
```

### Transactional Database Persistence

`nobro-storage::BlobStore` stores one bounded byte image across two alternating
flash pages. Payload and checksum are written before the commit marker, and mount
selects the newest valid generation. A reset during erase or programming therefore
exposes either the complete old image or the complete new image.

`nobro-database::PersistentTable` combines that transaction protocol with
`Table<V, N>`'s stable `RecordCodec`. Callers supply a scratch buffer, keeping load
and save allocation-free:

```rust
let mut persisted = nobro_database::PersistentTable::mount(board_flash);
let mut image = [0u8; 256];
let table = persisted.load::<Reading, 8>(&mut image)?;
persisted.save(&table, &mut image)?;
```

### Kernel API

#### Manifest

`SystemManifest<N>` is a fixed-capacity list of `ModuleSpec` records. Each
module declares:

- `id`: stable module tag
- `criticality`: best effort, user, driver, system, or hard realtime
- `requires`: capability bits needed at runtime, including optional AI and bridge capabilities
- `owns`: capability bits provided or exclusively owned by the module
- `memory`: flash, RAM, and sample-pool budget
- `deadline`: optional periodic contract
- `faults`: retry and escalation thresholds

Use manifest validation before constructing runtime state:

```rust
let report = nobro_kernel::ManifestReport::from_result(
    &manifest,
    manifest.validate_profile(profile),
);
assert!(report.verify_checksum());
assert_eq!(report.valid, 1);
```

Python contract bundles mirror the same ownership discipline for host tooling:
non-kernel capabilities must have a single owning module, and user-level modules
cannot claim kernel-owned capabilities such as the timebase or host-report
surface.
Use `check-bundle-matrix` to validate module naming, capability ownership,
AI/ROS descriptor uniqueness, hard-realtime deadlines, startup dependency
errors, and JSON roundtrip stability.
They also carry optional startup dependencies. The Python `plan_startup` helper
uses the same contract shape as the Rust startup graph, rejects unknown modules,
duplicate dependencies, and cycles, and emits a deterministic startup order for
editor tasks and CI gates.

#### Boot Assembly

`BootAssembly` is a no-heap helper for firmware that wants less startup
boilerplate without hiding the contracts. It builds a manifest from static
module specs, applies startup dependencies, runs admission, constructs the
runtime, boots it to `Running`, and keeps the manifest and admission reports:

```rust
let assembly = nobro_kernel::BootAssembly::<4, 4, 4, 4, 4, 4, 4, 4, 16>::build(
    &specs,
    &[nobro_kernel::StartupDependency::new(
        nobro_kernel::ModuleId::Sensor,
        nobro_kernel::ModuleId::Kernel,
    )],
    profile,
    nobro_kernel::FaultThresholds::DEFAULT,
    now_us,
)?;
assert_eq!(assembly.runtime.state(), nobro_kernel::SystemState::Running);
```

Use `BootAssemblyError` to preserve the failing phase: manifest validation,
startup graph construction, admission, or runtime boot. Use
`build_with_failure` when firmware should keep sealed manifest/admission reports
after a failed boot assembly step:

```rust
let failure = match AppBoot::build_with_failure(
    &specs,
    &dependencies,
    profile,
    nobro_kernel::FaultThresholds::DEFAULT,
    now_us,
) {
    Ok(_) => unreachable!(),
    Err(failure) => failure,
};
assert!(failure.manifest_report.verify_checksum());
```

Use `BootAssembly::reports()` or `BootAssemblyFailure::reports()` when app code
only needs to export the sealed startup reports:

```rust
let reports = failure.reports();
assert!(reports.manifest.verify_checksum());
```

`sal_adapter_demo` uses this path as the reference app assembly pattern. Adapter
preflight still writes `NOBRO_ADAPTER_COMPAT_REPORT` before hardware-facing
demo work begins, so host tools can stop at the adapter stage when descriptors
do not match the selected board profile.

#### Admission

`AdmissionController` composes manifest validation, startup ordering, quota
seeding, and capability grant construction:

```rust
let plan = nobro_kernel::AdmissionController::admit::<8, 8, 8>(
    &manifest,
    &startup_nodes,
    profile,
)?;
```

Admission failures are reported through `AdmissionReport`, using stable error
codes mirrored in `nobro-host` and `host/nobro-host-contract.json`.

#### Capability Trace

`CapabilityTrace<N>` records privileged operations only after the module passes
the same `CapabilityGrantTable` authorization used by the runtime. The trace is
a fixed-size ring buffer: it preserves deterministic replay order for retained
records, counts overwritten records, and never allocates.

```rust
let mut trace = nobro_kernel::CapabilityTrace::<8>::new();
trace.record_authorized(
    &grants,
    nobro_kernel::CapabilityTraceInput::new(
        nobro_kernel::ModuleId::Sensor,
        nobro_kernel::Capability::Bus0,
        nobro_kernel::CapabilityTraceOp::Read,
        now_us,
    )
    .args(0x68, 6),
)?;

let mut replay = [nobro_kernel::CapabilityTraceRecord::EMPTY; 4];
let copied = trace.copy_replay(
    nobro_kernel::CapabilityReplayScope::exact(
        nobro_kernel::ModuleId::Sensor,
        nobro_kernel::Capability::Bus0,
    ),
    &mut replay,
);
```

General modules do not call raw protected runtime primitives: those methods are
crate-private and `ModuleCtx` records attempt/completion or fault around the
operation. The foreign host boundary follows the same rule. An absent retained
record is meaningful for these governed operations when `dropped() == 0`;
trusted platform code is outside this module-security trace.

#### Runtime

`Runtime` is the fixed-capacity control plane for admitted applications. It
owns:

- module state via `ModuleRuntimeGuard`
- mailbox IPC
- software alarms
- key-value configuration
- quota reservations and releases
- recovery coordination
- degraded-mode reports
- event-log and health reports

Module-facing operations use the identity-bound context:

```rust
ctx.send(receiver, kind, arg0, arg1)?;
ctx.schedule_once(alarm_id, delay_us)?;
ctx.kv_set(key, value)?;
let sample = ctx.pool_alloc(kind, len, deadline_us)?;
```

Use `watchdog_expired_count(now_us)` for a non-mutating liveness precheck.
Use `sweep_watchdogs(now_us)` when the runtime should route expired modules
through recovery and update missed heartbeat counters.
Use `record_error_with_plan` or `record_watchdog_expired_with_plan` when a
supervisor needs the updated health/lifecycle state and the next bounded
recovery steps in one result:

```rust
let planning = runtime.record_error_with_plan::<4>(
    nobro_kernel::ModuleId::Sensor,
    nobro_kernel::KernelError::SensorReadFail,
    now_us,
    nobro_kernel::RecoveryPlanPolicy::DEFAULT,
)?;
assert!(planning.plan.deadline_us >= now_us);
```

Use `apply_recovery_step(step, now_us, hooks)` when a firmware loop or host
simulator dispatches a recovery step. `ModuleLifecycleHooks` is the executable
adapter boundary for notify, retry, quiesce, stop/start, self-test, heartbeat,
and resume. A hook error is returned as `RuntimeError::ModuleHook`, and the
runtime does not advance the module to active state after a failed operation.

The `check-watchdog-matrix` CLI validates non-mutating liveness prechecks,
expiry mutation, heartbeat reset, multi-module expiry, and capacity errors.
The `check-scheduler-matrix` CLI validates on-time ticks, tolerated early/late
jitter, missed deadlines, 32-bit time wraparound, counter reset, and invalid
scheduler configuration.

#### Executor Power And Structured Faults

`KernelExecutor::run_cycle` accepts a `PowerPlatform` implementation. When no
work is due, it programs the absolute wake deadline before entering the mode
chosen by `ExecutorPower`; every completed poll automatically charges measured
active time to its module's energy profile. Executor suspend/resume methods call
fallible peripheral hooks before committing module state.

Use `HealthFault` when subsystem context matters. It combines `KernelError` with
`FaultContext { source, code, detail0, detail1 }`. A `FaultPolicy` can retain
state and receives the module plus updated health counters; HealthReport v2
exports the latest context.

`EventLog` exposes non-mutating capacity helpers such as `is_full()`,
`remaining_capacity()`, `latest_sequence()`, and `has_dropped_events()`. Use
them when deciding whether to export a diagnostic report, compact a host trace,
or raise degraded-mode pressure without pushing another event.
The `check-event-log-matrix` CLI validates fixed-ring capacity accounting,
overwrite pressure, recent-record order, severity thresholds, zero-capacity
drop accounting, and invalid input handling.

Python host tooling mirrors quota accounting and degraded-mode planning through
`QuotaLedgerSimulator` and `DegradePlannerSimulator`. These helpers are for
design review, VS Code tasks, CI checks, and package examples; realtime firmware
still uses the Rust control plane.
The `check-quota-matrix` CLI validates fixed-capacity registration, reserve,
release, total-use, strict module identity, limit enforcement, underflow, and
overflow paths.
The `check-degrade-matrix` CLI validates flash, RAM, pool, module-limit,
same-criticality, planner-capacity, and essential-module pressure paths.
The `check-startup-matrix` CLI validates no-dependency, dependency-chain,
fan-in/fan-out, dependency-impact, unknown-node, self-cycle, duplicate-edge,
and cycle paths for startup graph construction.
The `check-boot-summary-matrix` CLI validates all-pass, missing-stage,
corrupt-checksum, failed-adapter, in-progress-stage, diagnostic-code, and
status-count paths for boot report summaries.
The `check-bundle-matrix` CLI validates contract bundle roundtrip, capability
ownership, module naming, AI/ROS uniqueness, hard-realtime deadline, and startup
dependency error paths.
The `check-report-matrix` CLI validates fixed report status classes, checksum
handling, failure labels, and decoded runtime, AI model, and ROS bridge fields.
`AiInvocationConstraints` and `preflight_ai_invocation` validate AI inference
admission before any model or endpoint is contacted. They check buffer sizes,
scratch and arena RAM, non-zero local arena declarations, module capability
declarations, route budget, stale snapshot policy, degraded fallback,
unavailable routes, and endpoint circuit state.
`RuntimeDrillSimulator` composes the same host-side planning and quota checks
with fixed-ring event logging and recovery escalation, which makes it useful for
reviewing a complete control-plane pressure scenario before writing board code.
Its `RecoverySummary` output gives stable retry, notification, reboot, final
state, and self-healing flags for automated review.
The `check-runtime-drill` CLI wraps the same scenario in a pass/fail gate for
disabled-module count, reboot count, and dropped event-log records.
The `check-recovery-matrix` CLI validates deterministic ignore, retry, notify,
reboot, OK-reset, fixed-plan execution, and output-buffer backpressure paths
for self-healing review.

`build_project_template` emits contract-first starter templates for standalone
SDK, Arduino, PlatformIO, Python host, and Python board bridge workflows. It
returns an in-memory file manifest so editors, CI jobs, and package tools can
decide where to write files without hiding the generated contract content.
`materialize_project_template` writes that manifest only after validating each
relative path, and it refuses to overwrite existing files unless the caller asks
for overwrite behavior.
`validate_project_template` checks the generated directory shape and validates
`nobro-contract.json`, then verifies generated VS Code task metadata for the
detected target. This gives host tools a quick onboarding gate without building
firmware or contacting a board. The `check-project` CLI returns non-zero when
this validation fails.
`repair_project_template` can rebuild stale or missing `.vscode/tasks.json`
metadata for a detected starter target without rewriting application files or
contracts.
The `check-starter-templates` CLI materializes and validates every supported
starter target in a temporary directory, then removes the generated files.
Task validation checks the expected command arguments as well as labels, so a
stale task cannot keep the right name while calling the wrong host tool.
Starter templates also include VS Code task metadata for that same project
check. Python host templates add runtime drill, runtime drill gate, AI route,
AI route matrix, AI preflight matrix, ROS preflight matrix, recovery matrix,
watchdog matrix, scheduler matrix, event log matrix, quota matrix, degrade matrix, startup
matrix, boot summary matrix, bundle matrix, and report matrix gate tasks.
Python board bridge templates add an offline bridge smoke task for MicroPython,
CircuitPython, and mPython-style development.
Host tooling can also run `sample-startup` to print a JSON startup dependency
plan for the reference runtime module set, or `check-startup-matrix` to verify
startup graph edge cases before adding board-specific adapters.
Rust firmware can call `StartupGraph::dependency_impact` to compute the
transitive modules affected by a faulted dependency before building a recovery
adapter action sequence:

```rust
let impact = startup.dependency_impact::<4>(nobro_kernel::ModuleId::Hal)?;
for module in impact.affected.iter().take(impact.affected_count).flatten() {
    let _ = module; // Quiesce affected modules before restarting the root.
}
```

`check-boot-summary-matrix` verifies first-diagnostic priority, diagnostic-code
layout, checksum corruption, failed-adapter labels, in-progress reports, and
per-status counts for the same host report path. `check-report-matrix` keeps the
individual fixed-report decoders locked to stable status and domain-field
semantics.

#### Scheduler

`Scheduler` tracks deadline ticks, max jitter, and deadline misses without heap
allocation. The default tick period is 20,000 us. Use
`set_jitter_tolerance_us()` to tune the miss threshold for a board profile,
host simulation, or timing source, and use `stats()` when exporting scheduler
health to diagnostics:

```rust
nobro_kernel::Scheduler::reset_stats();
nobro_kernel::Scheduler::set_jitter_tolerance_us(25);
let stats = nobro_kernel::Scheduler::stats();
assert_eq!(stats.jitter_tolerance_us, 25);
```

Use `check-scheduler-matrix` before packaging to verify the host mirror for
on-time ticks, tolerated early/late jitter, deadline misses, wraparound, reset,
and invalid-configuration paths.

#### Recovery

`RecoveryCoordinator` routes faults through health counters, lifecycle state,
actions, and event records. Recovery is module-scoped by default; global reset
policy should stay outside hot-path adapters.

`RecoveryPlan<N>` turns a `RecoveryOutcome` into ordered fixed-capacity steps
with due times, per-step budgets, and a total deadline:

```rust
let outcome = nobro_kernel::RecoveryOutcome {
    module: nobro_kernel::ModuleId::Sensor,
    error: nobro_kernel::KernelError::SensorReadFail,
    action: nobro_kernel::Action::RebootModule,
    state: nobro_kernel::SystemState::Recovering,
};
let plan = nobro_kernel::RecoveryPlan::<4>::from_outcome(
    outcome,
    now_us,
    nobro_kernel::RecoveryPlanPolicy::DEFAULT,
)?;
assert_eq!(plan.len, 4);
```

When a rebooted module is a shared dependency, feed startup impact data into the
planner so dependent modules are quiesced before the root restart and resumed in
startup order afterward:

```rust
let impact = startup.dependency_impact::<4>(nobro_kernel::ModuleId::Bus)?;
let plan = nobro_kernel::RecoveryPlan::<8>::from_outcome_with_impact(
    outcome,
    &impact,
    now_us,
    nobro_kernel::RecoveryPlanPolicy::DEFAULT,
)?;
```

Use `RecoveryPlanPolicy` to tune notify, retry, restart, verification, resume,
and maximum total recovery budgets. Capacity and budget failures are explicit
errors, so self-healing can be reviewed before being attached to board-specific
restart or power-control code.
`RecoveryStormPolicy` sets a bounded cooldown for identical module/error/action
work. Health and fault counters continue to advance, while duplicate event and
lifecycle work is coalesced; `RecoveryOutcome::coalesced` and
`suppressed_faults(module)` expose the decision. Coalesced outcomes cannot be
converted into duplicate recovery plans.
Both manifest-level and runtime-global `FaultThresholds` are validated:
notification must be nonzero and reboot escalation cannot precede notification.
Invalid global thresholds fail runtime construction as
`RuntimeError::FaultThreshold` before any module state is activated.
Runtime helpers return `RecoveryPlanning<N>`, which pairs the committed
`RecoveryOutcome` with the generated plan.
Use `Runtime::record_error_with_plan_and_impact` or
`Runtime::record_watchdog_expired_with_plan_and_impact` when the caller already
has startup impact data for a shared dependency. The planner validates that the
impact root matches the recovery outcome module before emitting dependent-module
steps.
Use `HotReloadPlan` and `Runtime::reload_module` for bounded module-slot
replacement. The runtime suspends the module, releases registered resources,
then requires `ModuleReloadHooks` to unmount, mount the requested revision,
self-test, verify a heartbeat, and resume. Kernel and disabled-module requests
are rejected. Hook failure is fail-closed: the module remains non-active. HAL
or adapter code supplies both the module-slot hooks and a `LeaseReleaser`, so
board-specific mechanics stay outside the kernel:

```rust
struct HalLeaseReleaser;

impl nobro_kernel::LeaseReleaser for HalLeaseReleaser {
    fn release_all_for_owner(&mut self, owner: u8) -> usize {
        // Bridge to the selected HAL lease backend.
        let _ = owner;
        0
    }
}

let mut leases = HalLeaseReleaser;
let outcome = runtime.reload_module::<5, _, _>(
    nobro_kernel::ModuleReloadRequest::new(
        nobro_kernel::ModuleId::Sensor,
        7,
        3,
        now_us,
        nobro_kernel::HotReloadPolicy::DEFAULT,
    ),
    &mut leases,
    &mut module_slot,
)?;
assert_eq!(outcome.plan.len, 5);
```

Use `next_due`, `due_count`, `remaining_count`, and `copy_due` to inspect
time-ready recovery steps from a firmware loop or host simulator without
mutating the plan or executing board-specific actions.
Use `RecoveryPlanExecution<N>` when a loop needs to advance the plan without
replaying already-dispatched work:

```rust
let mut execution = nobro_kernel::RecoveryPlanExecution::from_plan(plan);
let empty = nobro_kernel::RecoveryStep::new(
    nobro_kernel::ModuleId::Kernel,
    nobro_kernel::RecoveryStepKind::Observe,
    0,
    0,
);
let mut due = [empty; 2];
let dispatch = execution.dispatch_due(now_us, &mut due);
for step in due.iter().take(dispatch.dispatched) {
    runtime.apply_recovery_step(*step, now_us, &mut lifecycle_hooks)?;
}
```

The execution cursor owns no heap memory, uses caller-owned output buffers, and
reports remaining steps, next due time, consumed budget, overdue work, and
completion status.
Pair `RecoveryPlanExecution` with `Runtime::apply_recovery_step` to keep ordered
dispatch, executable platform actions, and module-state bookkeeping together.

### Network API

`nobro-net` provides no-heap mesh primitives for routing, secure links,
store-forward delivery, owned OTA image assembly, and fleet rollout planning.
`OtaImageAssembler<BYTES, CHUNKS>` validates image/chunk geometry, owns
out-of-order payloads, reports the first hole, and returns the image only after
complete SHA-256 verification.
`FleetOtaOrchestrator<N>` stages OTA updates as canary-first waves, enforces a
maximum number of active updates, blocks rollout when fleet health falls below
policy, and rolls failed nodes back without allocating:

```rust
let mut fleet = nobro_net::FleetOtaOrchestrator::<4>::new();
fleet.register(nobro_net::FleetOtaNode::new(1, 1))?;
fleet.register(nobro_net::FleetOtaNode::new(2, 1))?;

let wave = fleet.stage_next_wave(2, nobro_net::FleetOtaPolicy::DEFAULT)?;
assert_eq!(wave.phase, nobro_net::FleetOtaPhase::Canary);
fleet.mark_installing(1)?;
fleet.complete_node(1, true)?;
```

After the canary confirms, later calls stage normal rollout waves up to the
configured parallelism. Failed nodes move through rollback and eventually
blocked state according to the failure policy, giving host tooling or firmware
a deterministic rollout controller before transport-specific OTA packets are
sent.

`stage_next_wave` is useful for isolated policy simulation. Production update
paths should call `stage_verified_wave`, which accepts only the private-field
`VerifiedSignedImage` produced by `nobro-secure`'s asymmetric verification.

### HAL API

#### BoardDesc

`BoardDesc` exposes stable board facts:

- platform and board identifiers
- application flash origin
- memory and module budgets
- critical pins
- servo timing defaults

Host-facing board data can be exported through `BoardProfileReport`.

Board profile fixtures make identity, capacity, critical pins, and servo
defaults reviewable without switching Cargo features:

```rust
for fixture in nobro_hal::BOARD_PROFILE_FIXTURES {
    let report = fixture.report();
    assert!(report.verify_checksum());
}
```

`BoardPackage` combines `BoardDesc` with boot layout, flash region, RAM region,
capacity, and critical pins:

```rust
let package = nobro_hal::Board::package();
assert_eq!(package.validate(), Ok(()));
```

`BoardPackageError` identifies invalid board data such as empty capacity,
unaligned flash origin, empty memory regions, or duplicate critical pins.

When `nobro-kernel` is built with the `hal-profile` feature, admission limits
can be derived directly from the active package:

```rust
let profile = nobro_kernel::SystemProfile::from_board_package(
    &nobro_hal::ACTIVE_BOARD_PACKAGE,
)?;
```

`BoardPackageReport` exports the same package contract as a fixed-layout host
record:

```rust
let report = nobro_hal::BoardPackageReport::from_package(&nobro_hal::ACTIVE_BOARD_PACKAGE);
assert!(report.verify_checksum());
```

Board package fixtures make current board layouts reviewable without switching
Cargo features:

```rust
for fixture in nobro_hal::BOARD_PACKAGE_FIXTURES {
    assert_eq!(fixture.package.validate(), Ok(()));
    assert!(fixture.report().verify_checksum());
}
```

#### Leases

Portable code uses `HalLease` with neutral `LeaseId` class/instance values such
as `LeaseId::PRIMARY_I2C`; platform adapters map those IDs to concrete hardware.
Board-specific drivers may use `ResourceLease` and `LeaseGuard` directly for
exclusive ownership of shared
peripherals. A driver should acquire a lease, perform bounded work, and let the
guard release the resource. Recovery supervisors can inspect the current owner
and release every lease held by a faulted owner without disturbing other
modules.

Guards are generation-tagged and non-clonable. Safe I2C, SPI, radio, PWM, and nRF
scheduling-session operations revalidate the exact acquisition before touching hardware;
owner-scoped recovery invalidates stale sessions, quiesces the peripheral, clears nRF
interrupt/DMA routing state, and only then publishes the slot as free. Raw low-level
register APIs are `unsafe` and exist for gated compatibility integrations.

```rust
let guard = nobro_hal::ResourceLease::acquire_guard(nobro_hal::Resource::Twim0, module_id)?;
assert_eq!(nobro_hal::ResourceLease::owner(nobro_hal::Resource::Twim0), Some(module_id));
drop(guard);
nobro_hal::ResourceLease::release_all_for_owner(module_id);
```

#### Portable Bus Transactions

`HalI2c` exposes write, read, and repeated-start write/read operations;
`HalSpi` exposes bounded full-duplex transfers. Each provider declares
`TRANSFER_MODE`, so scheduling and evaluation code can distinguish polling from
DMA instead of assuming one platform-wide behavior. The current deep backend
reports polling I2C and DMA SPI.

Owned-peripheral ports use `HalAlarm`, `HalPwmChannel`, and `HalByteIo` for a
one-shot deadline, a constructed PWM channel, and bounded USB/serial byte I/O.
This avoids global singleton APIs on MCUs whose HAL carries ownership and pin
lifetimes in the provider type.

#### Event Capture

`HalEventCapture` is the portable abstraction for event-to-timestamp routing.
The nRF52840 backend maps it to PPI. Future ports can map it to another trigger
fabric without changing app code.

### SAL API

#### BusSal

Use for I2C, SPI, and UART-like transactions that need lease-aware access.

```rust
trait BusSal {
    type Error;
    fn write_read(&mut self, addr: u8, tx: &[u8], rx: &mut [u8])
        -> Result<(), Self::Error>;
}
```

#### SensorSal

Use for sampled data. Payload bytes travel through `Sample` tickets and static
pools rather than heap buffers.

```rust
if let Some(sample) = sensor.poll()? {
    runtime.publish_sample(sample)?;
}
```

`nobro-adapter-sensor-stub` provides a software fixture for adapter and
recovery tests. The default mode emits a plausible IMU sample every 50 polls;
custom profiles can model silent sensors, periodic adapter errors, or
implausible payloads:

```rust
let mut sensor = nobro_adapter_sensor_stub::SensorStub::with_profile(
    2,
    nobro_adapter_sensor_stub::SensorStubProfile::new(
        1,
        nobro_adapter_sensor_stub::SensorStubMode::BadDataEvery(4),
    ),
);
let sample = sensor.poll_at(1_000)?;
```

#### ActuatorSal

Use for deadline-aware output.

```rust
actuator.set_duty_us(channel, 1500, deadline_us)?;
```

#### StreamSal, RadioSal, CryptoSal

`StreamSal` handles framed byte streams, `RadioSal` handles radio process loops
and packet movement, and `CryptoSal` keeps cryptographic services behind a
portable capability surface.

#### AiInferenceSal

Use `AiInferenceSal` for bounded local, sidecar, hybrid, or remote inference.
The contract declares backend kind, model identity, max input/output sizes,
arena size, and timeout. Callers provide the input and output buffers so an AI
adapter does not hide heap allocation behind a model call.

```rust
let contract = ai.contract();
assert!(contract.input_bytes_max <= 512);

let input = [0u8; 16];
let mut output = [0u8; 32];
let result = ai.infer(
    nobro_sal::AiInferenceRequest::new(contract.model_id, &input, deadline_us),
    &mut output,
)?;
assert!(usize::from(result.output_len) <= output.len());
```

Hard-realtime modules should not wait directly on remote inference. They should
consume a fresh result snapshot or a degraded fallback state.

Use `preflight_ai_invocation` before calling an adapter. It checks model ID,
input size, output capacity, scratch and arena RAM, local arena declarations,
route availability, degraded fallback, stale-snapshot policy, and endpoint
circuit state without contacting a model or remote endpoint:

```rust
let limits = nobro_sal::AiInvocationLimits::new(
    output.len() as u32,
    128,
    8 * 1024,
    25_000,
);
let report = nobro_sal::preflight_ai_invocation(
    contract,
    policy,
    state,
    nobro_sal::AiInferenceRequest::new(contract.model_id, &input, deadline_us),
    limits,
);
assert!(report.passing());
```

Use `AiRoutePolicy` when an application can choose among local inference, an
edge sidecar, a third-party API, or a hybrid fallback path. The policy is a
fixed-size control record: it compares the model timeout with the caller's
budget, trips a small endpoint circuit breaker after repeated failures, and
returns a route target without allocating memory.
The stale snapshot window is contract-aware: a zero policy window inherits the
model contract's `stale_after_us`, while a non-zero policy uses the stricter of
the policy and model windows.

```rust
let policy = nobro_sal::AiRoutePolicy::new(
    nobro_sal::AiRoutePreference::HybridFallback,
    50_000,
    3,
);
let state = nobro_sal::AiRuntimeState::new(
    true,   // local model is loaded
    true,   // endpoint transport is ready
    12_000, // last good inference age
    0,      // consecutive endpoint failures
);
let decision = policy.decide(contract, state, 20_000);
assert_ne!(decision.target, nobro_sal::AiRouteTarget::Unavailable);
```

Adapters can export the same model and routing boundary as a host-readable
report:

```rust
let report = nobro_sal::AiModelContractReport::from_contract_and_policy(
    contract,
    Some(policy),
);
assert!(report.verify_checksum());
assert_eq!(report.route_preference, nobro_sal::AiRoutePreference::HybridFallback as u32);
```

#### ROS-Style Bridges

ROS and micro-ROS compatibility should be implemented through adapters and
metadata, not as kernel dependencies. Topic-like streams map to bounded queues,
service-like calls map to fixed request/response records, action-like work maps
to goal/feedback/result records, and parameters map to fixed-capacity
configuration.

`RosBridgeSal` provides the Rust-side bounded bridge surface. Names and message
types are represented by stable hashes so the realtime path does not carry
dynamic strings. Inputs and outputs remain caller-owned buffers.

```rust
let topic = nobro_sal::RosTopicContract::new(0x10, 0x20, 4, 64);
let service = nobro_sal::RosServiceContract::new(0x30, 16, 16, 50_000);
let contract = nobro_sal::RosBridgeContract::from_parts(
    nobro_sal::RosBridgeTransport::Serial,
    0xA11CE,
    &[topic],
    &[service],
    &[],
    &[],
);

assert_eq!(contract.topic_count, 1);
assert!(contract.total_buffer_bytes <= 512);
```

Bridge adapters can also publish a compact report that summarizes transport,
entity counts, total buffer demand, and maximum timeout:

```rust
let report = nobro_sal::RosBridgeContractReport::from_contract(contract);
assert!(report.verify_checksum());
assert_eq!(report.transport, nobro_sal::RosBridgeTransport::Serial as u32);
```

Use ROS bridge preflight helpers before moving payloads through an adapter:

```rust
let topic_check = nobro_sal::preflight_ros_topic(topic, 32);
let service_check = nobro_sal::preflight_ros_service(service, 16, 16, 50_000);
assert!(topic_check.passing());
assert!(service_check.passing());
```

`preflight_ros_topic`, `preflight_ros_service`, `preflight_ros_action`, and
`preflight_ros_parameter` check payload bounds, response capacity, queue depth,
and timeout budgets without contacting a ROS agent or transport.

Python bridge descriptors emit the same stable FNV-1a 32-bit hashes alongside
readable names, so host-generated metadata can be reviewed by people and still
map cleanly to Rust `RosBridgeContract` fields.

Python tooling also mirrors `AiRoutePolicy` for host-side simulations and editor
workflows. Use it to validate route decisions before the same policy is wired
into Rust or C/C++ firmware code.
The `check-ai-route` CLI wraps a sample policy decision in a pass/fail gate for
target selection, stale snapshot reuse, degraded fallback, unavailable routes,
and endpoint circuit-breaker state without contacting a network service.
Use its backend, preference, budget, readiness, stale-age, and failure-count
arguments to model on-device, edge-sidecar, remote API, and hybrid inference
paths in CI.
The `check-ai-route-matrix` CLI runs a deterministic compatibility matrix for
local, remote API, edge sidecar, stale snapshot, degraded fallback, and
unavailable route outcomes.
The `check-ai-preflight-matrix` CLI validates inference-call admission for
buffer bounds, arena and scratch RAM, declared local arenas, declared AI
capabilities, route budget, stale snapshot limits, degraded fallback,
unavailable routes, and endpoint circuit state.
The `check-ros-preflight-matrix` CLI validates ROS bridge-call admission for
topic payload bounds, service/action response capacity, queue depth, parameter
value size, and timeout budget.
The `check-public-headers` CLI validates public C/C++ report structs, helper
functions, forwarding headers, and AI/ROS preflight error-bit coverage.
The `check-python-surface` CLI validates top-level Python package re-exports
against `__all__` and required host APIs without importing the package.
The `check-cli-command-surface` CLI validates command registration and README
coverage for release-check and onboarding commands.
The `check-software-surface` CLI composes host contract, SDK/package metadata,
public header, Python public surface, CLI command surface, starter template, AI
route matrix, AI preflight matrix, ROS preflight matrix, recovery matrix,
watchdog matrix, scheduler matrix, event log matrix, quota matrix,
degrade matrix, startup matrix, boot summary matrix, bundle matrix, report
matrix, and runtime drill validation for pre-package review.

### Host API

`nobro-host` mirrors all report constants:

```rust
use nobro_host::{HostReport, RuntimeReport, RUNTIME_REPORT_SYMBOL};

fn inspect(report: &RuntimeReport) {
    assert_eq!(RuntimeReport::SYMBOL, RUNTIME_REPORT_SYMBOL);
    let status = report.status();
    let checksum_ok = report.verify_checksum();
}
```

Boot diagnostics can be collapsed into a fixed summary:

```rust
let summary = reports.summary();
assert_eq!(summary.first_stage_label(), "runtime");
assert_eq!(summary.pass_count, nobro_host::BOOT_REPORT_STAGE_COUNT as u8);
```
Host tools should prefer labels from `nobro-host` instead of embedding numeric
tables locally.

## Host contract (the JSON ABI mirror)

The host contract defines the data that external tools can read from firmware
images or runtime memory. The JSON mirror is:

```text
host/nobro-host-contract.json
```

The Rust mirror is:

```text
core/crates/nobro_host
```

### Stable Labels

Module tag labels include kernel, HAL, bus, radio, sensor, actuator, stream,
crypto, AI, and app modules. Capability labels include timebase, deadline
timer, event capture, bus, radio, servo PWM, stream, crypto, sample pool, host
report, AI inference, and AI endpoint ownership.

### Report Symbols

| Symbol | Meaning |
| ------ | ------- |
| `NOBRO_BOARD_PROFILE_REPORT` | Selected board, memory origin, budgets, and critical pins |
| `NOBRO_BOARD_PACKAGE_REPORT` | Boot layout, flash/RAM regions, board capacity, critical pins, and package validation result |
| `NOBRO_MANIFEST_REPORT` | Static module graph validity, capability bits, budget use, and error context |
| `NOBRO_ADAPTER_COMPAT_REPORT` | Adapter inventory compatibility before app admission |
| `NOBRO_AI_MODEL_REPORT` | AI backend, model ID, input/output bounds, arena bytes, timeout, and route policy |
| `NOBRO_ROS_BRIDGE_REPORT` | ROS-style bridge transport, entity counts, buffer demand, and maximum timeout |
| `NOBRO_ADMISSION_REPORT` | Admission result after manifest, startup, quota, and capability checks |
| `NOBRO_RUNTIME_REPORT` | Runtime lifecycle, mailbox pressure, alarms, KV writes, quota use, and event pressure |
| `NOBRO_HEALTH_REPORT` | Module health counters and latest recovery context |
| `NOBRO_EVENT_LOG_REPORT` | Fixed event-ring summary |
| `NOBRO_MODULE_RUNTIME_REPORT` | Module state counts and latest state transition |
| `NOBRO_DEGRADE_APPLICATION_REPORT` | Latest degraded-mode application result |
| `NOBRO_EVAL_REPORT` | Phase 1 resource scheduling evaluation record |
| `NOBRO_SAL_EVAL_REPORT` | SAL adapter evaluation record |

### Status Model

Reports use the same status categories:

- `missing`: zeroed report slot
- `in_progress`: valid header, incomplete report
- `pass`: complete and checksum-valid success
- `fail`: complete and checksum-valid domain failure
- `corrupt`: invalid header, version, or checksum

Host tools should decode the first non-passing boot stage in this order:

1. board profile
2. board package
3. manifest
4. adapter compatibility
5. admission
6. runtime

### Boot Summary

`nobro-host` exposes `BootReports::summary()` for tools that need one compact
view of boot state. The summary includes the first diagnostic, all six report
slots, diagnostic code, and per-status counts. Tools should use this helper
before rendering user-facing text.

Python host tooling mirrors this shape in `BootReportSummary.to_dict()` and the
`summarize-boot` CLI command, including the same diagnostic code layout and
per-status count fields.
Use `check-boot-summary-matrix` to validate all-pass, missing-stage,
corrupt-checksum, failed-adapter, in-progress-stage, diagnostic-code, and
status-count paths before changing report layouts or host tooling.
### Checksum Rule

Fixed reports use XOR checksums over every `u32` field except `checksum`.
Timestamps wider than `u32` are split into low and high words.

### Diagnostic Code

Boot diagnostic code layout:

```text
stage_code << 24 | status_class << 16 | error_code_low16
```

### AI And ROS Tables

`ai_contracts` defines stable numeric codes for AI backend kinds, route
preferences, route targets, and the `NOBRO_AI_MODEL_REPORT` layout.
`ros_bridge_contracts` defines the stable FNV-1a UTF-8 hash policy, ROS-style
entity kinds, transport codes, and the `NOBRO_ROS_BRIDGE_REPORT` layout used by
Python, C/C++, and Rust bridge metadata.

Use `nobro-host` helper labels rather than duplicating numeric maps in host
tools.
