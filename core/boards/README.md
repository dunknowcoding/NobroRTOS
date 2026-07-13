# Board Profiles

Hardware facts are kept **data-first**, one directory per board, so a new board is a
data drop plus a HAL platform port, not edits scattered across drivers and apps.

```text
boards/
  nrf52/
    nrf52840-nosd/    board.json   # no-SoftDevice layout (app @ 0x1000)
    nrf52840-s140/    board.json   # S140 v6 layout (app @ 0x26000)
  esp32/esp32c3-supermini/board.json
  rp2/rp2350-pico2w/board.json
  samd/samd21-uf2/board.json
  stm32/stm32f4-generic/board.json
  teensy/teensy4-generic/board.json
  generic/cortexm-generic/board.json
```

Each `board.json` carries the boot memory layout, capacity budgets, and critical pins.
These mirror the compiled `BoardDesc`/`BoardPackage` fixtures in
`crates/nobro_hal/src/board.rs` and `crates/nobro_hal/src/board_catalog.rs`, and are
checked for internal consistency by `tools/check_board_profiles.py`.

An application crate selects exactly one board via its `feature` (for example,
`board-promicro-nosd`). A board profile is a validated compatibility contract; full
peripheral support lands when the matching HAL platform backend is added.
