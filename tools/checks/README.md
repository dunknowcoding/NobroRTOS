# Checks

Reproducible contributor and hosted-CI gates. `run_checks.py` is the portable
local entry point; `ci_matrix.sh` adds the cross-MCU build matrix.

`check_firmware_generation.py` builds the same compact application for every
board profile that declares a standalone `application-image` contract. This
guards the board-owned target, linker, startup, clock, interrupt, DMA, and boot
metadata used by `nobro firmware`.

Hardware campaigns, competitor comparisons, raw evidence, and local device
identities are private maintainer inputs and are not part of this directory.
