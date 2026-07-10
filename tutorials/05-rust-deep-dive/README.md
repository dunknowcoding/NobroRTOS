# 05 — Rust Deep Dive 🦀

*The professional surface: swap driver backends by feature flag, price your firmware
before flashing, and ship evidence instead of promises.*

## What you need

| Thing | Where |
| --- | --- |
| Rust + the embedded target | `rustup target add thumbv7em-none-eabihf` and `rustup component add llvm-tools-preview` ([rustup.rs](https://rustup.rs)) |
| Python 3.10+ | for the eval/verify tooling |
| An nRF52840 board + J-Link (deep-HAL tier) | see the support tiers table in the README — conformance ports (ESP32-C3/S3, RP2350) work for the portable core |

## Exercise 1 — One app, three transports (the UDI)

`udi_imu_demo` reads the same physical IMU through three interchangeable backends
behind one `ImuSal` trait — the app's evaluation code never names a transport:

```bash
python tools/nobro_hw_eval.py udi --profile s140 --backend native   # HAL register driver
python tools/nobro_hw_eval.py udi --profile s140 --backend eh       # embedded-hal driver
python tools/nobro_hw_eval.py udi --profile s140 --backend arduino  # an Arduino-style C++ lib via the shim
```

All three must seal the same PASS; the report's `backend_id` proves which transport
ran. Read the pattern in [docs/ARCHITECTURE.md](../../docs/ARCHITECTURE.md) (UDI
section), then add a fourth backend for a sensor you own — the whole point is that
this takes a feature flag and an impl block, not a fork.

## Exercise 2 — Know your worst case before you flash

```bash
python tools/static_budget.py core/target/thumbv7em-none-eabihf/release/udi_imu_demo
```

Call-graph-priced worst-case stack, static RAM, flash, and cycles. Ceilings live in
`host/nobro-host-contract.json` (`build_budgets`) and the Evidence Pack **fails** if
you exceed them. Kernel-op costs are measured, not folklore:
`python tools/nobro_hw_eval.py wcet --profile s140` reproduces the numbers in
[docs/ENGINEERING.md](../../docs/ENGINEERING.md).

## Exercise 3 — Ship evidence

```bash
python tools/run_checks.py          # every software gate → one ALL PASS + Evidence Pack
python tools/fleet_evidence.py      # fold software + OTA + hardware runs into a fleet verdict
```

Open `_work/evidence/evidence_pack.html`. That artifact — gates, budgets, commit —
is the deliverable that distinguishes this RTOS: the claim and its proof travel
together.

## ✔ Verify

- [ ] Two different `backend_id`s sealed PASS on the same board
- [ ] A deliberately tightened budget ceiling flips the Evidence Pack to FAIL (then restore it)
- [ ] `run_checks.py` ends `RESULT: ALL PASS`

You're now past the tutorials. The map of everything: [docs/README.md](../../docs/README.md).
