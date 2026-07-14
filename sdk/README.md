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
