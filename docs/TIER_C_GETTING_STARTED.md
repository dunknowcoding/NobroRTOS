# Tier C: Write C, Link, Flash — No Rust Installed

Tier C is for C developers who want NobroRTOS's control plane (admission, budgets,
leases, `NOBRO_*` reports) without touching the Rust toolchain. You get a prebuilt
`libnobro.a` containing the whole runtime — boot, vector table, kernel, host services —
and you supply one C file.

## 1. Get the bundle

From a release download, or from anyone with the Rust toolchain:

```bash
python tools/build_libnobro.py --build     # stages _work/tierc/
```

The bundle: `libnobro.a`, the linker scripts it expects (`link.x`, `memory.x`,
`defmt.x`), the C ABI headers (`nobro_app.h`, `nobro_rtos.h`), a reference module
(`imu_module.c`), and one-line build scripts.

## 2. Write your module

Your entire authoring surface is two functions against `nobro_app.h`:

```c
#include "nobro_app.h"

int32_t nobro_app_init(void) {
    uint8_t wake[2] = {0x6B, 0x01};
    return nobro_i2c_write(0x68, wake, 2);      /* bounded host service */
}

int32_t nobro_app_poll(void) {
    /* nobro_i2c_write_read(...) + nobro_publish_imu(...) */
    return 0;
}
```

The kernel admits your module (capabilities, memory budget, deadlines) and drives
`init`/`poll`; hardware is reachable only through the bounded host services.

## 3. Link (this is the whole build)

```bash
./build.sh my_module.c        # or build.cmd my_module.c on Windows
# = arm-none-eabi-gcc <cpu flags> my_module.c \
#     -Wl,--whole-archive libnobro.a -Wl,--no-whole-archive \
#     -T link.x -T defmt.x -nostartfiles -lm -o firmware.elf
```

`--whole-archive` matters: the vector table lives in the archive and nothing
references it by symbol, so the linker must keep every member.

## 4. Flash + verify

The ELF targets the no-SoftDevice layout (app at `0x1000`). Flash it with any SWD
probe (`docs/HW_QUICKSTART.md`) or convert to UF2 for drag-and-drop. Verification is
the usual story: the firmware seals `NOBRO_IMU_HW_EVAL_REPORT`, and
`tools/nobro_hw_eval.py` or a serial monitor grades it PASS/FAIL.

## Current scope, honestly

- One prebuilt layout today: **nRF52840, no-SoftDevice**. The S140 variant is a
  rebuild flag away for whoever produces bundles.
- The link test (`python tools/build_libnobro.py --check`) runs in CI, so a bundle
  that stops linking against plain gcc fails the gate before it reaches you.
