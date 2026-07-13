# 05 — Rust Deep Dive 🦀

*The professional surface: swap driver backends by feature flag, price your firmware
before flashing, and ship evidence instead of promises.*

## What you need

| Thing | Where |
| --- | --- |
| Rust + the embedded target | `rustup target add thumbv7em-none-eabihf` and `rustup component add llvm-tools-preview` ([rustup.rs](https://rustup.rs)) |
| Python 3.10+ | for the public SDK and verification tooling |
| A supported board and upload tool | see the support tiers table in the README; conformance does not imply deep peripheral support |

## Exercise 1 — One app, three transports (the UDI)

`udi_imu_demo` reads the same physical IMU through three interchangeable backends
behind one `ImuSal` trait — the app's evaluation code never names a transport:

Build the `backend-native`, `backend-eh`, and `backend-arduino` feature variants. Each
must seal the same report shape; `backend_id` identifies the selected transport.
Read the pattern in [docs/ARCHITECTURE.md](../../docs/ARCHITECTURE.md) (UDI
section), then add a fourth backend for a sensor you own — the whole point is that
this takes a feature flag and an impl block, not a fork.

## Exercise 2 — Know your worst case before you flash

```bash
python tools/static_budget.py core/target/thumbv7em-none-eabihf/release/udi_imu_demo
```

Call-graph-priced worst-case stack, static RAM, flash, and cycles. Ceilings live in
`host/nobro-host-contract.json` (`build_budgets`) and the Evidence Pack **fails** if
you exceed them. Treat this as a static bound; deployment timing still requires
target-specific measurement.

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
