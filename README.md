# NobroRTOS

NobroRTOS is a compact, Rust-first embedded RTOS for robotics nodes that need
deadline-aware control, disciplined resource ownership, readable diagnostics,
and a path from one board to many boards without rewriting application logic.

The project starts with nRF52840-class boards and a deliberately small kernel
surface: manifests, quotas, capability grants, static sample pools, health
reports, recovery policy, and a six-trait service abstraction layer. The intent
is not to imitate a desktop OS. NobroRTOS is built for microcontrollers where a
servo pulse, an I2C transaction, a radio slot, and a recovery decision all have
to coexist inside tight memory and timing budgets.

**Repository:** https://github.com/dunknowcoding/NobroRTOS

**Author:** dunknowcoding (YouTube NiusRobotLab)

**License:** Apache-2.0

## Why NobroRTOS Exists

Robotics firmware often grows in an uncomfortable direction: a board package
owns the pins, a driver owns timing, an app owns recovery, a host script owns
the truth, and every new board adds another private rule. NobroRTOS pushes those
rules into explicit contracts so the system remains teachable, debuggable, and
portable.

The design target is a friendly RTOS with strong engineering bones:

- deadline-first scheduling primitives for control loops and timestamp capture
- static allocation and fixed-capacity reports on critical paths
- capability-based module admission before runtime work begins
- SAL traits that let apps speak to buses, streams, radios, actuators, sensors,
  and crypto without vendor headers
- host-readable reports for board profile, manifest, adapter compatibility,
  admission, runtime, event log, health, and degraded-mode decisions
- board data and feature gates that make compatibility visible instead of
  implicit

## Current Progress

NobroRTOS already has a working Rust workspace under `core/` with kernel, HAL,
SAL, host contract, adapters, and demo applications. The strongest completed
area is the software control plane: manifests, quota accounting, capability
grants, runtime disable paths, mailbox cleanup, alarm cleanup, degraded-mode
reports, and host-readable diagnostics are covered by local Rust tests.

The next engineering focus is adapter maturity and board-family expansion:
finish the portable board description model, harden adapter manifests, improve
host tooling around the `NOBRO_*` report contract, and keep each hardware-facing
feature backed by a software validation gate before board evidence is collected.

## Repository Layout

| Path | Purpose |
| ---- | ------- |
| `core/` | Rust workspace for kernel, HAL, SAL, host contract, adapters, and demo apps |
| `core/crates/airon-kernel` | Deadline, manifest, quota, capability, IPC, recovery, health, and reports |
| `core/crates/airon-hal` | Board descriptions, nRF52840 HAL backend, leases, timers, PWM, bus, capture |
| `core/crates/airon-sal` | Stable service traits for apps and adapters |
| `core/crates/airon-host` | Host-side constants and report decoders |
| `core/adapters/` | Thin SAL adapters for real devices and compatibility stubs |
| `core/apps/` | Firmware compositions and evaluation apps |
| `host/nobro-host-contract.json` | JSON mirror of the host report contract |
| `docs/` | User manual, API manual, architecture, porting, operations, and roadmap |

The Rust crate prefix is still `airon-*` while the product and repository are
now NobroRTOS. That keeps this branding step small and keeps downstream code
stable until a dedicated crate-name migration is worth the churn.

## Quick Start

Install Rust and the embedded target:

```powershell
rustup target add thumbv7em-none-eabihf
```

Run host-side validation from the workspace:

```powershell
cd core
$env:CARGO_TARGET_DIR = (Resolve-Path '..\_work').Path + '\cargo-target'
cargo test -p airon-kernel --target x86_64-pc-windows-msvc
cargo test -p airon-sal --target x86_64-pc-windows-msvc
cargo test -p airon-host --target x86_64-pc-windows-msvc
```

Use `_work/` for local build products, downloaded tools, logs, and scratch
artifacts. It is intentionally ignored by Git.

## Documentation

- [User Manual](docs/user-manual.md)
- [API Manual](docs/api-manual.md)
- [System Architecture](docs/system-architecture.md)
- [Porting Guide](docs/porting-guide.md)
- [Host Contract](docs/host-contract.md)
- [Operations Guide](docs/operations-guide.md)
- [Roadmap](docs/roadmap.md)

## Design Influences

NobroRTOS borrows carefully from proven embedded systems ideas: hardware
description as data from Zephyr devicetree, static async direction from Embassy,
component isolation from Tock, and mixed-criticality discipline from seL4 MCS.
The project keeps those ideas small enough for approachable robotics firmware.
