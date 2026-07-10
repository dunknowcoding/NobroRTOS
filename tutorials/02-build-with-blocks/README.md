# 02 — Build with Blocks 🧩

*Design a robot app with no syntax at all — blocks in, working plan out.*

In NobroRTOS an app can be **data instead of code**: which board, which servo,
which sensor, what behavior. You'll build that data with a visual editor and have
the toolchain check it like a real engineer's plan.

## What you need

| Thing | Where to get it |
| --- | --- |
| A web browser | you have one |
| This repository | download the ZIP from GitHub (green "Code" button → Download ZIP) and unzip it |
| Python 3.10+ (for the verify step) | [python.org/downloads](https://www.python.org/downloads/) — tick "Add to PATH" during install |

## Step 1 — Open the block editor

Double-click [`packages/block-editor/index.html`](../../packages/block-editor/index.html).
It runs entirely in your browser — nothing to install, nothing leaves your computer.

## Step 2 — Snap an app together

Pick a **board** block (say, the nRF52840), snap in a **servo** (the SG90 is the
classic little blue one), a **sensor** (the MPU6050 motion sensor), and a
**behavior** ("sweep the arm when the sensor feels a tap"). Press **Export** —
you get a small file called `app.json`. Open it in any text editor and look:
it's readable! Every choice you made is right there as plain data.

## Step 3 — Let the toolchain check your plan

This is the magic step. Open a terminal (Windows: press Start, type `cmd`) in the
unzipped folder and run:

```bash
python tools/nobro_app.py path/to/your/app.json
```

You'll see the toolchain *validate* your design against real catalogs — does that
servo exist? does the sensor's address make sense? — and print a plan:

```
  actuator arm: sg90 on PWM channel 0
  sensor   imu: mpu6050 on i2c @ 0x68
RESULT: PASS (app is valid)
```

A ready-made example lives in [`../hello-device/app.json`](../hello-device/app.json) —
try validating it first, then break it on purpose (change `"sg90"` to `"sg999"`)
and watch the validator *catch your mistake and explain it*.

## ✔ Verify

- [ ] Your exported `app.json` validates with `RESULT: PASS`
- [ ] A deliberately wrong brand name gets **caught with a clear message**

## Where this leads

`--gen main.rs` turns your validated plan into real firmware source. Building that
needs the pro toolchain from tier 05 — but notice what you already did: you designed
a hardware app precisely enough for a compiler, without writing a line of code.
Next: [03 — Arduino & Python](../03-arduino-and-python/README.md) →
