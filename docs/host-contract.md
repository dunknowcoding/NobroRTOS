# NobroRTOS Host Contract

The host contract defines the data that external tools can read from firmware
images or runtime memory. The JSON mirror is:

```text
host/nobro-host-contract.json
```

The Rust mirror is:

```text
core/crates/nobro_host
```

## Stable Labels

Module tag labels include kernel, HAL, bus, radio, sensor, actuator, stream,
crypto, AI, and app modules. Capability labels include timebase, deadline
timer, event capture, bus, radio, servo PWM, stream, crypto, sample pool, host
report, AI inference, and AI endpoint ownership.

## Report Symbols

| Symbol | Meaning |
| ------ | ------- |
| `NOBRO_BOARD_PROFILE_REPORT` | Selected board, memory origin, budgets, and critical pins |
| `NOBRO_BOARD_PACKAGE_REPORT` | Boot layout, flash/RAM regions, board capacity, critical pins, and package validation result |
| `NOBRO_MANIFEST_REPORT` | Static module graph validity, capability bits, budget use, and error context |
| `NOBRO_ADAPTER_COMPAT_REPORT` | Adapter inventory compatibility before app admission |
| `NOBRO_ADMISSION_REPORT` | Admission result after manifest, startup, quota, and capability checks |
| `NOBRO_RUNTIME_REPORT` | Runtime lifecycle, mailbox pressure, alarms, KV writes, quota use, and event pressure |
| `NOBRO_HEALTH_REPORT` | Module health counters and latest recovery context |
| `NOBRO_EVENT_LOG_REPORT` | Fixed event-ring summary |
| `NOBRO_MODULE_RUNTIME_REPORT` | Module state counts and latest state transition |
| `NOBRO_DEGRADE_APPLICATION_REPORT` | Latest degraded-mode application result |
| `NOBRO_EVAL_REPORT` | Phase 1 resource scheduling evaluation record |
| `NOBRO_SAL_EVAL_REPORT` | SAL adapter evaluation record |

## Status Model

Reports use the same status categories:

- `missing`: zeroed report slot
- `in_progress`: valid header, incomplete report
- `pass`: complete and checksum-valid success
- `fail`: complete and checksum-valid domain failure
- `corrupt`: invalid header, version, or checksum

Host tools should decode the first non-passing boot stage in this order:

1. board profile
2. board package
3. manifest
4. adapter compatibility
5. admission
6. runtime

## Boot Summary

`airon-host` exposes `BootReports::summary()` for tools that need one compact
view of boot state. The summary includes the first diagnostic, all six report
slots, diagnostic code, and per-status counts. Tools should use this helper
before rendering user-facing text.
## Checksum Rule

Fixed reports use XOR checksums over every `u32` field except `checksum`.
Timestamps wider than `u32` are split into low and high words.

## Diagnostic Code

Boot diagnostic code layout:

```text
stage_code << 24 | status_class << 16 | error_code_low16
```

Use `airon-host` helper labels rather than duplicating numeric maps in host
tools.
