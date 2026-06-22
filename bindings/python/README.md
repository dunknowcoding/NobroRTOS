# NobroRTOS Python Tooling

This folder contains Python-facing host tooling and contract builders. The
Python layer is for development workflows, not hard-realtime firmware paths.

Initial priorities:

- report decoding
- board/package fixture inspection
- manifest and adapter compatibility checks
- simulation harnesses for sensor and actuator fixtures
- AI/control orchestration hooks outside firmware hot paths
- VS Code task integration
- project template generation
- MicroPython, CircuitPython, and mPython-inspired bridge workflows

## Contract Builders

`nobro_rtos.contracts` provides small typed builders for:

- module specs
- memory budgets
- startup dependencies and startup plans
- AI model contracts
- ROS-style bridge descriptors

Example:

```python
from nobro_rtos import (
    AiBackendKind,
    AiModelContract,
    Capability,
    Criticality,
    MemoryBudget,
    ModuleSpec,
    NobroContractBundle,
)

bundle = NobroContractBundle(
    modules=(
        ModuleSpec(
            "ai",
            Criticality.USER,
            MemoryBudget(16 * 1024, 6 * 1024, 1),
            requires=(Capability.AI_INFERENCE, Capability.AI_ENDPOINT),
            owns=(Capability.AI_INFERENCE, Capability.AI_ENDPOINT),
        ),
    ),
    ai_models=(
        AiModelContract(
            model_id=42,
            backend=AiBackendKind.ON_DEVICE,
            input_bytes_max=128,
            output_bytes_max=32,
            arena_bytes=4096,
            timeout_us=20_000,
            stale_after_us=100_000,
        ),
    ),
)

print(bundle.to_json())
```

These builders keep Python-first users on the same contracts as the Rust core:
fixed budgets, explicit capabilities, bounded AI inference, and bounded
robotics bridge metadata. Bundle validation also checks capability ownership:
kernel-owned capabilities are treated as implicit providers, while non-kernel
capabilities must have exactly one owning module before another module can
require them.
`StartupDependency` and `plan_startup` mirror the Rust startup graph for
host-side ordering checks. They keep dependencies explicit, reject unknown
modules and cycles, and produce a deterministic order suitable for generated
project checks and CI.

`AiRoutePolicy` mirrors the Rust/C route decision contract for host simulation
and VS Code workflows. It can choose local inference, a remote endpoint, stale
snapshot reuse, degraded fallback, or an unavailable state from the same budget
and circuit-breaker inputs used by firmware. A zero policy stale window inherits
the model contract's `stale_after_us`; otherwise the stricter window is used.

ROS-style bridge descriptors keep readable names and also emit stable FNV-1a
32-bit hashes (`name_hash`, `message_type_hash`, `bridge_id_hash`, and
`transport_hash`). Rust-side `RosBridgeContract` code can use those hash fields
without carrying dynamic strings in realtime paths.

## Project Templates

`build_project_template` creates starter project manifests in memory for
standalone SDK, Arduino, PlatformIO, Python host, and Python board bridge
workflows. The
`sample-project` CLI prints the file list and contents as JSON, so callers can
inspect or filter the template first. The `write-project` CLI materializes the
same template with path-escape checks and no overwrite unless `--overwrite` is
set. The `check-project` CLI validates a generated project directory by
detecting its target shape, loading `nobro-contract.json`, and checking the
generated VS Code task metadata for the expected target. It returns a non-zero
process status when validation fails, so generated tasks and CI jobs can use it
as a real gate.
The `repair-project` CLI conservatively rebuilds `.vscode/tasks.json` when that
metadata is missing or stale; it does not rewrite user code or contracts.
The `check-starter-templates` CLI materializes every starter target in a
temporary directory, validates it, and removes the generated files when the
gate exits.
Generated templates include `.vscode/tasks.json` with a project check task; the
Python host template also includes runtime drill, runtime drill gate, and AI
route, AI route matrix, recovery matrix, watchdog matrix, scheduler matrix,
event log matrix, quota matrix, degrade matrix, and startup matrix gate tasks,
and the Python board bridge template includes an offline bridge smoke task for
MicroPython, CircuitPython, and mPython-style status-line workflows.
Project validation checks both task labels and the expected task command
arguments, so stale editor metadata cannot silently point to the wrong host
tool.

