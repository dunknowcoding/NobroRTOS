# Getting Started

One page from zero to a verified PASS, whichever kind of user you are.
Pick your row, follow one section, and end at the same place: a device (or
simulator) explaining itself through `NOBRO_*` reports.

| You have | Start at |
| --- | --- |
| A compatible board and a prebuilt starter image | [Zero-code path](#zero-code-path-no-toolchain) |
| Rust + a probe/UF2 board | [Hardware quick start](#hardware-quick-start) |
| Just a laptop | [Toolchains and IDEs](#toolchains-and-ides) |

## Hardware quick start

Build or obtain an image for the exact board profile, flash it explicitly, then read its
fixed `NOBRO_*` report over the transport that application exposes.

### Prerequisites

- A supported board and its normal upload mechanism (UF2, Arduino, or a debug probe).
- Rust (`rustup target add thumbv7em-none-eabihf`) and Python 3.10+.
- The compiler and image tools required by the selected port.

### Public flash command

```bash
python sdk/cli/nobro.py flash --help
```

Applications that expose a serial report end with an explicit verdict:

```
=== imu on nosd ===
  magic                  = 1313164366 (0x4E42...)
  all_pass               = 1 (0x1)
PASS: all_pass=1
```

Image generation and upload settings are board-specific. Do not reuse an address, image,
or boot layout from another profile.

### What "PASS" means

The app seals a fixed-layout report (`NOBRO_*_REPORT`) and consumers check `magic`,
`completed`, `all_pass`, and its checksum. A silent target or unavailable peripheral is
not a passing result.

### No probe? No board?

- **Serial boards:** most demos also print their report line over USB-CDC/UART; any
  serial monitor shows the same `all_pass=1`.
- **No hardware at all:** the Python simulators under `bindings/python` and the host
  test suite (`cargo test` on the portable crates, `tools/ci_matrix.sh`) exercise the
  same contracts on your desktop.

### Performance notes (facts, not folklore)

- SPI transfers use EasyDMA. The current portable I2C provider uses bounded CPU-polled
  legacy TWI and reports `TransferMode::Polling`; contract checks prevent it from being
  advertised as DMA.
- Sensor samples move through the kernel as **zero-copy tickets** (`SamplePool`):
  producers publish a slot, consumers borrow it - payloads are not copied through
  queues.
- Resource and timing claims must be measured for the final target and workload.

## Zero-code path (no toolchain)

The zero-toolchain path requires a prebuilt image supplied by a release or another trusted
builder: flash it by drag-and-drop, watch the board
explain itself in a browser, and design your first app as blocks — no Rust, no
compiler, no IDE.

### 1. Flash the starter (once)

You need an nRF52840 board with the UF2 (S140) bootloader — most nice!nano-style
boards ship with it.

1. Double-tap the RESET button. A USB drive appears (its `INFO_UF2.TXT` should say
   `SoftDevice: S140`).
2. Drag `nobrortos-starter-s140.uf2` onto the drive. The board reboots on its own.

There is currently no published release artifact in this repository. A trusted builder runs
`python tools/package_prebuilt_uf2.py --build` and hands you the file from
`_work/prebuilt/`.

### 2. Watch it explain itself

Open `packages/web-flasher/index.html` in Chrome or Edge and click
**Open report console**, then pick the board's serial port. The starter streams its
self-verification and the console translates it into plain sentences:

```
✅ PASS  CDC: all checks passing
NobroRTOS IMU who=0x71 addr=104 i2c=1 reads=1240 err=0 accel=1002mg ... PASS
```

If the required sensor is unavailable, you see exactly which check failed —
the same first-fault discipline every NobroRTOS app has.

(No browser with Web Serial? Any serial monitor at 115200 shows the same lines; the
repository's Python report tools provide the decoder from a source checkout.)

### 3. Design your own app as blocks

Open `packages/block-editor/index.html`, arrange board + servo + sensor (+ ML)
blocks, and export `app.json`. Validate it instantly — this needs only Python:

```bash
python tools/nobro_app.py your-app.json          # catalog-checked plan, PASS/FAIL
```

### 4. When you outgrow no-code

Turning your `app.json` into firmware is one command with the toolchain installed
(`python tools/nobro_app.py your-app.json --gen main.rs`, then build) — or hand the
JSON to anyone with the toolchain. Every path lands back at the same report console,
so nothing you learned here is thrown away.

| You are here | Next rung |
| --- | --- |
| No-code starter | Arduino library (`packages/arduino`, no Rust needed) |
| Arduino sketches | Python host tools from this repository |
| Python | C/C++ modules, then the full Rust workspace |

## Toolchains and IDEs

### One project command

The SDK project flow keeps generated work under ignored `_work/projects/` by default:

```bash
python sdk/cli/nobro.py project new rover
python sdk/cli/nobro.py project run _work/projects/rover
python sdk/cli/nobro.py project report _work/projects/rover/reports/simulation.json
```

`run` explains the graph-derived contract and admission headroom, compiles a host
graph regenerated from the same `workload.json`, runs the bounded simulation, and
decodes its report. The generated scaffold is a host model, not a flashable image.

### One-file production firmware

The concise firmware path starts from `tutorials/rover-one-file/app.nobro` and generates
a real `no_std` nRF crate plus the workload consumed by admission/explain:

```bash
python sdk/cli/nobro.py firmware tutorials/rover-one-file/app.nobro --build
python sdk/cli/nobro.py project explain _work/projects/rover/workload.json
```

`board nrf52840-s140` and `board nrf52840-nosd` are deliberately distinct linker
profiles. The generator never chooses between them from a connected endpoint. Generated
files live in ignored `_work/projects` by default. Inferred role budgets are safe
starting estimates, not measured WCET; inspect them before flashing an application.
An optional `wake 25us` line after `board` admits a measured compare-wake-to-dispatch
upper bound. It is an engineering input, not a value inferred from a board name.

NobroRTOS is not tied to the Arduino IDE or VS Code. The core builds from a terminal on
Linux, macOS, or Windows with Rust, Python, the selected target support, and any external
flash utility required by that target.

### Pure CLI (recommended, any OS)

```bash
rustup target add thumbv7em-none-eabihf          # or your board's target
cd core
cargo build -p kernel-selftest --release          # build firmware
cd ..
python3 tools/flash.py jlink --bin app.bin --addr 0x1000   # J-Link (nRF)
python3 tools/flash.py uf2  --file app.uf2 --drive <DRIVE> # UF2 (Pico/nice!nano)
python3 tools/flash.py arduino --port <PORT> --fqbn <FQBN> --build-dir <DIR>  # ESP/AVR
```

`tools/flash.py` is one flashing abstraction over J-Link, UF2 drag-drop, and arduino-cli;
override the J-Link path with the `JLINK_EXE` env var. `tools/bin2uf2.py` converts raw
binaries only for its explicit nRF52840, RP2040, and RP2350-Arm family IDs.

### Cross-MCU, one command

```bash
bash tools/check_portability.sh   # builds the portable core for all 6 MCU families
bash tools/ci_matrix.sh           # host tests + portability + port builds + validators
bash tools/lint_gate.sh           # clippy -D warnings gate
```

### Editors and IDEs (all optional)

- **Any editor + rust-analyzer** (Neovim, Helix, Zed, Emacs, IntelliJ-Rust, VS Code).
- **No editor at all**: the CLI above is complete.
- **Arduino IDE / PlatformIO**: needed for ESP32/AVR Arduino applications (which run
  vendor firmware), via `arduino-cli` - not for the NobroRTOS Rust core.

### Config-driven, no Rust knowledge

```bash
python3 tools/nobro_app.py my_robot/app.json --gen main.rs
```

Describe the board + actuators + sensors in JSON; the builder validates against the device
catalog and generates the Rust. This is the on-ramp for beginners and for higher-level
front-ends (a GUI, a block editor, or another language) that emit the JSON.

### Other OS / SDK surfaces

- **Linux/macOS/Windows**: identical Cargo + Python flow; only the flashing backend differs.
- **CI**: `tools/ci_matrix.sh` is a single exit-coded gate for GitHub Actions / GitLab CI.
- **C / C++**: `bindings/c` + `bindings/cpp` expose the module ABI for non-Rust codebases.
- **Web flasher / PlatformIO packaging**: the SDK manifest (`sdk/sdk-manifest.json`,
  validated by `tools/check_sdk_manifest.py`) declares the Arduino + PlatformIO surfaces.
