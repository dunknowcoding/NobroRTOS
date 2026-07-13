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
python sdk/cli/nobro.py flash --help
python sdk/cli/nobro.py package arduino --zip
```

`project` creates a graph-declared application, explains its derived admission
contract and marginal costs, builds the generated host scaffold, simulates it, and
decodes its report. Flashing is explicit because a host scaffold is not a device image.

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
