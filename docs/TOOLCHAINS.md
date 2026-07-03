# NobroRTOS toolchains — IDE-optional, OS-agnostic

NobroRTOS is not tied to the Arduino IDE or VS Code. The core is plain Rust + Cargo, so it
builds and flashes from a terminal on Linux, macOS, or Windows with only `rustup`.

## Pure CLI (recommended, any OS)

```bash
rustup target add thumbv7em-none-eabihf          # or your board's target
cargo build -p kernel-selftest --release          # build firmware
python3 tools/flash.py jlink --bin app.bin --addr 0x1000   # J-Link (nRF)
python3 tools/flash.py uf2  --file app.uf2 --drive <DRIVE> # UF2 (Pico/nice!nano)
python3 tools/flash.py arduino --port <PORT> --fqbn <FQBN> --build-dir <DIR>  # ESP/AVR
```

`tools/flash.py` is one flashing abstraction over J-Link, UF2 drag-drop, and arduino-cli;
override the J-Link path with the `JLINK_EXE` env var. `tools/bin2uf2.py` converts a raw
`.bin` to a UF2 for any family (rp2350 / rp2040 / nrf52840).

## Cross-MCU, one command

```bash
bash tools/check_portability.sh   # builds the portable core for all 6 MCU families
bash tools/ci_matrix.sh           # host tests + portability + port builds + validators
bash tools/lint_gate.sh           # clippy -D warnings gate
```

## Editors and IDEs (all optional)

- **Any editor + rust-analyzer** (Neovim, Helix, Zed, Emacs, IntelliJ-Rust, VS Code).
- **No editor at all**: the CLI above is complete.
- **Arduino IDE / PlatformIO**: only needed for the ESP32/AVR *bench nodes* (which run
  vendor firmware), via `arduino-cli` — not for the NobroRTOS Rust core.

## Config-driven, no Rust knowledge

```bash
python3 tools/nobro_app.py my_robot/app.json --gen main.rs
```

Describe the board + actuators + sensors in JSON; the builder validates against the device
catalog and generates the Rust. This is the on-ramp for beginners and for higher-level
front-ends (a GUI, a block editor, or another language) that emit the JSON.

## Other OS / SDK surfaces

- **Linux/macOS/Windows**: identical Cargo + Python flow; only the flashing backend differs.
- **CI**: `tools/ci_matrix.sh` is a single exit-coded gate for GitHub Actions / GitLab CI.
- **C / C++**: `bindings/c` + `bindings/cpp` expose the module ABI for non-Rust codebases.
- **Web flasher / PlatformIO packaging**: the SDK manifest (`sdk/sdk-manifest.json`,
  validated by `tools/check_sdk_manifest.py`) declares the Arduino + PlatformIO surfaces.
