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
- `criticality`: system, hard realtime, soft realtime, utility, or optional
- `requires`: capability bits needed at runtime
- `owns`: capability bits provided or exclusively owned by the module
- `memory`: flash, RAM, and sample-pool budget
- `deadline`: optional periodic contract
- `faults`: retry and escalation thresholds

Use manifest validation before constructing runtime state:

```rust
let report = airon_kernel::manifest::ManifestReport::from_manifest(
    &manifest,
    &profile,
);
assert_eq!(report.status(), airon_kernel::report::ReportStatus::Pass);
```

### Admission

`AdmissionController` composes manifest validation, startup ordering, quota
seeding, and capability grant construction:

```rust
let plan = AdmissionController::admit::<8, 8, 8>(
    &manifest,
    &profile,
    &startup_nodes,
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

`BoardPackage` combines `BoardDesc` with boot layout, flash region, RAM region,
capacity, and critical pins:

```rust
let package = airon_hal::Board::package();
assert_eq!(package.validate(), Ok(()));
```

`BoardPackageError` identifies invalid board data such as empty capacity,
unaligned flash origin, empty memory regions, or duplicate critical pins.

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

Host tools should prefer labels from `airon-host` instead of embedding numeric
tables locally.
