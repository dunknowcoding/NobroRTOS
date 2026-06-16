# AIRON `_work/` — transient root

All downloads, toolchains, build outputs, and logs for AIRON live here. **Not committed.**

| Path | Purpose |
|------|---------|
| `toolchain/` | rustup/cargo home, optional portable tools |
| `cargo-target/` | `CARGO_TARGET_DIR` for Rust builds |
| `downloads/` | Generic fetch cache |
| `ncs/` | nCS/Zephyr sidecar builds (Phase 2+) |
| `logs/` | Test logs |
| `artifacts/` | Last-known-good `.hex` (gitignored) |

## Environment (PowerShell)

```powershell
$env:AIRON_WORK_ROOT = "F:\Arduino\driver\AIRON\_work"
$env:CARGO_TARGET_DIR = "$env:AIRON_WORK_ROOT\cargo-target"
$env:RUSTUP_HOME = "$env:AIRON_WORK_ROOT\toolchain\rustup"
$env:CARGO_HOME = "$env:AIRON_WORK_ROOT\toolchain\cargo"
$env:PATH = "$env:CARGO_HOME\bin;$env:PATH"
conda activate IronEngineWorld
```

## Cleanup

```powershell
.\clean.ps1        # trim deps/incremental
.\clean.ps1 -Deep  # full cargo-target wipe
```
