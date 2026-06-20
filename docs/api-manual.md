# NobroRTOS API Manual

This manual summarizes the public crates and the core contracts used by
applications, adapters, and host tooling.

## Crate Overview

| Crate | Purpose |
| ----- | ------- |
| `airon-kernel` | Manifest, admission, runtime, quota, capability, scheduler, IPC, alarms, recovery, health, and reports |
| `airon-hal` | Board profiles, platform traits, nRF52840 implementation, leases, timers, PWM, bus, and event capture |
| `airon-sal` | Stable service traits for adapters and apps |
| `airon-host` | Host-side constants, report layouts, labels, and status helpers |

## Kernel API

### Manifest

`SystemManifest<N>` is a fixed-capacity list of `ModuleSpec` records. Each
module declares:

- `id`: stable module tag
- `criticality`: best effort, user, driver, system, or hard realtime
- `requires`: capability bits needed at runtime
- `owns`: capability bits provided or exclusively owned by the module
- `memory`: flash, RAM, and sample-pool budget
- `deadline`: optional periodic contract
- `faults`: retry and escalation thresholds

Use manifest validation before constructing runtime state:

```rust
let report = airon_kernel::ManifestReport::from_result(
    &manifest,
    manifest.validate_profile(profile),
);
assert!(report.verify_checksum());
assert_eq!(report.valid, 1);
```

### Boot Assembly

`BootAssembly` is a no-heap helper for firmware that wants less startup
boilerplate without hiding the contracts. It builds a manifest from static
module specs, applies startup dependencies, runs admission, constructs the
runtime, boots it to `Running`, and keeps the manifest and admission reports:

```rust
let assembly = airon_kernel::BootAssembly::<4, 4, 4, 4, 4, 4, 4, 4, 16>::build(
    &specs,
    &[airon_kernel::StartupDependency::new(
        airon_kernel::ModuleId::Sensor,
        airon_kernel::ModuleId::Kernel,
    )],
    profile,
    airon_kernel::FaultThresholds::DEFAULT,
    now_us,
)?;
assert_eq!(assembly.runtime.state(), airon_kernel::SystemState::Running);
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
    airon_kernel::FaultThresholds::DEFAULT,
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
let plan = airon_kernel::AdmissionController::admit::<8, 8, 8>(
    &manifest,
    &startup_nodes,
    profile,
)?;
```

Admission failures are reported through `AdmissionReport`, using stable error
codes mirrored in `airon-host` and `host/nobro-host-contract.json`.

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
for fixture in airon_hal::BOARD_PROFILE_FIXTURES {
    let report = fixture.report();
    assert!(report.verify_checksum());
}
```

`BoardPackage` combines `BoardDesc` with boot layout, flash region, RAM region,
capacity, and critical pins:

```rust
let package = airon_hal::Board::package();
assert_eq!(package.validate(), Ok(()));
```

`BoardPackageError` identifies invalid board data such as empty capacity,
unaligned flash origin, empty memory regions, or duplicate critical pins.

When `airon-kernel` is built with the `hal-profile` feature, admission limits
can be derived directly from the active package:

```rust
let profile = airon_kernel::SystemProfile::from_board_package(
    &airon_hal::ACTIVE_BOARD_PACKAGE,
)?;
```

`BoardPackageReport` exports the same package contract as a fixed-layout host
record:

```rust
let report = airon_hal::BoardPackageReport::from_package(&airon_hal::ACTIVE_BOARD_PACKAGE);
assert!(report.verify_checksum());
```

Board package fixtures make current board layouts reviewable without switching
Cargo features:

```rust
for fixture in airon_hal::BOARD_PACKAGE_FIXTURES {
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

`airon-adapter-sensor-stub` provides a software fixture for adapter and
recovery tests. The default mode emits a plausible IMU sample every 50 polls;
custom profiles can model silent sensors, periodic adapter errors, or
implausible payloads:

```rust
let mut sensor = airon_adapter_sensor_stub::SensorStub::with_profile(
    2,
    airon_adapter_sensor_stub::SensorStubProfile::new(
        1,
        airon_adapter_sensor_stub::SensorStubMode::BadDataEvery(4),
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

## Host API

`airon-host` mirrors all report constants:

```rust
use airon_host::{HostReport, RuntimeReport, RUNTIME_REPORT_SYMBOL};

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
assert_eq!(summary.pass_count, airon_host::BOOT_REPORT_STAGE_COUNT as u8);
```
Host tools should prefer labels from `airon-host` instead of embedding numeric
tables locally.
