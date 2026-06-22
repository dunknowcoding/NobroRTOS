# NobroRTOS API Manual

This manual summarizes the public crates and the core contracts used by
applications, adapters, and host tooling.

## Crate Overview

| Crate | Purpose |
| ----- | ------- |
| `nobro-kernel` | Manifest, admission, runtime, quota, capability, scheduler, IPC, alarms, recovery, health, and reports |
| `nobro-hal` | Board profiles, platform traits, nRF52840 implementation, leases, timers, PWM, bus, and event capture |
| `nobro-sal` | Stable service traits for adapters, apps, AI inference, and edge bridges |
| `nobro-host` | Host-side constants, report layouts, labels, and status helpers |

## Kernel API

### Manifest

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

### Boot Assembly

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

### Admission

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

### Runtime

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

Common runtime operations:

```rust
runtime.reserve_quota(module_id, flash_bytes, ram_bytes, pool_slots)?;
runtime.send_control_message(sender, receiver, opcode, payload)?;
runtime.schedule_alarm(module_id, deadline_us, period_us)?;
runtime.disable_module(module_id)?;
let report = runtime.runtime_report();
```

Use `watchdog_expired_count(now_us)` for a non-mutating liveness precheck.
Use `sweep_watchdogs(now_us)` when the runtime should route expired modules
through recovery and update missed heartbeat counters.
The `check-watchdog-matrix` CLI validates non-mutating liveness prechecks,
expiry mutation, heartbeat reset, multi-module expiry, and capacity errors.
The `check-scheduler-matrix` CLI validates on-time ticks, tolerated early/late
jitter, missed deadlines, 32-bit time wraparound, counter reset, and invalid
scheduler configuration.

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
fan-in/fan-out, unknown-node, self-cycle, duplicate-edge, and cycle paths for
startup graph construction.
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
scratch and arena RAM, module capability declarations, route budget, stale
snapshot policy, degraded fallback, unavailable routes, and endpoint circuit
state.
`RuntimeDrillSimulator` composes the same host-side planning and quota checks
with fixed-ring event logging and recovery escalation, which makes it useful for
reviewing a complete control-plane pressure scenario before writing board code.
Its `RecoverySummary` output gives stable retry, notification, reboot, final
state, and self-healing flags for automated review.
The `check-runtime-drill` CLI wraps the same scenario in a pass/fail gate for
disabled-module count, reboot count, and dropped event-log records.
The `check-recovery-matrix` CLI validates deterministic ignore, retry, notify,
reboot, and OK-reset recovery paths for self-healing review.

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
AI route matrix, AI preflight matrix, recovery matrix, watchdog matrix,
scheduler matrix, event log matrix, quota matrix, degrade matrix, startup
matrix, boot summary matrix, bundle matrix, and report matrix gate tasks.
Python board bridge templates add an offline bridge smoke task for MicroPython,
CircuitPython, and mPython-style development.
Host tooling can also run `sample-startup` to print a JSON startup dependency
plan for the reference runtime module set, or `check-startup-matrix` to verify
startup graph edge cases before adding board-specific adapters.
`check-boot-summary-matrix` verifies first-diagnostic priority, diagnostic-code
layout, checksum corruption, failed-adapter labels, in-progress reports, and
per-status counts for the same host report path. `check-report-matrix` keeps the
individual fixed-report decoders locked to stable status and domain-field
semantics.

### Scheduler

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

### Recovery

`RecoveryCoordinator` routes faults through health counters, lifecycle state,
actions, and event records. Recovery is module-scoped by default; global reset
policy should stay outside hot-path adapters.

## HAL API

### BoardDesc

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

### Leases

`ResourceLease` and `LeaseGuard` provide exclusive ownership for shared
peripherals. A driver should acquire a lease, perform bounded work, and let the
guard release the resource.

```rust
let mut lease = lease_table.acquire(ResourceId::Twim0, module_id)?;
bus.write_read(&mut lease, addr, tx, rx)?;
```

### Event Capture

`HalEventCapture` is the portable abstraction for event-to-timestamp routing.
The nRF52840 backend maps it to PPI. Future ports can map it to another trigger
fabric without changing app code.

## SAL API

### BusSal

Use for I2C, SPI, and UART-like transactions that need lease-aware access.

```rust
trait BusSal {
    type Error;
    fn write_read(&mut self, addr: u8, tx: &[u8], rx: &mut [u8])
        -> Result<(), Self::Error>;
}
```

### SensorSal

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

### ActuatorSal

Use for deadline-aware output.

```rust
actuator.set_duty_us(channel, 1500, deadline_us)?;
```

### StreamSal, RadioSal, CryptoSal

`StreamSal` handles framed byte streams, `RadioSal` handles radio process loops
and packet movement, and `CryptoSal` keeps cryptographic services behind a
portable capability surface.

### AiInferenceSal

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

### ROS-Style Bridges

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
buffer bounds, arena and scratch RAM, declared AI capabilities, route budget,
stale snapshot limits, degraded fallback, unavailable routes, and endpoint
circuit state.
The `check-software-surface` CLI composes host contract, SDK/package metadata,
public header, starter template, AI route matrix, AI preflight matrix, recovery
matrix, watchdog matrix, scheduler matrix, event log matrix, quota matrix,
degrade matrix, startup matrix, boot summary matrix, bundle matrix, report
matrix, and runtime drill validation for pre-package review.

## Host API

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
