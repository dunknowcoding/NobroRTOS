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

## CLI

Run the module from this folder or after installing the package:

```powershell
python -m nobro_rtos sample-ai-ros
```

The command prints a sample JSON bundle with one AI module, one model contract,
and one ROS-style serial bridge.

Validate the repository host contract against the Python enums:

```powershell
python -m nobro_rtos check-host-contract
```

From the repository root, use the local tool wrapper:

```powershell
python tools/nobro_contract_tool.py check-host-contract
```

## Tests

The current tests use only the Python standard library:

```powershell
$env:PYTHONDONTWRITEBYTECODE = "1"
python -m unittest discover -s tests
```
