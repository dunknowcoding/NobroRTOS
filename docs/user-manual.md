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
cargo test -p nobro-kernel --target x86_64-pc-windows-msvc
cargo test -p nobro-sal --target x86_64-pc-windows-msvc
cargo test -p nobro-host --target x86_64-pc-windows-msvc
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

For small apps, `BootAssembly` can do steps 3 through 6 while keeping the
manifest, startup graph, admission result, and runtime visible:

```rust
use nobro_kernel::{
    kernel_module_spec, BootAssembly, Capability, CapabilitySet, Criticality,
    DeadlineContract, FaultThresholds, MemoryBudget, ModuleId, ModuleSpec,
    StartupDependency, SystemProfile,
};

type AppBoot = BootAssembly<4, 4, 4, 4, 4, 4, 4, 4, 16>;

let specs = [
    kernel_module_spec(
        MemoryBudget::new(16 * 1024, 4 * 1024, 4),
        DeadlineContract::new(20_000, 10),
    ),
    ModuleSpec::new(ModuleId::Sensor, Criticality::Driver)
        .requires(CapabilitySet::empty().with(Capability::SamplePool))
        .memory(MemoryBudget::new(8 * 1024, 2 * 1024, 2)),
];

let boot = AppBoot::build(
    &specs,
    &[StartupDependency::new(ModuleId::Sensor, ModuleId::Kernel)],
    SystemProfile::new(64 * 1024, 16 * 1024, 8, 4),
    FaultThresholds::DEFAULT,
    0,
)?;

assert!(boot.manifest_report.verify_checksum());
assert!(boot.admission_report.verify_checksum());
```

Firmware that depends on `nobro-hal` can enable `nobro-kernel/hal-profile` and
derive the profile from the active board package:

```rust
let profile = SystemProfile::from_board_package(&nobro_hal::ACTIVE_BOARD_PACKAGE)?;
```

For diagnostics-first startup code, use `build_with_failure` and export the
reports from the returned `BootAssemblyFailure` before halting or entering a
maintenance path:

```rust
let failure = match AppBoot::build_with_failure(
    &specs,
    &[StartupDependency::new(ModuleId::Sensor, ModuleId::Kernel)],
    SystemProfile::new(64 * 1024, 16 * 1024, 8, 4),
    FaultThresholds::DEFAULT,
    0,
) {
    Ok(_) => unreachable!(),
    Err(failure) => failure,
};

assert!(failure.manifest_report.verify_checksum());
```

Both successful assemblies and `BootAssemblyFailure` expose `reports()`, so
firmware can publish the manifest and admission reports through one helper.

The `sal_adapter_demo` app follows this pattern: it assembles the manifest,
startup graph, admission plan, and runtime through `BootAssembly`, then writes
the adapter compatibility report before entering hardware-facing demo code.

## Working With SAL

Application code should depend on `nobro-sal` traits:

- `BusSal` for I2C, SPI, and UART-like bus access
- `StreamSal` for framed byte streams
- `RadioSal` for radio process loops and packet movement
- `ActuatorSal` for deadline-aware actuator output
- `SensorSal` for sampled sensor data
- `CryptoSal` for cryptographic operations
- `AiInferenceSal` for bounded on-device, sidecar, hybrid, or remote inference

Adapters translate concrete hardware or libraries into those traits. Apps
should not call vendor headers directly.

For bring-up and CI, `nobro-adapter-sensor-stub` can run as a deterministic
sensor fixture. It can emit plausible IMU samples, stay silent, inject periodic
adapter errors, or produce implausible payloads without requiring external
hardware.

## AI And Robotics Bridges

AI modules should declare their model identity, backend kind, memory budget,
input/output bounds, timeout, and recovery behavior before admission. Local
TinyML, generated C++ model libraries, accelerator sidecars, companion
computers, and third-party API calls should all enter the system as adapters
rather than private schedulers.

ROS and micro-ROS compatibility should be implemented as bridge adapters.
Topic-like data should use bounded queues, service-like calls should use fixed
request/response records, action-like work should expose bounded goal,
feedback, and result state, and parameters should map to fixed-capacity
configuration.

