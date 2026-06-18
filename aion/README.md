# AIRON

AIRON is a small embedded runtime for nRF52840-class robotics nodes. It is not a
PC operating system. The first target is ArduinoNRF-compatible ProMicro
nRF52840 boards with application images linked either at `0x1000`
(`board-promicro-nosd`) or `0x26000` (`board-nicenano-s140`).

## Design Goals

- Keep the kernel small: deadline scheduling, sample tickets, static pools, and
  error policy live in `airon-kernel`.
- Keep hardware compatibility explicit: register-level behavior, board layout,
  leases, PPI capture, PWM, and TWIM live behind `airon-hal`.
- Keep user code portable: applications and adapters should depend on the six
  traits in `airon-sal` instead of vendor headers.
- Keep bring-up friendly: every phase should have a local software check before
  it needs an external board.

See `docs/architecture.md` for the maintainability, compatibility, partitioning,
self-recovery, and memory-discipline rules that guide new modules.

## Repository Map

| Path | Purpose |
| ---- | ------- |
| `crates/airon-kernel` | Deadline scheduler, sample pool, eval gates, error policy |
| `crates/airon-hal` | Platform traits and the nRF52840 backend |
| `crates/airon-sal` | Bus, stream, radio, actuator, sensor, and crypto traits |
| `crates/airon-host` | Host contract constants shared by scripts and docs |
| `adapters/*` | Thin SAL adapters for concrete devices or compatibility stubs |
| `apps/*` | Firmware entry points assembled from HAL, kernel, SAL, and adapters |

## Local Checks Without Hardware

Run these from `aion/`:

```powershell
$env:CARGO_TARGET_DIR = (Resolve-Path '..\_work').Path + '\cargo-target'
cargo test -p airon-kernel --target x86_64-pc-windows-msvc -- --test-threads=1
cargo test -p airon-host --target x86_64-pc-windows-msvc
cargo check --workspace
```

`cargo check --workspace` uses `.cargo/config.toml`, so it checks the embedded
`thumbv7em-none-eabihf` target by default.

## Compatibility Notes

nRF52840 uses PPI for event-to-task wiring. AIRON names the portable abstraction
`HalEventCapture` so future ports can map the same concept to STM32 trigger
routing, RP2040 PIO, or other peripheral fabrics without exposing nRF-specific
registers to applications.

Board-specific bootloader layouts are represented by Cargo features:

| Feature | App start | Intended boards |
| ------- | --------- | --------------- |
| `board-promicro-nosd` | `0x1000` | board1-board3, J-Link or UF2 no-SoftDevice |
| `board-nicenano-s140` | `0x26000` | board4-board5, SoftDevice S140 v6 layout |

Do not run full-chip recover, erase-all, or bootloader reflashing as part of
AIRON validation. Application-area flashing only is the expected policy.

## Current Phase

Phase 1 focuses on ArduinoNRF parity and resource scheduling:

- TIMER0 microsecond timebase
- PPI timestamp capture
- exclusive leases and `LeaseGuard` RAII for Timer0, TWIM0/1, SPIM0, RADIO,
  RTC2, and TIMER3
- 50 Hz deadline slot scheduling
- PWM and TWIM behavior checks
- host-readable eval reports

Phase 2 starts adapter usability with `robo-servo`, `sensor-stub`, and
`mpu9250-imu`, while larger radio and satellite-library adapters remain future
work.
