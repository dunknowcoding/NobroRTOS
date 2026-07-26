# CLI tools

These are public SDK command implementations. Prefer
`python sdk/cli/nobro.py <command>` for the stable user-facing surface.

| Role | Commands |
| --- | --- |
| Project authoring | `nobro_project.py`, `nobro_app.py`, `nobro_adapter.py`, `nobro_firmware_project.py` |
| Firmware operations | `flash.py`, `sign_firmware.py` |
| Analysis | `nobro_admission.py`, `nobro_diagnostics.py`, `nobro_shrink.py`, `static_budget.py`, `verify_timing_lease.py` |
| Interoperability | `import_dts.py`, `ros_msg_gen.py` |
| Contracts and learning | `nobro_contract_tool.py`, `tutorial_runner.py` |

Standalone utilities that are not yet dispatcher commands remain directly
invocable from this directory, including ROS message generation, DeviceTree
import, admission analysis, and tutorial validation.