```python
from nobro_rtos import ProjectTarget, build_project_template

template = build_project_template(
    name="edge_demo",
    target=ProjectTarget.PLATFORMIO,
    module_name="control",
)
assert "nobro-contract.json" in template.file_map()
```

## Simulation Helpers

`SensorStubSimulator` mirrors the Rust `sensor-stub` fixture modes for host
workflows. `ServoSimulator` mirrors the RoboServo-style actuator timing and
readback checks. `WatchdogSimulator` mirrors the kernel heartbeat tracker.
`SchedulerSimulator` mirrors the kernel deadline tick counters. The
`check-scheduler-matrix` CLI validates on-time ticks, early/late jitter within
tolerance, missed deadlines, 32-bit time wraparound, counter reset, and invalid
scheduler configuration.
`EventLogSimulator` mirrors the fixed-ring event log. The
`check-event-log-matrix` CLI validates capacity accounting, overwrite pressure,
recent-record order, severity thresholds, zero-capacity behavior, and invalid
input handling.
`QuotaLedgerSimulator` mirrors fixed-capacity quota accounting. The
`check-quota-matrix` CLI validates registration capacity, reserve/release
totals, strict module identity, limit enforcement, release underflow,
configuration errors, and arithmetic overflow.
`DegradePlannerSimulator` mirrors degraded-mode module fitting. The
`check-degrade-matrix` CLI validates flash, RAM, pool, module-limit,
same-criticality, capacity, and essential-module pressure paths.
The `check-startup-matrix` CLI validates no-dependency, dependency-chain,
fan-in/fan-out, unknown-node, self-cycle, duplicate-edge, and cycle paths for
the deterministic startup planner.
`RuntimeDrillSimulator` composes planning, quota, event-log, and recovery
checks into one deterministic pressure drill.
`RecoveryPolicySimulator` mirrors the kernel's health threshold escalation for
host-side self-healing drills.
`RecoverySummary` turns drill recovery decisions into stable action counts,
final state, and reboot requirement flags for CI gates and editor views.

```python
from nobro_rtos import (
    EventLogSimulator,
    DegradePlannerSimulator,
    QuotaLedgerSimulator,
    RecoveryPolicySimulator,
    ResourceBudget,
    RuntimeDrillSimulator,
    SchedulerSimulator,
    SensorStubSimulator,
    ServoSimulator,
    SystemProfile,
    WatchdogSimulator,
)

sim = SensorStubSimulator.bad_data_every(2, sample_period_ticks=1)
first = sim.poll()
second = sim.poll()

assert first.plausible
assert not second.plausible

servo = ServoSimulator(readback_offset_us=10)
command = servo.set_duty_us(0, 1500, deadline_us=100, issued_at_us=90)
assert command.accepted

recovery = RecoveryPolicySimulator(notify_after=2, reboot_after=3)
assert recovery.record_error("sensor", "sensor_read_fail", 10).action.value == "ignore"

watchdog = WatchdogSimulator(capacity=1)
watchdog.register("sensor", timeout_us=100, now_us=0)
assert watchdog.expired_count(150) == 1
assert watchdog.expired(150)[0].module == "sensor"

scheduler = SchedulerSimulator(jitter_tolerance_us=25)
for tick in (1000, 21020, 41050):
    scheduler.on_deadline_tick(tick)
assert scheduler.stats().deadline_misses == 1

events = EventLogSimulator(capacity=3)
for index in range(4):
    events.push(index * 10, "kernel", "warn", "host", "counter", index)
assert events.dropped == 1

quota = QuotaLedgerSimulator(capacity=1)
quota.register("sensor", ResourceBudget(1024, 256, 1))
quota.reserve("sensor", ResourceBudget(512, 128, 1))
assert quota.available("sensor").flash_bytes == 512

decision = DegradePlannerSimulator.fit(
    modules=(),
    profile=SystemProfile(64 * 1024, 16 * 1024, 8, 4),
)
assert decision.disabled_count == 0

drill = RuntimeDrillSimulator(
    modules=(),
    profile=SystemProfile(64 * 1024, 16 * 1024, 8, 4),
)
assert drill.run().decision.disabled_count == 0
```

The simulator is deterministic and uses only caller-visible records, making it
suitable for VS Code tasks, notebook experiments, and CI checks that should not
touch a board.

## CLI

Run the module from this folder or after installing the package:

