# User Guide

Day-to-day work with the NobroRTOS SDK: the mental model, the declarative
app generator, the prebuilt-firmware loop, authoring C modules against the
prebuilt runtime, and keeping a working tree clean.

## The repository, mentally

This manual helps a firmware developer understand the repository, run local
validation, and assemble a small NobroRTOS application.

### Mental Model

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

### Workspace Setup

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

### Build Profiles

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

### Creating An Application

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

An application can assemble the manifest, startup graph, admission plan, and runtime
through `BootAssembly`, then publish adapter compatibility before entering
hardware-facing code.

### Working With SAL

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

### AI And Robotics Bridges

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

For low-power mesh interop, the host contracts decode IEEE 802.15.4 MAC frames
and classify Thread-style 6LoWPAN payloads. A gateway adapter can retain the
base MAC record and add a bounded Thread rollup when a captured PSDU contains
IPHC or IPv6 6LoWPAN traffic.

AI model contracts and ROS bridge contracts can be exported as fixed reports, so
host tools can inspect model memory, timeout, stale-result policy, bridge entity
counts, total buffer demand, and transport choice without running a hardware
probe.

Camera adapters should keep scene/liveness analytics and person detection separate:
liveness proves the camera stream is real, while the optional person model turns
the latest frame into a bounded inference result. Multi-board manifests can pass
the same file with the `person_model` key on a `vision` node.

Audio adapters can stream little-endian PCM16 through a bounded marker protocol.
The keyword-spotting feature contract is fixed at
16 kHz mono, one second, 15 frames, and 8 log-energy bands, so a PDM or I2S
adapter only has to supply bounded PCM windows.

