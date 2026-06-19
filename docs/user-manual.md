# NobroRTOS User Manual

This manual helps a firmware developer understand the repository, run local
validation, and assemble a small NobroRTOS application.

## Mental Model

NobroRTOS is organized around explicit contracts:

- A board describes memory layout, critical pins, and capacity budgets.
- A manifest describes modules, required capabilities, owned capabilities,
  memory budgets, deadline contracts, and fault thresholds.
- The admission controller validates the manifest before runtime work begins.
- The runtime owns mailbox IPC, alarms, key-value configuration, quotas,
  degraded-mode state, and recovery coordination.
- SAL traits keep applications independent from concrete drivers.
- Host reports make boot, admission, runtime, health, and recovery state
  readable from outside the firmware image.

## Workspace Setup

Install Rust and the embedded target:

```powershell
rustup target add thumbv7em-none-eabihf
```

Use `_work/` for generated files:

```powershell
$env:CARGO_TARGET_DIR = (Resolve-Path '_work').Path + '\cargo-target'
```

Run commands from `core/` unless noted otherwise:

```powershell
cd core
cargo test -p airon-kernel --target x86_64-pc-windows-msvc
cargo test -p airon-sal --target x86_64-pc-windows-msvc
cargo test -p airon-host --target x86_64-pc-windows-msvc
```

## Build Profiles

`cargo check --workspace` uses `core/.cargo/config.toml`, so the default build
target is `thumbv7em-none-eabihf`.

Board layout is selected by feature:

| Feature | App origin | Use |
| ------- | ---------- | --- |
| `board-promicro-nosd` | `0x1000` | ProMicro-style nRF52840 images without SoftDevice |
| `board-nicenano-s140` | `0x26000` | nRF52840 images that coexist with SoftDevice S140 v6 layout |

Example:

```powershell
cargo check -p sal-adapter-demo --no-default-features --features board-promicro-nosd
```

## Creating An Application

A NobroRTOS application should assemble existing contracts rather than hide
hardware behavior in private globals.

1. Select one board feature.
2. Build adapter descriptors for each enabled device.
3. Assemble a `SystemManifest` with module budgets and capabilities.
4. Build a `StartupGraph`.
5. Call `AdmissionController::admit`.
6. Construct `Runtime` from the admitted plan.
7. Export host reports for diagnostics.

Minimal manifest sketch:

```rust
use airon_kernel::{
    capability::CapabilityBits,
    manifest::{kernel_module_spec, Criticality, DeadlineContract, FaultThresholds,
               MemoryBudget, ModuleSpec, SystemManifest},
};

let mut manifest = SystemManifest::<4>::new();
manifest.push(kernel_module_spec()).unwrap();
manifest.push(ModuleSpec {
    id: 0x105,
    criticality: Criticality::SoftRealtime,
    requires: CapabilityBits::BUS0 | CapabilityBits::SAMPLE_POOL,
    owns: CapabilityBits::empty(),
    memory: MemoryBudget {
        flash_bytes: 8 * 1024,
        ram_bytes: 2 * 1024,
        pool_slots: 2,
    },
    deadline: Some(DeadlineContract {
        period_us: 5_000,
        budget_us: 300,
    }),
    faults: FaultThresholds::default(),
}).unwrap();
```

## Working With SAL

Application code should depend on `airon-sal` traits:

- `BusSal` for I2C, SPI, and UART-like bus access
- `StreamSal` for framed byte streams
- `RadioSal` for radio process loops and packet movement
- `ActuatorSal` for deadline-aware actuator output
- `SensorSal` for sampled sensor data
- `CryptoSal` for cryptographic operations

Adapters translate concrete hardware or libraries into those traits. Apps
should not call vendor headers directly.

## Diagnostics

NobroRTOS exports fixed-layout reports with `NOBRO_*` symbols. The canonical
contract is `host/nobro-host-contract.json`.

Important reports:

- `NOBRO_BOARD_PROFILE_REPORT`
- `NOBRO_BOARD_PACKAGE_REPORT`
- `NOBRO_MANIFEST_REPORT`
- `NOBRO_ADAPTER_COMPAT_REPORT`
- `NOBRO_ADMISSION_REPORT`
- `NOBRO_RUNTIME_REPORT`
- `NOBRO_HEALTH_REPORT`
- `NOBRO_EVENT_LOG_REPORT`
- `NOBRO_MODULE_RUNTIME_REPORT`
- `NOBRO_DEGRADE_APPLICATION_REPORT`

Reports are designed to be copied from memory as `repr(C)` records and checked
with simple XOR checksums.

## Cleanup

Keep generated data under `_work/`:

```powershell
Remove-Item -Recurse -Force _work\cargo-target -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force _work\artifacts -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force _work\logs -ErrorAction SilentlyContinue
```

`_work/`, target directories, firmware binaries, coverage output, and local
toolchains are ignored by Git.
