# NobroRTOS SDK

Everything a user consumes, in one place — the command surface, the C headers, the
prebuilt firmware, and the language packages. The implementation lives in `core/`;
this folder is the *product*.

```
sdk/
├── cli/nobro.py        the SDK command (app · eval · flash · verify · fleet ·
│                       budget · sign · package · contract)
├── include/            the C ABI headers, drift-gated copies of the canonical
│                       bindings/c/include (regenerate: nobro package arduino --sync)
├── firmware/           prebuilt, committed firmware images
│   ├── nobrortos-starter-s140.uf2   drag-drop starter (S140 layout, app @0x26000)
│   └── starter-app.json             the declarative app it runs
├── python/             the pip-installable host package (install pointer)
└── sdk-manifest.json   machine-readable SDK contract (validated in CI)
```

## The one command

```bash
python sdk/cli/nobro.py verify            # every software gate -> Evidence Pack
python sdk/cli/nobro.py eval imu          # build+flash+run+grade on hardware
python sdk/cli/nobro.py app my-app.json   # validate a declarative app
python sdk/cli/nobro.py package arduino --zip
```

Every subcommand forwards to a stable tool under `tools/` and accepts that tool's
flags unchanged (`nobro eval --help` = the real help).

## What consumes what

| You are building… | Take |
| --- | --- |
| An Arduino sketch | the library zip (`nobro package arduino --zip`) — headers included |
| A C module, no Rust | the Tier C bundle (`nobro package tierc --build`) + `include/` |
| A Python host tool | `pip install ./bindings/python` (see [python/README.md](python/README.md)) |
| A first experience | drag `firmware/nobrortos-starter-s140.uf2` onto the board's UF2 drive — [tutorial 01](../tutorials/README.md) |
| Rust firmware | the workspace itself ([docs/GETTING_STARTED.md](../docs/GETTING_STARTED.md)) |

## Guarantees

- `include/` never drifts from the canonical headers — a CI gate fails on mismatch.
- The committed UF2 is bootloader-safe (app region only) and verified by the
  `prebuilt uf2 loop` gate, which re-parses the container on every run.
- The whole surface is contract-checked: `python tools/nobro_contract_tool.py
  check-software-surface` (part of `nobro verify`).
- Generated archives and build outputs go to `_work/`, never committed — the only
  committed binaries are the intentional `firmware/` images.