### Diagnostics

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
python tools/nobro_contract_tool.py check-ai-route-matrix
python tools/nobro_contract_tool.py check-ai-preflight-matrix
python tools/nobro_contract_tool.py check-ros-preflight-matrix
python tools/nobro_contract_tool.py check-bundle-matrix
python tools/nobro_contract_tool.py sample-recovery --error sensor_read_fail --events 4
python tools/nobro_contract_tool.py check-recovery-matrix
python tools/nobro_contract_tool.py sample-watchdog --timeout-us 100 --sweeps 3 --step-us 75
python tools/nobro_contract_tool.py check-watchdog-matrix
python tools/nobro_contract_tool.py sample-scheduler --ticks 1000 21020 41050 --tolerance-us 25
python tools/nobro_contract_tool.py check-scheduler-matrix
python tools/nobro_contract_tool.py sample-event-log --capacity 3 --events 4 --recent 3
python tools/nobro_contract_tool.py check-event-log-matrix
python tools/nobro_contract_tool.py sample-quota
python tools/nobro_contract_tool.py check-quota-matrix
python tools/nobro_contract_tool.py sample-degrade --flash-limit 73728 --ram-limit 16384
python tools/nobro_contract_tool.py check-degrade-matrix
python tools/nobro_contract_tool.py sample-runtime-drill --fault-count 3
python tools/nobro_contract_tool.py check-runtime-drill --fault-count 3
python tools/nobro_contract_tool.py check-software-surface
python tools/nobro_contract_tool.py check-python-surface
python tools/nobro_contract_tool.py check-cli-command-surface
python tools/nobro_contract_tool.py check-starter-templates
python tools/nobro_contract_tool.py sample-startup
python tools/nobro_contract_tool.py check-startup-matrix
python tools/nobro_contract_tool.py check-boot-summary-matrix
python tools/nobro_contract_tool.py sample-project platformio --name edge_demo --module control
python tools/nobro_contract_tool.py sample-project python_board_bridge --name edge_demo --module control
python tools/nobro_contract_tool.py write-project platformio --output _work\edge_demo --name edge_demo
python tools/nobro_contract_tool.py check-project _work\edge_demo --target platformio
python tools/nobro_contract_tool.py repair-project _work\edge_demo --target platformio
python tools/verify_timing_lease.py
python tools/tutorial_runner.py
```

Generated starter projects include VS Code task metadata for the same project
check, so IDE users can run the validation gate from the command palette.
The project check command returns non-zero when validation fails, which makes
the generated task suitable for CI and editor problem workflows.
`repair-project` can rebuild that IDE metadata without overwriting project code
or `nobro-contract.json`.
Project checks validate the expected task commands, not just task labels, so
renamed or stale editor tasks are reported before they mislead a workflow.
`tools/static_budget.py` can gate build-time memory and timing envelopes for an
ELF before it is packaged. It reports flash, static RAM, worst-case stack, and a
static instruction-cycle estimate, then returns non-zero when a configured RAM
or cycle budget is exceeded:

```powershell
python tools/static_budget.py _work\firmware\app.elf --ram-budget 32768
python tools/static_budget.py _work\firmware\app.elf --cycle-budget 200000 --clock-hz 64000000
```

The timing estimate is a build-time review gate, not a substitute for final
target timing validation. It flags loops, recursion, indirect calls, and
unknown mnemonics so deadline-sensitive modules can be inspected before they
reach board-specific tests.
The static web flasher lives under `packages/web-flasher` and supports local
firmware drop, Web Serial boot-entry commands, and WebUSB transfers for devices
that expose a compatible bulk endpoint.
The static block editor lives under `packages/block-editor` and emits the same
`app.json` schema consumed by `tools/nobro_app.py`, so visual workflows still
land in the normal contract validator and Rust skeleton generator.
`check-starter-templates` validates every starter target in a temporary
directory, which keeps Arduino, PlatformIO, standalone SDK, Python host, and
Python board bridge onboarding paths aligned.
Python host starter projects include runtime drill, AI route, AI route matrix,
AI preflight matrix, ROS preflight matrix, recovery matrix, watchdog matrix,
scheduler matrix, event log matrix, quota matrix, degrade matrix, startup matrix, boot summary matrix,
bundle matrix, and report matrix gate tasks.
Python board bridge templates also include an offline bridge smoke task for
MicroPython, CircuitPython, and mPython-style status-line workflows.
The runtime drill output includes a recovery summary with retry, notification,
reboot, final-state, and self-healing flags for software-only review.
The runtime drill checker turns the same scenario into a pass/fail software
gate for CI and VS Code tasks. The software surface checker combines host
contract, package metadata, public headers, Python public surface, CLI command
surface, starter templates, AI route matrix, AI preflight matrix, ROS preflight matrix, recovery
matrix, watchdog matrix, scheduler matrix, event log matrix, quota matrix,
degrade matrix, startup matrix, boot summary matrix,
bundle matrix, report matrix, and runtime drill validation before packaging.
The timing and lease verifier exhaustively checks a bounded lease state space
and scheduler-jitter model without external model-checker dependencies.
The tutorial runner validates the public NobroRTOS book, tutorial app contract,
generated skeleton path, and bounded verifier in one beginner-friendly gate.
Fleet OTA rollout logic lives in `nobro-net`: use `FleetOtaOrchestrator` to
stage canary-first update waves, enforce maximum parallel updates, block rollout
when fleet health is low, and roll failed nodes back before a transport-specific
OTA sender moves chunks through the mesh.
The Python public surface checker validates the top-level package exports with
AST parsing, so host tools can catch stale imports without executing package
initialization.
The CLI command surface checker validates command registration and README
coverage together, so onboarding instructions fail early when a command name
drifts.
The report matrix checker verifies fixed-report status classes, checksum
handling, error labels, and decoded runtime, AI model, and ROS bridge fields.
The AI route checker validates route-policy behavior without contacting a
remote inference endpoint. It can model on-device, edge-sidecar, remote API,
and hybrid route decisions by changing backend, readiness, budget, stale-age,
and endpoint-failure arguments. The AI route matrix checker validates local,
remote API, edge sidecar, stale snapshot, degraded fallback, and unavailable
paths in one gate.
The AI preflight matrix checker validates inference-call admission before a
model runs: input/output buffers, scratch and arena RAM, non-zero local arena
declarations, module capability declarations, route budget, stale snapshot
limits, degraded fallback, unavailable routes, and endpoint circuit state.
The ROS preflight matrix checker validates bridge-call admission before a ROS
transport or agent is contacted: topic payload bounds, service/action response
capacity, queue depth, parameter value size, and timeout budget.
The recovery matrix checker validates ignore, retry, notify, reboot, OK-reset,
fixed-plan execution, dependency-impact reboot planning, runtime state
bookkeeping, and output-buffer backpressure paths in one gate.
The watchdog matrix checker validates non-mutating liveness prechecks, expiry
mutation, heartbeat reset, multi-module expiry, and capacity errors.
The scheduler matrix checker validates on-time ticks, tolerated early/late
jitter, missed deadlines, 32-bit time wraparound, counter reset, and invalid
scheduler configuration.
The event log matrix checker validates fixed-ring capacity accounting,
overwrite pressure, recent-record order, severity thresholds, zero-capacity
drop accounting, and invalid input handling.
The quota matrix checker validates fixed-capacity registration, reserve,
release, total-use, strict module identity, limit enforcement, underflow, and
overflow paths.
The degrade matrix checker validates flash, RAM, pool, module-limit,
same-criticality, planner-capacity, and essential-module pressure paths.
The startup sample prints the dependency order for a reference runtime module
set, which helps keep boot sequencing explicit before adding board-specific
adapter code. The startup matrix checker validates no-dependency, chain,
fan-in/fan-out, dependency-impact, unknown-node, self-cycle, duplicate-edge,
and cycle paths for the same deterministic planner. Rust startup graphs can also report the
transitive impact of a faulted dependency in reverse startup order, which gives
recovery code a stable quiesce-before-restart list.
The boot summary matrix checker validates all-pass, missing-stage,
corrupt-checksum, failed-adapter, in-progress-stage, diagnostic-code, and
status-count paths for host-side boot report review.
The bundle matrix checker validates module naming, capability ownership,
AI/ROS descriptor uniqueness, hard-realtime deadlines, startup dependency
errors, and JSON roundtrip stability.

### Cleanup

Keep generated data under `_work/`:

```powershell
Remove-Item -Recurse -Force _work\cargo-target -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force _work\artifacts -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force _work\logs -ErrorAction SilentlyContinue
```

`_work/`, target directories, firmware binaries, coverage output, and local
toolchains are ignored by Git.

## Declarative apps (the app.json generator)

For the newer graph-oriented flow, use the SDK project command:

```bash
python sdk/cli/nobro.py project new rover
python sdk/cli/nobro.py project explain _work/projects/rover/workload.json
python sdk/cli/nobro.py project build _work/projects/rover
python sdk/cli/nobro.py project simulate _work/projects/rover
```

The concise `workload.json` task/channel graph regenerates the build input and is
expanded into the same low-level contract the kernel admits. `project run` combines
contract explanation, build, simulation, and report decoding.

`project explain` validates the same task/wire model before it prices the graph. The
first workload error is reported with a stable `NOBRO-E03x`/`NOBRO-E04x` code, for example an
unknown wire endpoint is `NOBRO-E041`, so CI logs and beginner-facing tutorials point
to the same fix.

Optional kernel capabilities are configured only in the top-level `features` object:

```json
{
  "target": "nrf52840-nosd",
  "features": {"capacity-report": true}
}
```

Validation, generated Cargo features, and `project explain` all read the versioned
SDK feature catalog. The explanation includes the feature's marginal and aggregate
flash/RAM reserve plus its latency class and evidence level. Catalog values are
conservative ceilings for the named target and composition—not universal binary-size
or physical-latency measurements. An unsupported or unpriced feature fails closed;
it is never treated as zero cost.

The canonical workload is also the direct native-firmware source:

```bash
python sdk/cli/nobro.py firmware _work/projects/rover/workload.json \
  --out _work/firmware --build
