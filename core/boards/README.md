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
contract. Standalone images name their target, linker, flags, and ownership; Arduino
compositions name their exact FQBN and header; maintained ports name their manifest.
The generator emits `generation.json` and `DEPLOY.md` for all maintained routes, builds
only routes that really link the declaration, and rejects unavailable profiles instead
of pretending a generic image is safe.
`tools/build/generate_board_catalog.py` generates the typed Rust
`EXACT_BOARD_PROFILES` catalog from these files. The profile gate rejects stale
generated output and reconciles every implemented `BoardPackage` against it.
Do not edit `generated_board_profiles.rs` directly.

`candidate_families.json` is a separate, fail-closed intake list for families and
exact market boards that do not yet have a supported board profile. Family entries
record upstream routes; market-board entries distinguish `discovered`, `cataloged`,
and exact `target-compiled` states. Even the compiled state proves only the named
target build: it grants no peripheral, boot, recovery, physical, or support claim.
An exact device and compiler/core version are required before a compile contract can
be recorded. Candidate C/RTL
witnesses live under `core/candidates/kernel_lite`; image, object, simulation, board,
and physical states remain distinct. The 8051 entry is limited to C/Arduino ABI or
kernel-lite feasibility until a maintained Rust target, atomic model, and runtime are
demonstrated.

An application crate selects exactly one board via its `feature` (for example,
`board-promicro-nosd`). A board profile is a validated compatibility contract; full
peripheral support lands when the matching HAL platform backend is added.

The canonical S140 selector is `board-promicro-s140`.
`board-nicenano-s140` remains a compatibility alias only.

Native capability truth is separate from board facts. The versioned vocabulary,
deep/constrained profiles, four-state declarations, and exact compiled witnesses
live in [`hal_contract_v2.json`](hal_contract_v2.json) and are reconciled with
`platform_tiers.json` by the HAL contract gate.

`ai_profiles.json` is the public admission envelope for Q4/Q2 inference. It records
reserved flash/RAM/stack/scratch, model bytes, maximum dense shape/MACs, alignment,
and unsupported operations by MCU family. These are limits, not benchmark claims:
each deployment must privately prove its worst-case cycles fit its configured
deadline. Raw board measurements, model inputs, and comparison data are never part
of this public registry.
