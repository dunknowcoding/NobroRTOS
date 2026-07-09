# Getting Started Without Writing Code

The zero-toolchain path: flash one prebuilt image by drag-and-drop, watch the board
explain itself in a browser, and design your first app as blocks — no Rust, no
compiler, no IDE.

## 1. Flash the starter (once)

You need an nRF52840 board with the UF2 (S140) bootloader — most nice!nano-style
boards ship with it.

1. Double-tap the RESET button. A USB drive appears (its `INFO_UF2.TXT` should say
   `SoftDevice: S140`).
2. Drag `nobrortos-starter-s140.uf2` onto the drive. The board reboots on its own.

Where to get the UF2: a release download, or anyone with the toolchain runs
`python tools/package_prebuilt_uf2.py --build` and hands you the file from
`_work/prebuilt/`.

## 2. Watch it explain itself

Open `packages/web-flasher/index.html` in Chrome or Edge and click
**Open report console**, then pick the board's serial port. The starter streams its
self-verification and the console translates it into plain sentences:

```
✅ PASS  CDC: all checks passing
NobroRTOS IMU who=0x71 addr=104 i2c=1 reads=1240 err=0 accel=1002mg ... PASS
```

If the board's sensor is missing or mis-wired, you see exactly which check failed —
the same first-fault discipline every NobroRTOS app has.

(No browser with Web Serial? Any serial monitor at 115200 shows the same lines, and
`pip install nobro-rtos-tools` gives you a Python decoder.)

## 3. Design your own app as blocks

Open `packages/block-editor/index.html`, arrange board + servo + sensor (+ ML)
blocks, and export `app.json`. Validate it instantly — this needs only Python:

```bash
python tools/nobro_app.py your-app.json          # catalog-checked plan, PASS/FAIL
```

## 4. When you outgrow no-code

Turning your `app.json` into firmware is one command with the toolchain installed
(`python tools/nobro_app.py your-app.json --gen main.rs`, then build) — or hand the
JSON to anyone with the toolchain. Every path lands back at the same report console,
so nothing you learned here is thrown away.

| You are here | Next rung |
| --- | --- |
| No-code starter | Arduino library (`packages/arduino`, no Rust needed) |
| Arduino sketches | Python host tools (`pip install nobro-rtos-tools`) |
| Python | C/C++ modules, then the full Rust workspace |