```powershell
python -m nobro_rtos doctor
python -m nobro_rtos sample-ai-ros
python -m nobro_rtos sample-ai-route
python -m nobro_rtos check-ai-route --backend hybrid --require-target on_device
python -m nobro_rtos check-ai-route-matrix
python -m nobro_rtos sample-report runtime
python -m nobro_rtos sample-report health
python -m nobro_rtos sample-report ai_model
python -m nobro_rtos sample-report ros_bridge
python -m nobro_rtos sample-sensor --mode bad_data_every --ticks 4 --period 1
python -m nobro_rtos sample-actuator --start-us 1200 --stop-us 1800 --step-us 300
python -m nobro_rtos sample-recovery --error sensor_read_fail --events 4
python -m nobro_rtos check-recovery-matrix
python -m nobro_rtos sample-watchdog --timeout-us 100 --sweeps 3 --step-us 75
python -m nobro_rtos check-watchdog-matrix
python -m nobro_rtos sample-scheduler --ticks 1000 21020 41050 --tolerance-us 25
python -m nobro_rtos check-scheduler-matrix
python -m nobro_rtos sample-event-log --capacity 3 --events 4 --recent 3
python -m nobro_rtos check-event-log-matrix
python -m nobro_rtos sample-quota
python -m nobro_rtos check-quota-matrix
python -m nobro_rtos sample-degrade --flash-limit 73728 --ram-limit 16384
python -m nobro_rtos check-degrade-matrix
python -m nobro_rtos sample-runtime-drill --fault-count 3
python -m nobro_rtos check-runtime-drill --fault-count 3
python -m nobro_rtos check-software-surface
python -m nobro_rtos check-starter-templates
python -m nobro_rtos sample-startup
python -m nobro_rtos check-startup-matrix
python -m nobro_rtos sample-project platformio --name edge_demo --module control
python -m nobro_rtos write-project platformio --output _work\edge_demo --name edge_demo
python -m nobro_rtos check-project _work\edge_demo --target platformio
python -m nobro_rtos repair-project _work\edge_demo --target platformio
```

The command prints a sample JSON bundle with one AI module, one model contract,
and one ROS-style serial bridge. The route sample prints a matching AI route
policy, runtime state, and decision. The route checker turns that decision into
a pass/fail gate for target selection, stale snapshots, degraded fallback,
unavailable routes, and open endpoint circuits. It can simulate on-device,
remote API, edge-sidecar, and hybrid contracts by changing backend, preference,
budget, readiness, stale-age, and endpoint-failure arguments. The route matrix
checker validates local, remote API, edge sidecar, stale snapshot, degraded
fallback, and unavailable scenarios in one gate. The report samples
print sealed fixed reports that can be fed directly into `decode-report`. The
sensor sample emits deterministic fixture records and injected-fault summaries.
The actuator sample emits deterministic servo command records with deadline and
readback summaries.
The recovery sample emits a deterministic health-counter timeline for notify
and reboot escalation drills. The recovery matrix checker validates ignore,
retry, notify, reboot, and OK-reset recovery paths in one gate. The watchdog
sample emits heartbeat and expiry events for liveness planning. The watchdog
matrix checker validates non-mutating prechecks, expiry mutation, heartbeat
reset, multi-module expiry, and capacity errors. The scheduler sample emits
deadline jitter counters for timing-policy checks. The scheduler matrix checker
validates on-time, tolerance, deadline-miss, wraparound, reset, and invalid
configuration paths. The event-log sample emits
fixed-ring pressure, dropped-event, and recent-record summaries. The event log
matrix checker validates capacity, overwrite, recent-order, severity-threshold,
zero-capacity, and invalid-input paths. The quota sample emits
fixed-capacity resource reservations, releases, remaining budget, and total
usage. The quota matrix checker validates registration capacity, reserve,
release, total-use, identity, limit, underflow, and overflow paths. The
degraded-mode sample emits a pressure reason plus the enabled and disabled
module sets selected by the same criticality-first policy used by the kernel
planner. The degrade matrix checker validates flash, RAM, pool, module-limit,
same-criticality, capacity, and essential-module pressure paths. The runtime
drill sample combines degraded-mode planning, quota usage, fixed-ring event
logging, and recovery escalation in a single host-side
JSON scenario, including a recovery summary with action counts, final state,
and reboot requirement flags. The project sample emits a deterministic
contract-first starter template as JSON for standalone SDK, Arduino,
PlatformIO, Python host, or Python board bridge workflows. The project writer
creates the same starter files under the selected output directory and refuses
to replace existing files unless `--overwrite` is passed. The project checker reports
target detection, module count, discovered files, and validation errors as JSON.
It returns non-zero when the project is invalid.
The starter-template checker verifies every generated starter target in a
temporary directory before packaging or publishing template changes.
VS Code users can run the generated `NobroRTOS: Check Project` task from the
starter project.
The runtime drill checker applies pass/fail limits to disabled modules, reboot
actions, and dropped event-log records, then returns a non-zero process status
when a limit is exceeded.
The software surface checker is the recommended pre-package gate for host-side
validation. It combines the host contract, SDK/package metadata, public C/C++
headers, starter templates, AI route matrix, recovery matrix, watchdog matrix,
scheduler matrix, event log matrix, quota matrix, degrade matrix, startup
matrix, and runtime drill gates into one JSON report.
The startup sample emits a deterministic dependency order for the runtime
module set, making startup sequencing reviewable before firmware code is
assembled. The startup matrix checker validates no-dependency, chain,
fan-in/fan-out, unknown-node, self-cycle, duplicate-edge, and cycle paths.