```

For `nrf52840-nosd`, the gated nano specimen measures 2584/16/24 B
flash/static/total RAM with the feature off and 2880/20/28 B with it on: deltas
of 296/4/4 B under catalog ceilings of 384/16/32 B. The target gate verifies
the feature marker is absent/off and present/on. This prices only the stated
linked composition and toolchain. `nrf52840-s140` remains unavailable until it
has its own linked A/B evidence.

If the desired output is production nRF firmware rather than a host scaffold, use one
short `app.nobro` declaration:

```bash
python sdk/cli/nobro.py firmware tutorials/rover-one-file/app.nobro --build
python sdk/cli/nobro.py project explain _work/projects/rover/workload.json
```

The Python authoring persona declares the same periodic tasks and wires, runs
deterministic callbacks under pytest, and exports strict JSON:

```python
from nobro_rtos import HZ, NobroApp

app = (NobroApp("rover", board="nrf52840-nosd")
       .task("motor", HZ(200), role="control")
       .task("imu", HZ(100), role="sensor")
       .wire("imu", "motor", 8))
app.run(50_000)
app.write_json("app.json")
```

```bash
python sdk/cli/nobro.py firmware app.json --build
```

The firmware command reads JSON; it does not execute the Python source. Task
callbacks are host-only test hooks and are omitted from JSON. Device code is
native Rust using the same admission generator as `app.nobro`. This path
currently supports the generator's nRF52840 SoftDevice and no-SoftDevice
profiles. Wire capacity is topology metadata until a native payload binding is
selected.

The generator emits `workload.json`, `generation.json`, and a compiling `no_std` Cargo
crate from the same input. The explicit board profile selects the S140 or no-SoftDevice
memory layout. Defaults reduce beginner boilerplate while keeping budget review visible.
If the exact board/composition has a measured compare-wake-to-dispatch upper bound,
place `wake 25us` directly after the `board` line. It is charged by admission; omit
the line rather than inventing a value when no bound has been measured.

`gen-app` turns a declarative module spec into a **buildable NobroRTOS firmware app**.
You describe the module (criticality + memory budget); the generator emits a workspace
crate whose `main.rs` assembles the manifest via `BootAssembly`, admits it, and exports
the host-readable `NOBRO_MANIFEST_REPORT` / `NOBRO_ADMISSION_REPORT`. The generated Rust
is compiler-checked, so your contract is preserved - you never hand-write Rust.

This is the declarative app-generation path: describe the app as data, generate the Rust.

### Generate

```powershell
python tools/nobro_contract_tool.py gen-app --name my_control_app --module control
# options: --criticality {best_effort|user|driver|system}  --flash <bytes>  --ram <bytes>  --pool <slots>
```

This writes `core/apps/control/my_control_app/` (`Cargo.toml`, `build.rs`, `src/main.rs`,
`nobro-contract.json`, `README.md`) and registers it as a workspace member.

### Build

```powershell
cd core
cargo build -p my-control-app --release
```

### Verify on hardware

Flash and read the reports (see [GETTING_STARTED.md](GETTING_STARTED.md)). A booted
app populates `NOBRO_MANIFEST_REPORT` (magic `NBMF`) and `NOBRO_ADMISSION_REPORT`
(magic `NBAD`); both carry the module count and a sealed checksum, so a host tool
confirms the manifest assembled and admission passed without a `defmt` decoder.

Verified end to end on the development board: a generated `driver`-criticality app compiles for
`thumbv7em`, boots, assembles a 2-module manifest (kernel + your module), and passes
admission.

### Editing the contract

Edit `nobro-contract.json` (the module's criticality / memory budget) and re-run
`gen-app --overwrite`, or edit `src/main.rs` directly. Either way the manifest is
re-validated by the compiler and at admission.

### Authoring module logic in C or C++

`gen-app` scaffolds a Rust app. To write module *logic* outside Rust, generate a
C or C++ module skeleton over the [C ABI](../bindings/c/include/nobro_app.h):

```powershell
python tools/nobro_contract_tool.py gen-module --name my_sensor --lang c   --out my_mod
python tools/nobro_contract_tool.py gen-module --name my_sensor --lang cpp --out my_mod
```

This writes an editable module (`nobro_app_init()` once, `nobro_app_poll()` each
cycle) and prints the build command, which compiles + links your file into the
`c_abi_demo` firmware via the `c-source` / `cpp-source` path (needs
`arm-none-eabi-gcc` / `g++`). Both languages are verified end to end on the development board - the
kernel admits the C or C++ module and it drives a sensor to a passing report. See
[bindings/c/README.md](../bindings/c/README.md) and
[bindings/cpp/README.md](../bindings/cpp/README.md).

## The prebuilt UF2 loop

**UX rung 0 target:** flash a prebuilt UF2 **once**, then iterate by editing *data*
(`app.json` from the block editor) — no toolchain, no rebuild, code-free after first
flash.

### The loop

```
1. Drag-drop prebuilt UF2  →  board enumerates (DFU/COM)
2. Open block editor       →  design app visually
3. Export app.json         →  drop on serial / future UF2 data partition
4. Web console / ReportReader  →  plain-English PASS/FAIL from NOBRO_* reports
```

NobroRTOS ships the bundle builder and manifest gate. Runtime `app.json` hot-swap is not
implemented; changing application behavior still requires rebuilding firmware.

| Piece | Status |
| --- | --- |
| Block editor → `app.json` | Implemented |
| Web-flasher report console | Implemented |
| `nobro_app.py` validator | Done |
| Bootloader-safe UF2 flash | Done (`hw_eval --flash uf2`) |
| Prebuilt diagnostic UF2 bundle | Implemented by `package_prebuilt_uf2.py --build` |
| `app.json` runtime reload | Not implemented; rebuild required |

### Prebuilt shell firmware

The shell UF2 is a known-good firmware image that:

1. Boots through the six-stage chain and emits decodable `NOBRO_*` reports.
2. Enumerates as USB CDC and exposes host-readable diagnostic reports.
3. Bundles validated starter `app.json` as source input for a later rebuild.

Build command:

```bash
python tools/package_prebuilt_uf2.py --build
```

### What the user sees

After the one-time UF2 flash:

- Block editor exports `app.json`.
- User validates the file and rebuilds firmware; live serial/mass-storage ingestion is not
  currently supported.
- Console shows: "✅ servo mounted, sensor alive" or the first-fault sentence.

This matches the CircuitPython "edit `code.py`, save, it runs" bar — except the
editable artifact is **contract data**, not Python source.

### Gate

`python tools/package_prebuilt_uf2.py --check` verifies:

- The committed bundle manifest matches the safe flash layout.
- Sample `app.json` from the block editor passes `nobro_app.validate()`.
- A locally built UF2, when present under `_work/prebuilt/`, has valid block structure,
  family ID, and address bounds.

## Tier C: C modules against libnobro.a

Tier C is for C developers who want NobroRTOS's control plane (admission, budgets,
leases, `NOBRO_*` reports) without touching the Rust toolchain. You get a prebuilt
`libnobro.a` containing the whole runtime — boot, vector table, kernel, host services —
and you supply one C file.

### 1. Get the bundle

No release bundle is currently published. A trusted builder with the Rust toolchain runs:

```bash
python tools/build_libnobro.py --build     # stages _work/tierc/
```

The bundle: `libnobro.a`, the linker scripts it expects (`link.x`, `memory.x`,
`defmt.x`), the C ABI headers (`nobro_app.h`, `nobro_rtos.h`), legacy and
declarative reference modules (`imu_module.c`, `declarative_app.c`), and one-line
build scripts.

### 2. Write your module

Declare each task as its name, rate, and step; declare graph wires; then run:

```c
#include "nobro_app.h"

