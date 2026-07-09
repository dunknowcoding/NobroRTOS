# Universal Driver Interface (UDI)

NobroRTOS treats drivers the way Adafruit Unified Sensor treats sensors: **one
category, one trait, many mountable backends.** A part is catalog data; a backend
is a compile-time feature that plugs a concrete library or transport behind the
same SAL trait.

This is the public rule behind the `ImuSal` hardware proof (`udi_imu_demo`) and
the pattern to extend to other sensor categories.

## The rule

```
Category trait (SAL)     e.g. ImuSal::sample()
    ├─ backend-native      register driver in-tree (mpu9250-imu)
    ├─ backend-eh          any embedded-hal driver crate
    ├─ backend-c-module    C/C++ module via nobro_app.h
    └─ backend-arduino     stock Arduino library via NobroArduinoShim
```

Every backend:

1. Implements the **same category trait** (`ImuSal`, `TempSal`, more to come).
2. Is selected by **exactly one** `backend-*` Cargo feature (mutual exclusion).
3. Carries a stable **`backend_id`** in the hardware eval report so you can prove
   which transport sealed the PASS without the evaluation function naming a driver.
4. Runs through the **same eval body** — only the mount changes.

## What transfers vs what you re-express

| From your existing code | UDI answer |
| --- | --- |
| Arduino sensor library | `backend-arduino` shim behind the category trait |
| `embedded-hal` driver crate | `backend-eh` adapter |
| Register-level C driver | `backend-c-module` via `nobro_app.h` |
| In-tree Nobro driver | `backend-native` |
| Task / loop / executor | NobroRTOS module + manifest (see cookbooks) |

## Proven today: `ImuSal`

`core/apps/udi_imu_demo` shares one `app.rs` evaluation across three binaries:

| Backend | Feature | `backend_id` | Transport |
| --- | --- | --- | --- |
| Native HAL | `backend-native` | 1 | SPI via `nobro_hal` |
| embedded-hal | `backend-eh` | 2 | SPI via `SpiDevice` |
| Arduino shim | `backend-arduino` | 3 | SPI via `NobroArduinoShim` + stock MPU9250 class |

Hardware eval:

```bash
python tools/nobro_hw_eval.py udi --profile s140 --backend arduino
python tools/nobro_hw_eval.py udi --profile s140 --backend native
python tools/nobro_hw_eval.py udi --profile s140 --backend eh
```

All three must report `all_pass=1` with the expected `backend_id`. The eval
function never names a transport.

## Adding a new category

1. Define a **category trait** in `nobro_sal` with bounded return types (no heap).
2. Add a **catalog entry** in `nobro_device` (part id, bus, who-am-i, ranges).
3. Ship at least **two backends** (native + eh is the minimum credible proof).
4. Add a **swap demo app** with one shared eval body and feature-gated mounts.
5. Gate it in `nobro_hw_eval.py` and `run_checks.py`.

## Adding a new backend to an existing category

1. Implement the category trait in a new adapter crate or C/C++ module.
2. Add a `backend-*` feature with `compile_error!` if more than one is enabled.
3. Wire the mount in the demo app's `main.rs` (thin — only constructs the backend).
4. Flash and read the report; `backend_id` must be unique and documented.

## Related docs

- [PORTING_FROM.md](PORTING_FROM.md) — high-level import story
- [COOKBOOK_EMBASSY.md](COOKBOOK_EMBASSY.md) — task→module for Rust async refugees
- [COOKBOOK_FREERTOS.md](COOKBOOK_FREERTOS.md) — xTaskCreate→module for C veterans
- [HW_QUICKSTART.md](HW_QUICKSTART.md) — one-command hardware eval


## Second category: `TempSal` (hardware-proven)

The rule generalizes: `TempSal::read_temp_centi_c()` reports centi-degrees Celsius from
whatever part a backend wraps. All three `udi_imu_demo` backends implement it against the
same die-temperature register, sealed on the same board back to back:

| Backend | `backend_id` | temp reading |
| --- | --- | --- |
| native HAL | 1 | 31.24 C |
| embedded-hal | 2 | 31.20 C |
| Arduino-library shim | 3 | 31.15 C |

Three transports, one silicon, answers within 0.1 C - the category abstraction costs
nothing in fidelity. The report's `temp_centi_c` field and its 10-60 C plausibility
check are part of `all_pass`.