AI model contracts and ROS bridge contracts can be exported as fixed reports, so
host tools can inspect model memory, timeout, stale-result policy, bridge entity
counts, total buffer demand, and transport choice without running a hardware
probe.

## Diagnostics

NobroRTOS exports fixed-layout reports with `NOBRO_*` symbols. The canonical
contract is `host/nobro-host-contract.json`.

Important reports:

- `NOBRO_BOARD_PROFILE_REPORT`
- `NOBRO_BOARD_PACKAGE_REPORT`
- `NOBRO_MANIFEST_REPORT`
- `NOBRO_ADAPTER_COMPAT_REPORT`
- `NOBRO_AI_MODEL_REPORT`
- `NOBRO_ROS_BRIDGE_REPORT`
- `NOBRO_ADMISSION_REPORT`
- `NOBRO_RUNTIME_REPORT`
- `NOBRO_HEALTH_REPORT`
- `NOBRO_EVENT_LOG_REPORT`
- `NOBRO_MODULE_RUNTIME_REPORT`
- `NOBRO_DEGRADE_APPLICATION_REPORT`

Reports are designed to be copied from memory as `repr(C)` records and checked
with simple XOR checksums.

Host-side simulators mirror selected runtime contracts for early design and CI
checks:

```powershell
python tools/nobro_contract_tool.py sample-sensor --mode bad_data_every --ticks 4 --period 1
python tools/nobro_contract_tool.py check-ai-route
python tools/nobro_contract_tool.py sample-recovery --error sensor_read_fail --events 4
python tools/nobro_contract_tool.py sample-watchdog --timeout-us 100 --sweeps 3 --step-us 75
python tools/nobro_contract_tool.py sample-scheduler --ticks 1000 21020 41050 --tolerance-us 25
python tools/nobro_contract_tool.py sample-quota
python tools/nobro_contract_tool.py sample-degrade --flash-limit 73728 --ram-limit 16384
python tools/nobro_contract_tool.py sample-runtime-drill --fault-count 3
python tools/nobro_contract_tool.py check-runtime-drill --fault-count 3
python tools/nobro_contract_tool.py check-software-surface
python tools/nobro_contract_tool.py sample-startup
python tools/nobro_contract_tool.py sample-project platformio --name edge_demo --module control
python tools/nobro_contract_tool.py sample-project python_board_bridge --name edge_demo --module control
python tools/nobro_contract_tool.py write-project platformio --output _work\edge_demo --name edge_demo
python tools/nobro_contract_tool.py check-project _work\edge_demo --target platformio
python tools/nobro_contract_tool.py repair-project _work\edge_demo --target platformio
```

Generated starter projects include VS Code task metadata for the same project
check, so IDE users can run the validation gate from the command palette.
`repair-project` can rebuild that IDE metadata without overwriting project code
or `nobro-contract.json`.
Project checks validate the expected task commands, not just task labels, so
renamed or stale editor tasks are reported before they mislead a workflow.
Python host starter projects include both runtime drill and AI route gate tasks.
Python board bridge templates also include an offline bridge smoke task for
MicroPython, CircuitPython, and mPython-style status-line workflows.
The runtime drill output includes a recovery summary with retry, notification,
reboot, final-state, and self-healing flags for software-only review.
The runtime drill checker turns the same scenario into a pass/fail software
gate for CI and VS Code tasks. The software surface checker combines host
contract, package metadata, public headers, AI route, and runtime drill
validation before packaging.
The AI route checker validates route-policy behavior without contacting a
remote inference endpoint. It can model on-device, edge-sidecar, remote API,
and hybrid route decisions by changing backend, readiness, budget, stale-age,
and endpoint-failure arguments.
The startup sample prints the dependency order for a reference runtime module
set, which helps keep boot sequencing explicit before adding board-specific
adapter code.

## Cleanup

Keep generated data under `_work/`:

```powershell
Remove-Item -Recurse -Force _work\cargo-target -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force _work\artifacts -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force _work\logs -ErrorAction SilentlyContinue
```

`_work/`, target directories, firmware binaries, coverage output, and local
toolchains are ignored by Git.
