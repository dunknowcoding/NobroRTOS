# NobroRTOS Tools

This folder contains repository-native tools.

Tooling direction:

- SDK package builders
- Arduino and PlatformIO package generators
- host contract validators
- board fixture generators
- report decoding utilities

Generated outputs and caches must stay outside the repository.

## Contract Tool

`nobro_contract_tool.py` runs the Python contract tooling from the repository
root without requiring package installation:

```powershell
python tools/nobro_contract_tool.py doctor
python tools/nobro_contract_tool.py check-host-contract
python tools/nobro_contract_tool.py check-distribution-metadata
python tools/nobro_contract_tool.py check-public-headers
python tools/nobro_contract_tool.py check-software-surface
python tools/nobro_contract_tool.py check-starter-templates
python tools/nobro_contract_tool.py sample-ai-ros
python tools/nobro_contract_tool.py sample-ai-route
python tools/nobro_contract_tool.py check-ai-route
python tools/nobro_contract_tool.py check-ai-route-matrix
python tools/nobro_contract_tool.py check-recovery-matrix
python tools/nobro_contract_tool.py check-runtime-drill
python tools/nobro_contract_tool.py sample-startup
python tools/nobro_contract_tool.py sample-report runtime
python tools/nobro_contract_tool.py sample-report health
```

## Host Simulation Commands

These commands exercise deterministic software contracts without requiring a
board connection:

```powershell
python tools/nobro_contract_tool.py sample-sensor --mode bad_data_every --ticks 4 --period 1
python tools/nobro_contract_tool.py sample-actuator --start-us 1200 --stop-us 1800 --step-us 300
python tools/nobro_contract_tool.py sample-recovery --error sensor_read_fail --events 4
python tools/nobro_contract_tool.py sample-watchdog --timeout-us 100 --sweeps 3 --step-us 75
python tools/nobro_contract_tool.py sample-scheduler --ticks 1000 21020 41050 --tolerance-us 25
python tools/nobro_contract_tool.py sample-event-log --capacity 3 --events 4 --recent 3
python tools/nobro_contract_tool.py check-runtime-drill --fault-count 3
python tools/nobro_contract_tool.py sample-startup
```

`sample-runtime-drill` includes a recovery summary with retry, notification,
reboot, final-state, and self-healing flags for software-only review.
`check-ai-route` returns non-zero when a configured AI route decision violates
target, stale snapshot, degraded fallback, unavailable, or endpoint-circuit
limits. Use backend, preference, budget, readiness, stale-age, and
endpoint-failure arguments to model local, edge, remote API, and hybrid paths.
`check-ai-route-matrix` validates a deterministic set of local, remote API,
edge sidecar, stale snapshot, degraded fallback, and unavailable route paths.
`check-recovery-matrix` validates deterministic ignore, retry, notify, reboot,
and OK-reset recovery paths.
`check-runtime-drill` returns non-zero when disabled modules, module reboots, or
dropped event-log records exceed the configured limits.
`check-software-surface` is the pre-package gate for software-only validation:
it combines the host contract, SDK/package metadata, public headers, starter
templates, AI route matrix, recovery matrix, and runtime drill checks into one
JSON report.
`check-starter-templates` materializes every starter project in a temporary
directory, validates it, and removes the temporary files when the gate exits.
`check-project` and `repair-project` also return non-zero when a starter
project remains invalid, so generated VS Code tasks can fail clearly.
