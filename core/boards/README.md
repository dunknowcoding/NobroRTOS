# Board Profiles

Hardware facts are kept **data-first**, one directory per board, so a new board is a
data drop plus a HAL platform port, not edits scattered across drivers and apps.

```text
boards/
  nrf52840-nosd/    board.json   # nRF52840, no-SoftDevice layout (app @ 0x1000)
  nrf52840-s140/    board.json   # nRF52840 + S140 v6 SoftDevice (app @ 0x26000)
  esp32c3-supermini/board.json   # ESP32-C3 portable-core profile
  rp2350-pico2w/    board.json   # RP2350 portable-core profile
  samd21-uf2/        board.json   # SAMD21 UF2/SAM-BA-class profile
  stm32f4-generic/   board.json   # generic STM32F4 Cortex-M4 profile
  teensy4-generic/   board.json   # generic i.MX RT1062 / Teensy 4-class profile
  cortexm-generic/   board.json   # small generic Cortex-M planning profile
```

Each `board.json` carries the boot memory layout, capacity budgets, and critical pins.
These mirror the compiled `BoardDesc`/`BoardPackage` fixtures in
`crates/nobro_hal/src/board.rs` and `crates/nobro_hal/src/board_fixtures.rs`, and are
checked for internal consistency by `tools/check_board_profiles.py`.

An application crate selects exactly one board via its `feature` (for example,
`board-promicro-nosd`). A board profile is a validated compatibility contract; full
peripheral support lands when the matching HAL platform backend is added.
