# 05 — Rust deep dive

This tier covers backend substitution, static resource pricing, and the complete
portable source/package check.

## What you need

| Item | Setup |
| --- | --- |
| Rust and the embedded target | `rustup target add thumbv7em-none-eabihf` |
| LLVM tools | `rustup component add llvm-tools-preview` |
| Python 3.10+ | SDK and source utilities |
| Supported board and upload tool | Choose from the support tiers in the main README |

## Exercise 1 — one app, multiple backends

`udi_imu_demo` keeps the application body behind one `ImuSal` contract while the
selected feature supplies the native, `embedded-hal`, or Arduino-style backend.
Build each feature variant and confirm that it exposes the same status layout with a
different `backend_id`. Then add another backend without forking the application.

## Exercise 2 — price the firmware

```bash
python tools/static_budget.py core/target/thumbv7em-none-eabihf/release/udi_imu_demo
```

The tool reports static RAM, flash, call-graph stack depth, and a conservative static
cycle envelope. Treat these as review inputs; deployment timing still depends on the
target, compiler, interrupts, buses, and workload.

## Exercise 3 — run the portable checks

```bash
python tools/run_checks.py
```

The command should end with `RESULT: ALL PASS`. Build outputs remain under ignored
`_work/` or Cargo target directories.

## Verify

- [ ] Two backend variants preserve the same public status layout
- [ ] The static budget tool reports the expected ELF sections
- [ ] `run_checks.py` ends with `RESULT: ALL PASS`

Continue with the [documentation index](../../docs/README.md).
