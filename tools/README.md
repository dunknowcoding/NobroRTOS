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
python tools/nobro_contract_tool.py check-event-log-matrix
python tools/nobro_contract_tool.py check-ai-preflight-matrix
python tools/nobro_contract_tool.py check-ros-preflight-matrix
python tools/nobro_contract_tool.py check-quota-matrix
python tools/nobro_contract_tool.py check-degrade-matrix
python tools/nobro_contract_tool.py check-startup-matrix
python tools/nobro_contract_tool.py check-boot-summary-matrix
python tools/nobro_contract_tool.py check-bundle-matrix
python tools/nobro_contract_tool.py check-report-matrix
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
`check-ai-preflight-matrix` validates AI invocation admission before inference:
input/output buffers, scratch and arena RAM, route budget, capability
declarations, stale snapshots, and endpoint circuit policy.
`check-ros-preflight-matrix` validates ROS-style bridge admission before a
transport is contacted: topic payload bounds, service/action response capacity,
queue depth, parameter value size, and timeout budget.
`check-bundle-matrix` validates deterministic contract bundle roundtrip,
capability ownership, module naming, AI/ROS uniqueness, hard-realtime deadline,
and startup dependency error paths.
`check-recovery-matrix` validates deterministic ignore, retry, notify, reboot,
OK-reset, fixed-plan execution, and output-buffer backpressure paths.
`check-watchdog-matrix` validates deterministic liveness precheck, expiry,
heartbeat reset, multi-module expiry, and capacity-error paths.
`check-scheduler-matrix` validates deterministic on-time, tolerance,
deadline-miss, wraparound, reset, and invalid-configuration scheduler paths.
`check-event-log-matrix` validates deterministic fixed-ring capacity, overwrite,
recent-order, severity-threshold, zero-capacity, and invalid-input paths.
`check-quota-matrix` validates deterministic fixed-capacity registration,
reserve, release, total-use, identity, limit, underflow, and overflow paths.
`check-degrade-matrix` validates deterministic degraded-mode flash, RAM, pool,
module-limit, same-criticality, capacity, and essential-module paths.
`check-startup-matrix` validates deterministic no-dependency, dependency-chain,
fan-in/fan-out, dependency-impact, unknown-node, self-cycle, duplicate-edge,
and cycle paths.
`check-boot-summary-matrix` validates deterministic all-pass, missing-stage,
corrupt-checksum, failed-adapter, in-progress-stage, diagnostic-code, and
status-count paths for boot report summaries.
`check-report-matrix` validates fixed report pass, fail, missing, in-progress,
and corrupt states, checksum handling, error labels, and AI/ROS domain fields.
`check-runtime-drill` returns non-zero when disabled modules, module reboots, or
dropped event-log records exceed the configured limits.
`check-software-surface` is the pre-package gate for software-only validation:
it combines the host contract, SDK/package metadata, public headers, starter
templates, AI route matrix, AI preflight matrix, ROS preflight matrix,
recovery matrix, watchdog matrix, scheduler matrix, event log matrix, quota matrix, degrade matrix, startup matrix, boot summary
matrix, bundle matrix, and runtime drill checks into one JSON report.
`check-starter-templates` materializes every starter project in a temporary
directory, validates it, and removes the temporary files when the gate exits.
`check-project` and `repair-project` also return non-zero when a starter
project remains invalid, so generated VS Code tasks can fail clearly.
