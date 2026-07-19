# Tools

This directory contains portable utilities used by the SDK and reproducible hosted
checks. Machine-specific hardware automation, raw logs, comparison programs, fuzz
corpora, and private reports are intentionally not tracked.

## User utilities

| Tool | Purpose |
| --- | --- |
| `nobro_project.py` | Create, explain, build, simulate, and report a Nobro project |
| `nobro_adapter.py` | Scaffold categorized adapters and optional board-feature backends |
| `nobro_firmware_project.py` | Generate admitted native firmware from `app.nobro` or strict Python-app JSON |
| `nobro_app.py` | Validate and generate an `app.json` application |
| `nobro_contract_tool.py` | Inspect and decode public host contracts |
| `flash.py` | Flash through supported J-Link, UF2, or Arduino backends |
| `static_budget.py` | Report stack, RAM, flash, and static cycle bounds from an ELF |
| `sign_firmware.py` | Measure and sign a firmware image |
| `ros_msg_gen.py` | Generate bounded Rust records from ROS message definitions |
| `import_dts.py` | Convert a limited DeviceTree input into a board-profile draft |
| `check_nano_kernel.py` | Build-time admission, executable-section, L0 size, and absent-symbol gate |

## Build and package utilities

`build_libnobro.py`, `bin2uf2.py`, `firmware_image.py`, `gen_memory_x.py`,
`package_arduino.py`, `package_platformio.py`, and `package_prebuilt_uf2.py`
produce public SDK artifacts.

## Reproducible checks

`run_checks.py` runs the portable source/package suite. The narrower `check_*.py`,
`ci_matrix.sh`, and `lint_gate.sh` entry points are kept small so contributors and
hosted CI can run the same checks without private configuration.

Board-feature integrations remain evidence scoped. For example,
`check_audio_facade.py` executes the bounded audio lifecycle against a fake
transport, while `check_audio_integrations.py` verifies an exact NiusAudio
revision and the enabled/disabled ESP32-S3 build delta. Neither check represents
physical codec, speaker, microphone, timing, or vendor-runtime memory evidence.

`check_esp32_peripheral_facade.py` executes continuous-ADC, LEDC, and RMT
failure/lifecycle contracts against deterministic fakes.
`check_esp32_peripheral_integrations.py` proves exact Arduino-ESP32 family
target builds plus a zero-cost disabled composition. These remain software and
target-build evidence, not ADC accuracy, waveform, loopback, or vendor-runtime
memory evidence.
