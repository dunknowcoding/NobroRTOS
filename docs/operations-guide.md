# NobroRTOS Operations Guide

This guide keeps the repository clean and repeatable during development.

## Local Work Root

Use `_work/` for all generated assets:

| Path | Purpose |
| ---- | ------- |
| `_work/cargo-target/` | `CARGO_TARGET_DIR` |
| `_work/artifacts/` | local firmware images |
| `_work/logs/` | run logs and captured output |
| `_work/downloads/` | temporary downloads |
| `_work/toolchain/` | optional portable tools |

`_work/` is ignored by Git. Do not commit generated firmware, build caches,
coverage data, or downloaded toolchains.

## Validation Commands

```powershell
cd core
$env:CARGO_TARGET_DIR = (Resolve-Path '..\_work').Path + '\cargo-target'
cargo fmt --all -- --check
cargo test -p nobro-kernel --target x86_64-pc-windows-msvc
cargo test -p nobro-sal --target x86_64-pc-windows-msvc
cargo test -p nobro-host --target x86_64-pc-windows-msvc
cargo check --workspace
cd ..
python tools/nobro_contract_tool.py doctor
python tools/nobro_contract_tool.py check-host-contract
python tools/nobro_contract_tool.py check-distribution-metadata
python tools/nobro_contract_tool.py check-public-headers
python tools/nobro_contract_tool.py check-software-surface
python tools/nobro_contract_tool.py check-starter-templates
python tools/nobro_contract_tool.py check-ai-route
python tools/nobro_contract_tool.py check-ai-route-matrix
python tools/nobro_contract_tool.py check-recovery-matrix
python tools/nobro_contract_tool.py check-runtime-drill
```

## Commit Hygiene

- Keep documentation and comments in English.
- Keep local route notes out of Git.
- Keep generated files under ignored paths.
- Commit coherent architecture or feature slices.
- Do not create tags or releases until the project has a formal complete
  version.

## Python Environment

If Python tooling is needed, use the `IronEngineWorld` conda environment:

```powershell
conda activate IronEngineWorld
```

Python tools should write outputs under `_work/`.
