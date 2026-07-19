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
revision, zero-disabled behavior, the ESP32-S3 isolated build delta, and its
configuration-specific registry/report binding. Physical codec, speaker,
microphone, timing, and vendor-runtime memory evidence remains a separate
private, state-restoring campaign.

`check_esp32_peripheral_facade.py` executes continuous-ADC, LEDC, and RMT
failure/lifecycle contracts against deterministic fakes.
`check_esp32_peripheral_integrations.py` proves exact Arduino-ESP32 family
target builds, target-specific persistent-ADC flash/static-RAM prices, and a
zero-cost disabled composition, including calibration symbols. Physical
runtime and coexistence measurements remain state-restoring private inputs to
the exact registry price; the public gate does not claim absolute ADC
accuracy.

`check_wifis3_integrations.py` compiles the categorized UNO R4 WiFiS3
association facade against the board package, proves an identical
zero-disabled baseline, and locks its exact binding to the explicit
`unmeasured` price state. It does not convert compilation into physical
association, socket, or vendor-resource evidence.

`check_arduino_esp_wifi.py` pins Arduino-ESP32 3.3.10 provenance, compiles the
optional WiFi facade on ESP32, ESP32-C3, and ESP32-S3, proves the disabled C3
composition is byte-identical to baseline, scans its link map for forbidden
vendor symbols, and gates the complete price of one exact C3 workload.
