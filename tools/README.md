# tools/ — developer & CI surface

Every file here has one job. Users normally drive these through the SDK command
(`python sdk/cli/nobro.py <cmd>`); the table is for contributors.

## Orchestrators

| Tool | Job |
| --- | --- |
| `run_checks.py` | THE gate suite → one `ALL PASS` + Evidence Pack |
| `nobro_verify.py` | Evidence Pack builder (gates + budgets → JSON/HTML) |
| `fleet_evidence.py` | fold software/OTA/hardware/replay evidence → fleet verdict |
| `ci_matrix.sh` | extended Rust build matrix (host tests, portability, ports) |
| `lint_gate.sh` | clippy `-D warnings` across portable crates + HAL |
| `nobro_project.py` | create/import/explain/build/simulate/flash/report project flow |
| `nobro_firmware_project.py` | generate an admitted workload and production nRF firmware from one short app declaration |
| `measure_authoring.py` | reproduce the scoped three-periodic-task authoring comparison |

## Gates (each also runs standalone)

`check_block_editor.py` · `check_board_profiles.py` · `check_portability.sh` ·
`check_async_miri.py` · `check_fuzz_targets.py` · `check_platform_tiers.py` ·
`check_ecosystem_matrix.py` ·
`check_release_versions.py` · `check_ros_bridge.py` · `check_sdk_manifest.py` ·
`check_udi.py` · `check_web_flasher.py` · `chaos_test.py` · `tutorial_runner.py` ·
`verify_timing_lease.py` · `wasm_slot_spike.py` · `ota_preflight_demo.py`

## Build / flash / package

| Tool | Job |
| --- | --- |
| `nobro_hw_eval.py` | build+flash+run+read+grade a hardware eval app (bootloader-safe) |
| `flash.py` | flash an image via jlink / uf2 / arduino backends |
| `bin2uf2.py` / `gen_memory_x.py` | image + linker-script utilities |
| `package_arduino.py` | Arduino library packaging + header drift gate (also syncs `sdk/include`) |
| `package_prebuilt_uf2.py` | the committed starter UF2 + its manifest gate |
| `build_libnobro.py` | Tier C `libnobro.a` bundle + gcc link gate |
| `sign_firmware.py` | measure + sign images (host side of SecureBoot) |
| `static_budget.py` | worst-case stack/RAM/flash/cycles from an ELF |

## Codegen / contracts

`nobro_app.py` (app.json → validate/generate) · `nobro_contract_tool.py` (the
contract multi-tool) · `ros_msg_gen.py` · `import_dts.py` · `gen_api_index.py`

## `dev/` — lab & maintainer material

Bench collectors, model-training pipelines, radio/vision/audio experiments, board
provisioning, the publish checklist, and `bench/` sketches. Nothing in `dev/` is
needed to *use* NobroRTOS; see `dev/` file headers for their individual stories.
Local-only files (`boards.json`, `leak_needles.local.txt`) stay gitignored.
