# Board Profiles

Hardware facts are kept **data-first**, one directory per board, so a new board is a
data drop plus a HAL platform port — not edits scattered across drivers and apps.

```text
boards/
  nrf52840-nosd/   board.json   # nRF52840, no-SoftDevice layout (app @ 0x1000)
  nrf52840-s140/   board.json   # nRF52840 + S140 v6 SoftDevice (app @ 0x26000)
```

Each `board.json` carries the boot memory layout, capacity budgets, and critical pins.
These mirror the compiled `BoardDesc`/`BoardPackage` fixtures in
`crates/nobro_hal/src/board.rs` (which the in-tree tests validate), and are checked for
internal consistency by `tools/check_board_profiles.py`.

An application crate selects exactly one board via its `feature` (e.g.
`board-promicro-nosd`). Today the HAL implements the **nRF52840** platform, so the
profiles here are two boot layouts of that MCU family; additional boards (RP2040, STM32)
land as the HAL gains those platforms — that is the open "Multi-board expansion" work.