static int32_t imu(void) { /* one bounded sensor transaction */ return 0; }
static int32_t control(void) { /* one bounded control update */ return 0; }

static int32_t configure(void) {
    int32_t result = nobro_task("imu", HZ(100), imu);
    if (result < 0) return result;
    result = nobro_task("control", HZ(50), control);
    if (result < 0) return result;
    result = nobro_wire("imu", "control", 8);
    if (result < 0) return result;
    return nobro_run();
}

NOBRO_APP(configure)
```

The fixed-capacity Tier-C registry admits the declaration through the shared Rust
`AppGraph` validator and periodically drives the callbacks. Use
`nobro_task_with()` plus `nobro_task_options_t` only when the periodic-driver
defaults need a control/service role or explicit timing override. A wire derives
the graph/mailbox relationship; it is not a payload send/receive function.
Hardware remains reachable only through bounded host services.

The shipped runtime owns callback state through `ForeignModuleRunner`: rejected
admission never reaches `nobro_app_init`, and a negative init or poll result
revokes all host-service authority before further callbacks are rejected. The
runtime records init/poll failures as structured module recovery faults before
entering its fail-closed platform idle path.
Every exported host service also passes through one dispatcher-owned
`ForeignHostContext`. The C module cannot provide an identity: the context binds
the admitted module, checks its capability, charges bounded call/byte usage, and
records the attempted and completed operation before returning a result. Quota
exhaustion is reported as a negative host-service result and therefore follows
the same fail-closed callback recovery path.

### 3. Link (this is the whole build)

```bash
./build.sh my_module.c        # or build.cmd my_module.c on Windows
# = arm-none-eabi-gcc <cpu flags> my_module.c \
#     -Wl,--whole-archive libnobro.a -Wl,--no-whole-archive \
#     -T link.x -T defmt.x -nostartfiles -lm -o firmware.elf
```

`--whole-archive` matters: the vector table lives in the archive and nothing
references it by symbol, so the linker must keep every member.

### 4. Flash + verify

The ELF targets the no-SoftDevice layout (app at `0x1000`). Flash it with a compatible
SWD tool (`docs/GETTING_STARTED.md`) or convert it to UF2 for drag-and-drop. The firmware
seals `NOBRO_IMU_HEALTH_REPORT`; consume it through a transport exposed by the selected
application. Machine-specific probe scripts and endpoint settings are not distributed.

### Current scope, honestly

- One prebuilt layout today: **nRF52840, no-SoftDevice**. The S140 variant is a
  rebuild flag away for whoever produces bundles.
- The link test (`python tools/build_libnobro.py --check`) runs in CI, so a bundle
  that stops linking against plain gcc fails the gate before it reaches you.

## Keeping the tree clean (operations)

This guide keeps the repository clean and repeatable during development.

### Local Work Root

Use `_work/` for all generated assets:

| Path | Purpose |
| ---- | ------- |
| `_work/cargo-target/` | `CARGO_TARGET_DIR` |
| `_work/artifacts/` | local firmware images |
| `_work/logs/` | run logs and captured output |
| `_work/downloads/` | temporary downloads |
| `_work/toolchain/` | optional portable tools |

`_work/` is ignored by Git. Do not commit generated firmware, build caches,
coverage data, or downloaded toolchains.

### Validation Commands

```powershell
cd core
$env:CARGO_TARGET_DIR = (Resolve-Path '..\_work').Path + '\cargo-target'
cargo fmt --all -- --check
cargo test -p nobro-kernel --target x86_64-pc-windows-msvc
cargo test -p nobro-sal --target x86_64-pc-windows-msvc
cargo test -p nobro-host --target x86_64-pc-windows-msvc
cargo check --workspace
cd ..
python tools/nobro_contract_tool.py doctor
python tools/nobro_contract_tool.py check-host-contract
python tools/nobro_contract_tool.py check-distribution-metadata
python tools/nobro_contract_tool.py check-public-headers
python tools/nobro_contract_tool.py check-software-surface
python tools/nobro_contract_tool.py check-starter-templates
python tools/nobro_contract_tool.py check-ai-route
python tools/nobro_contract_tool.py check-ai-route-matrix
python tools/nobro_contract_tool.py check-ai-preflight-matrix
python tools/nobro_contract_tool.py check-ros-preflight-matrix
python tools/nobro_contract_tool.py check-bundle-matrix
python tools/nobro_contract_tool.py check-report-matrix
python tools/nobro_contract_tool.py check-recovery-matrix
python tools/nobro_contract_tool.py check-watchdog-matrix
python tools/nobro_contract_tool.py check-scheduler-matrix
python tools/nobro_contract_tool.py check-event-log-matrix
python tools/nobro_contract_tool.py check-quota-matrix
python tools/nobro_contract_tool.py check-degrade-matrix
python tools/nobro_contract_tool.py check-startup-matrix
python tools/nobro_contract_tool.py check-boot-summary-matrix
python tools/nobro_contract_tool.py check-report-matrix
python tools/nobro_contract_tool.py check-runtime-drill
```

`check-report-matrix` should pass before packaging any host tooling change that
touches fixed reports, boot diagnostics, AI model descriptors, or ROS bridge
descriptors.
`check-ai-preflight-matrix` should pass before packaging AI-facing tooling or
starter templates. It catches oversized inference buffers, insufficient module
RAM, missing AI capabilities, stale snapshot policy violations, degraded
fallback, unavailable routes, and open endpoint circuits without contacting a
model or endpoint.
`check-ros-preflight-matrix` should pass before packaging ROS-style bridge
tooling or starter templates. It catches oversized bridge payloads, undersized
response buffers, zero-depth queues, parameter value overflow, and timeout
budget violations without contacting a ROS transport or agent.
Kernel recovery tests should pass before changing self-healing logic. They
cover fixed-capacity recovery planning, execution cursor progress, overdue-step
visibility, output-buffer backpressure, watchdog escalation, and module
lifecycle recovery completion without contacting external hardware.

### Commit Hygiene

- Keep documentation and comments in English.
- Keep local route notes out of Git.
- Keep generated files under ignored paths.
- Commit coherent architecture or feature slices.
- Do not create tags or releases until the project has a formal complete
  version.

### Python Environment

If Python tooling is needed, use any Python 3 environment (a venv is fine):

```powershell
python3 -m venv .venv && . .venv/bin/activate
```

Python tools should write outputs under `_work/`.
