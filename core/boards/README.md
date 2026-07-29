# Board Profiles

Hardware facts are kept **data-first**, one directory per board, so a new board is a
data drop plus a HAL platform port, not edits scattered across drivers and apps.

```text
boards/
  nrf52/
    nrf52840-nosd/    board.json   # no-SoftDevice layout (app @ 0x1000)
    nrf52840-s140/    board.json   # S140 v6 layout (app @ 0x26000)
  avr/nano-v3-atmega328p/board.json
  esp32/<exact-board>/board.json
  esp8266/wemos-d1-mini/board.json
  renesas/uno-r4-wifi/board.json
  rp2/rp2040-pico/board.json
  rp2/rp2350-pico2w/board.json
  samd/samd21-uf2/board.json
  stm32/stm32f4-generic/board.json
  teensy/teensy4-generic/board.json
  generic/cortexm-generic/board.json
```

Every profile uses `nobro-board-profile-v2`. It independently names the silicon,
physical board, boot layout, framework core, HAL stack, and USB stack. Optional
pins use JSON `null`; negative pin sentinels are forbidden. A profile may be exact
and still report firmware generation as unavailable—catalog truth is not a support
claim.

Each `board.json` also carries the boot memory layout, capacity budgets, and
selected critical pins.
Profiles usable by `nobro firmware` also carry a board-owned `firmware_generation`
contract: target triple, entry model, linker source, rust flags, and explicit
interrupt/DMA/clock/boot ownership. Profiles whose startup must remain in a maintained
port name that manifest instead of pretending a generic image is safe.
`tools/build/generate_board_catalog.py` generates the typed Rust
`EXACT_BOARD_PROFILES` catalog from these files. The profile gate rejects stale
generated output and reconciles every implemented `BoardPackage` against it.
Do not edit `generated_board_profiles.rs` directly.

An application crate selects exactly one board via its `feature` (for example,
`board-promicro-nosd`). A board profile is a validated compatibility contract; full
peripheral support lands when the matching HAL platform backend is added.

The canonical S140 selector is `board-promicro-s140`.
`board-nicenano-s140` remains a compatibility alias only.

Native capability truth is separate from board facts. The versioned vocabulary,
deep/constrained profiles, four-state declarations, and exact compiled witnesses
live in [`hal_contract_v2.json`](hal_contract_v2.json) and are reconciled with
`platform_tiers.json` by the HAL contract gate.
