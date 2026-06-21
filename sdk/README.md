# NobroRTOS Standalone SDK

This folder defines the standalone SDK distribution surface.

Current contents:

- `sdk-manifest.json` describes the canonical SDK contract, include roots,
  host tools, package surfaces, and generated-output policy.
- C and C++ headers are sourced from `bindings/c/include` and
  `bindings/cpp/include`.
- Host report decoding helpers are sourced from `bindings/python` and
  `tools/nobro_contract_tool.py`.
- Python contract builders, report decoders, and host simulators are packaged
  from `bindings/python`.

The core implementation remains in `core/`; this folder should contain only the
stable SDK packaging surface.

Generated archives, compiler outputs, and local caches must be written under a
throwaway work directory such as `_work/` and must not be committed.

Before packaging the SDK surface, run the software surface gate from the
repository root:

```powershell
python tools/nobro_contract_tool.py check-software-surface
```

The gate returns one JSON report for the host contract, package metadata,
public headers, starter templates, AI route matrix, recovery matrix, and
runtime drill checks.