Validate the repository host contract against the Python enums:

```powershell
python -m nobro_rtos check-host-contract
```

From the repository root, use the local tool wrapper:

```powershell
python tools/nobro_contract_tool.py doctor
python tools/nobro_contract_tool.py check-host-contract
python tools/nobro_contract_tool.py check-distribution-metadata
python tools/nobro_contract_tool.py check-public-headers
python tools/nobro_contract_tool.py check-software-surface
python tools/nobro_contract_tool.py check-starter-templates
python tools/nobro_contract_tool.py check-ai-route
python tools/nobro_contract_tool.py check-ai-route-matrix
python tools/nobro_contract_tool.py check-recovery-matrix
python tools/nobro_contract_tool.py check-watchdog-matrix
python tools/nobro_contract_tool.py check-scheduler-matrix
python tools/nobro_contract_tool.py check-event-log-matrix
python tools/nobro_contract_tool.py check-quota-matrix
python tools/nobro_contract_tool.py check-degrade-matrix
python tools/nobro_contract_tool.py check-startup-matrix
```

Decode a boot diagnostic code:

```powershell
python tools/nobro_contract_tool.py decode-boot 0x04040003
```

Validate a contract bundle JSON file:

```powershell
python tools/nobro_contract_tool.py validate-bundle path\to\bundle.json
```

Decode a report JSON file:

```powershell
python tools/nobro_contract_tool.py decode-report manifest path\to\manifest_report.json
python tools/nobro_contract_tool.py decode-report adapter_compatibility path\to\adapter_report.json
python tools/nobro_contract_tool.py decode-report board_package path\to\board_package_report.json
python tools/nobro_contract_tool.py decode-report admission path\to\admission_report.json
python tools/nobro_contract_tool.py decode-report runtime path\to\runtime_report.json
python tools/nobro_contract_tool.py decode-report health path\to\health_report.json
python tools/nobro_contract_tool.py decode-report event_log path\to\event_log_report.json
python tools/nobro_contract_tool.py decode-report module_runtime path\to\module_runtime_report.json
python tools/nobro_contract_tool.py decode-report degrade_application path\to\degrade_report.json
python tools/nobro_contract_tool.py decode-report ai_model path\to\ai_model_report.json
python tools/nobro_contract_tool.py decode-report ros_bridge path\to\ros_bridge_report.json
```

AI and ROS report decoding includes host-contract labels for backend,
route-preference, and transport fields while preserving the raw numeric record.
Runtime diagnostics decode fixed timestamps, module labels, health counters,
event-log payload fields, and degraded-mode summary counters.

Summarize a boot report bundle:

```powershell
python tools/nobro_contract_tool.py summarize-boot path\to\boot_reports.json
```

The boot summary output mirrors the host contract: it includes
`diagnostic_code`, `diagnostic`, `pass_count`, `missing_count`,
`in_progress_count`, `fail_count`, `corrupt_count`, and the full slot list.

## Tests

The current tests use only the Python standard library:

```powershell
$env:PYTHONDONTWRITEBYTECODE = "1"
python -m unittest discover -s tests
```
