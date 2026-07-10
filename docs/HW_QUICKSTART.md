# Hardware Quick Start

Everything on real hardware funnels through **one command**: flash an eval app, let it
run, then read its fixed `NOBRO_*` report back over the debug probe and grade it PASS
or FAIL. No serial console required.

## Prerequisites

- An nRF52840 dev board wired to a SEGGER J-Link (SWD). Other probes work with
  `probe-rs`; the tool flag below assumes J-Link.
- Rust (`rustup target add thumbv7em-none-eabihf`) and Python 3.10+.
- `arm-none-eabi-objcopy` on PATH (any GNU Arm toolchain provides it).

## One command

```bash
python tools/nobro_hw_eval.py imu            # build + flash + run + read + grade
python tools/nobro_hw_eval.py sal            # servo + sensor SAL round-trip
python tools/nobro_hw_eval.py eh             # embedded-hal driver path
python tools/nobro_hw_eval.py sched          # scheduler / PPI / PWM timing
```

Each run ends with an explicit verdict:

```
=== imu on nosd ===
  magic                  = 1313164366 (0x4E42...)
  all_pass               = 1 (0x1)
PASS: all_pass=1
```

Options: `--profile` picks the flash layout (`nosd` at 0x1000 or `s140` at 0x26000 for
SoftDevice boards), `--jlink <path>` points at a non-default J-Link CLI, `--no-build`
reuses the last binary.

## What "PASS" means

The app seals a fixed-layout report struct in RAM (`NOBRO_*_REPORT`); the tool reads it
via the probe and checks `magic`, `completed`, and `all_pass`. If a board is silent or
mis-wired you get a short read with a pointed message — not a hang.

## No probe? No board?

- **Serial boards:** most demos also print their report line over USB-CDC/UART; any
  serial monitor shows the same `all_pass=1`.
- **No hardware at all:** the Python simulators under `bindings/python` and the host
  test suite (`cargo test` on the portable crates, `tools/ci_matrix.sh`) exercise the
  same contracts on your desktop.

## Performance notes (facts, not folklore)

- Bus transfers (SPIM/TWIM) ride the nRF's **EasyDMA** - the CPU is not bit-banging
  or polling data bytes; drivers wait on transfer-end events with bounded spins.
- Sensor samples move through the kernel as **zero-copy tickets** (`SamplePool`):
  producers publish a slot, consumers borrow it - payloads are not copied through
  queues.
- Kernel-op costs are measured, not guessed: see
  [MEASURED_LATENCIES.md](MEASURED_LATENCIES.md).
