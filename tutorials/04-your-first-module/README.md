# 04 — Your First Module 🔧

*Run YOUR C code under a real RTOS kernel — admission, budgets, capability checks —
without installing Rust.*

NobroRTOS modules are two C functions. The kernel admits your module against a
manifest (memory budget, capabilities), then drives it. You link against a prebuilt
runtime archive: `libnobro.a`.

## What you need

| Thing | Where |
| --- | --- |
| `arm-none-eabi-gcc` | [Arm GNU toolchain](https://developer.arm.com/downloads/-/arm-gnu-toolchain-downloads) (Windows: the MSYS2 `arm-none-eabi-gcc` package also works) |
| The Tier C bundle | someone with Rust runs `python tools/build_libnobro.py --build` → gives you `_work/tierc/` (archive + headers + linker scripts + build scripts) |
| Optional: an nRF52840 board + SWD probe to run it | tier 01's board + a J-Link/CMSIS-DAP |

## Step 1 — Meet the whole API

Open `nobro_app.h` from the bundle. Your entire authoring surface:

```c
#include "nobro_app.h"

int32_t nobro_app_init(void) {                 /* called once after admission   */
    uint8_t wake[2] = {0x6B, 0x01};
    return nobro_i2c_write(0x68, wake, 2);     /* bounded host service          */
}

int32_t nobro_app_poll(void) {                 /* called forever by the kernel  */
    /* read a sensor, publish a sample: nobro_i2c_write_read + nobro_publish_imu */
    return 0;
}
```

You never touch registers. You *request* services; the kernel's capability table
decides — that's what makes a module safe to compose.

## Step 2 — Build (one line)

```bash
./build.sh my_module.c        # Windows: build.cmd my_module.c
```

That's `gcc → firmware.elf`, linking your object against the whole prebuilt runtime
(kernel, HAL, vector table). Full explanation of the flags:
[docs/USER_GUIDE.md](../../docs/USER_GUIDE.md), "Tier C" section.

## Step 3 — Run it (if you have hardware)

Flash `firmware.elf` per [docs/GETTING_STARTED.md](../../docs/GETTING_STARTED.md).
Your module's health lands in `NOBRO_IMU_HW_EVAL_REPORT`, readable over the probe or
a serial monitor — the same PASS/FAIL story as every tier.

## ✔ Verify

- [ ] `firmware.elf` links with no Rust installed
- [ ] `arm-none-eabi-nm firmware.elf | grep nobro_app_init` shows YOUR symbol inside a full RTOS image

Next: [05 — Rust Deep Dive](../05-rust-deep-dive/README.md) →
