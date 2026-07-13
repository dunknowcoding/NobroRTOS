# tools/ — user and contributor utilities

Every tracked file here has one job. Maintainer comparisons, lab configurations,
fuzz corpora, and private bench utilities are not stored in this repository.

## Orchestrators

| Tool | Job |
| --- | --- |
| `run_checks.py` | THE gate suite → one `ALL PASS` + Evidence Pack |
| `nobro_verify.py` | Evidence Pack builder (public gates + budgets → JSON/HTML) |
| `fleet_evidence.py` | fold software/OTA/hardware/replay evidence → fleet verdict |
| `ci_matrix.sh` | extended Rust build matrix (host tests, portability, ports) |
| `lint_gate.sh` | clippy `-D warnings` across portable crates + HAL |
| `nobro_project.py` | create/import/explain/build/simulate/flash/report project flow |
| `nobro_firmware_project.py` | generate an admitted workload and production nRF firmware from one short app declaration |
| `check_arduino_facade.py` | compile/run positive and negative allocation-free C++ facade contracts |

## Gates (each also runs standalone)

`check_block_editor.py` · `check_board_profiles.py` · `check_portability.sh` ·
`check_async_miri.py` · `check_platform_tiers.py` ·
`check_ecosystem_matrix.py` ·
`check_release_versions.py` · `check_ros_bridge.py` · `check_sdk_manifest.py` ·
`check_udi.py` · `check_web_flasher.py` · `chaos_test.py` · `tutorial_runner.py` ·
`verify_timing_lease.py` · `ota_preflight_demo.py`

## Build / flash / package

| Tool | Job |
| --- | --- |
| `flash.py` | flash an image via jlink / uf2 / arduino backends |
| `firmware_image.py` | extract guarded application bytes and build an nRF52840 UF2 |
| `bin2uf2.py` / `gen_memory_x.py` | image + linker-script utilities |
| `package_arduino.py` | Arduino library packaging + header drift gate (also syncs `sdk/include`) |
| `package_prebuilt_uf2.py` | the committed starter UF2 + its manifest gate |
| `build_libnobro.py` | Tier C `libnobro.a` bundle + gcc link gate |
| `sign_firmware.py` | measure + sign images (host side of SecureBoot) |
| `static_budget.py` | worst-case stack/RAM/flash/cycles from an ELF |

## Codegen / contracts

`nobro_app.py` (app.json → validate/generate) · `nobro_contract_tool.py` (the
contract multi-tool) · `ros_msg_gen.py` · `import_dts.py` · `gen_api_index.py`
