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
- MicroPython, CircuitPython, and mPython-inspired bridge workflows

## Contract Builders

`nobro_rtos.contracts` provides small typed builders for:

- module specs
- memory budgets
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
            owns=(Capability.AI_ENDPOINT,),
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
robotics bridge metadata.

`AiRoutePolicy` mirrors the Rust/C route decision contract for host simulation
and VS Code workflows. It can choose local inference, a remote endpoint, stale
snapshot reuse, degraded fallback, or an unavailable state from the same budget
and circuit-breaker inputs used by firmware.

ROS-style bridge descriptors keep readable names and also emit stable FNV-1a
32-bit hashes (`name_hash`, `message_type_hash`, `bridge_id_hash`, and
`transport_hash`). Rust-side `RosBridgeContract` code can use those hash fields
without carrying dynamic strings in realtime paths.

## Simulation Helpers

`SensorStubSimulator` mirrors the Rust `sensor-stub` fixture modes for host
workflows. `ServoSimulator` mirrors the RoboServo-style actuator timing and
readback checks. `WatchdogSimulator` mirrors the kernel heartbeat tracker.
`RecoveryPolicySimulator` mirrors the kernel's health threshold escalation for
host-side self-healing drills.

```python
from nobro_rtos import (
    RecoveryPolicySimulator,
    SensorStubSimulator,
    ServoSimulator,
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
assert watchdog.expired(150)[0].module == "sensor"
```

The simulator is deterministic and uses only caller-visible records, making it
suitable for VS Code tasks, notebook experiments, and CI checks that should not
touch a board.

## CLI

Run the module from this folder or after installing the package:

```powershell
python -m nobro_rtos sample-ai-ros
python -m nobro_rtos sample-ai-route
python -m nobro_rtos sample-report ai_model
python -m nobro_rtos sample-report ros_bridge
python -m nobro_rtos sample-sensor --mode bad_data_every --ticks 4 --period 1
python -m nobro_rtos sample-actuator --start-us 1200 --stop-us 1800 --step-us 300
python -m nobro_rtos sample-recovery --error sensor_read_fail --events 4
python -m nobro_rtos sample-watchdog --timeout-us 100 --sweeps 3 --step-us 75
```

The command prints a sample JSON bundle with one AI module, one model contract,
and one ROS-style serial bridge. The route sample prints a matching AI route
policy, runtime state, and decision. The report samples print sealed fixed
reports that can be fed directly into `decode-report`. The sensor sample emits
deterministic fixture records and injected-fault summaries. The actuator sample
emits deterministic servo command records with deadline and readback summaries.
The recovery sample emits a deterministic health-counter timeline for notify
and reboot escalation drills. The watchdog sample emits heartbeat and expiry
events for liveness planning.

Validate the repository host contract against the Python enums:

```powershell
python -m nobro_rtos check-host-contract
```

From the repository root, use the local tool wrapper:

```powershell
python tools/nobro_contract_tool.py check-host-contract
python tools/nobro_contract_tool.py check-distribution-metadata
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
python tools/nobro_contract_tool.py decode-report ai_model path\to\ai_model_report.json
python tools/nobro_contract_tool.py decode-report ros_bridge path\to\ros_bridge_report.json
```

AI and ROS report decoding includes host-contract labels for backend,
route-preference, and transport fields while preserving the raw numeric record.

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
