# NobroRTOS Core Workspace

This directory contains the Rust implementation of NobroRTOS. The crate names
still use the `airon-*` prefix for continuity, while the product, host report
symbols, repository, and public documentation use NobroRTOS naming.

## Workspace Map

| Path | Role |
| ---- | ---- |
| `crates/nobro_kernel` | Manifest validation, deadline model, quotas, capability grants, IPC, health, recovery, and reports |
| `crates/nobro_hal` | Board descriptions, nRF52840 platform backend, resource leases, timers, PWM, bus, and event capture |
| `crates/nobro_sal` | Bus, stream, radio, actuator, sensor, and crypto traits |
| `crates/nobro_host` | Host constants and fixed-layout report decoders |
| `adapters/*` | Thin SAL implementations for concrete devices or compatibility stubs |
| `apps/*` | Firmware compositions used for evaluation and examples |

## Host Validation

```powershell
$env:CARGO_TARGET_DIR = (Resolve-Path '..\_work').Path + '\cargo-target'
cargo fmt --all -- --check
cargo test -p airon-kernel --target x86_64-pc-windows-msvc
cargo test -p airon-sal --target x86_64-pc-windows-msvc
cargo test -p airon-host --target x86_64-pc-windows-msvc
```

`cargo check --workspace` uses `.cargo/config.toml` and checks the embedded
`thumbv7em-none-eabihf` target by default.

## Compatibility Notes

Board-specific bootloader layouts are represented by Cargo features:

| Feature | App start | Intended boards |
| ------- | --------- | --------------- |
| `board-promicro-nosd` | `0x1000` | ProMicro-style nRF52840 boards without SoftDevice |
| `board-nicenano-s140` | `0x26000` | nRF52840 boards using a SoftDevice S140 v6 layout |

NobroRTOS uses `HalEventCapture` as the portable term for event-to-timestamp
routing. On the current nRF52840 backend this maps to PPI; future ports can map
the same concept to their own peripheral fabric without changing app code.
