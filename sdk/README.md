# NobroRTOS SDK

The SDK collects the user-facing command, C headers, prepared firmware, and package
metadata. The implementation lives in `core/`.

```text
sdk/
|-- cli/nobro.py       project, app, flash, budget, sign, package, and contract commands
|-- include/           drift-gated copies of the canonical C headers
|-- firmware/          prepared firmware images and their app metadata
|-- python/            installation pointer for the Python host package
`-- sdk-manifest.json  machine-readable distribution contract
```

## Common commands

```bash
python sdk/cli/nobro.py app my-app.json
python sdk/cli/nobro.py project new rover
python sdk/cli/nobro.py project run _work/projects/rover
python sdk/cli/nobro.py project shrink occupancy.json --json capacities.json
python sdk/cli/nobro.py flash --help
python sdk/cli/nobro.py package arduino --zip
```

`project` creates a graph-declared application, explains its derived admission
contract and marginal costs, builds the generated host scaffold, simulates it, decodes
its report, and emits identity-bound capacity proposals without rewriting source.
Flashing is explicit because a host scaffold is not a device image.

One-file device projects run deadline, jitter, execution, blocking, utilization,
response-time, memory, pool, capability, and quota admission in the generated build
script. A rejected build names the task and a `NOBRO-E0xx` reason. Successful builds
place only the admitted priority/release/binding table in `.rodata`; the L0 target does
not link the admission engine, recovery, reporting, tracing, quota, health, stack-guard,
MPU, async, classic-compat, or formatting subsystems. The public gate verifies both
symbol absence and the 3,000 B minimal / 3,400 B complex L0 flash ceilings.
The Rust presets are `L0NanoKernel`, `L1GuardedKernel`, `L2ManagedKernel`, and
`L3AssuredKernel`; generated one-file firmware selects L0, while dynamic/Tier-C
assembly retains the L3 runtime admission and `seal` path. An existing L0
dispatcher can opt into L1 with `nano.with_stack_guards(guards)` without
re-running admission, resetting its epoch, or losing pending releases.
Capability checks and mailbox/alarm/KV limits are independently selectable with
`let mut governance = nano.governance();`; they use the bindings already admitted
by `.capabilities(...)` and `.object_quotas(...)` and do not pull in the managed
runtime or require stack guards.
Health escalation and lifecycle recovery are another independent choice:
`let mut recovery = nano.recovery(FaultThresholds::DEFAULT, now_us)?;`.
Tasks use their Nano indices, and retained tracing or the managed runtime are not required.
Long-lived MCU services can use `recovery_into` with caller-owned storage to avoid
a capacity-sized construction frame.
Add a retained event ring only when needed with `recovery_with_trace::<N>` (or
`recovery_with_trace_into`). For dependency-aware restart order, declare task-index
edges once with `recovery_dependencies().depends_on(task, dependency)` and call
`record_error_with_dependencies`; no runtime manifest or module IDs are required.

### Right-size from a device run

Use one campaign file for the declared stacks, kernel mailbox, sample pool, and
required main/ISR paths:

```bash
# 1. Bind this exact workload, build manifest, coverage, and declarations.
python sdk/cli/nobro.py project shrink --bindings --campaign campaign.json \
  --workload workload.json --build-manifest build.json --json bindings.json

# 2. Enable `nobro-kernel/capacity-report`, use bindings.json to construct
#    CapacityIdentity/CapacityRegistry/CapacityCampaignConfig, run every declared
#    path, quiesce the app, and capture CapacityReport::as_bytes() as capacity-report.bin.

# 3. Verify and decode the device bytes into the strict occupancy schema.
python sdk/cli/nobro.py project shrink --device-report capacity-report.bin \
  --campaign campaign.json --workload workload.json --build-manifest build.json \
  --json occupancy.json

# 4. Emit a review-only proposal; application source is never rewritten.
python sdk/cli/nobro.py project shrink occupancy.json --json capacities.json
```

The device decoder rejects corrupt, stale, incomplete, mismatched, or
post-finish evidence. Saturation, drops, a reached capacity, or incomplete path
coverage can be decoded for diagnosis, but the proposal step fails closed and
emits no declarations.

## Package selection

| You are building | Use |
| --- | --- |
| Arduino sketch | the Arduino library zip (`nobro package arduino --zip`) |
| C module without Rust sources | the Tier C bundle plus `include/` |
| Python host application | `pip install ./bindings/python` |
| First device application | `firmware/nobrortos-starter-s140.uf2` and tutorial 01 |
| Rust firmware | the workspace and [getting-started guide](../docs/GETTING_STARTED.md) |

Generated archives, compiler output, caches, and logs belong under ignored `_work/`.
The committed firmware directory contains only intentional SDK images.
