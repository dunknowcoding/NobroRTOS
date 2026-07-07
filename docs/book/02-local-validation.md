# Local Validation

Most NobroRTOS work can be reviewed before touching hardware. Start with the
software gates:

```powershell
cd core
cargo test -p nobro-kernel --target x86_64-pc-windows-msvc
cargo test -p nobro-sal --target x86_64-pc-windows-msvc
cargo test -p nobro-net --target x86_64-pc-windows-msvc
```

From the repository root, run the host-facing gates:

```powershell
python tools/nobro_contract_tool.py check-software-surface
python tools/verify_timing_lease.py
python tools/tutorial_runner.py
```

Use `_work/` for generated outputs, logs, downloaded datasets, and local build
products. The repository is designed so public sources stay small, readable,
and reproducible.

If a gate fails, read the first failing contract. The usual fix is to make a
module budget, capability, startup dependency, or buffer size explicit rather
than adding a hidden runtime assumption.
