#!/usr/bin/env python3
"""The NobroRTOS SDK command - one entry point for the user-facing tools.

    python sdk/cli/nobro.py <command> [args...]

| Command  | Does | Forwards to |
| -------- | ---- | ----------- |
| app      | validate / generate firmware from an app.json | tools/cli/project/ |
| flash    | flash an image (jlink / uf2 / arduino)        | tools/cli/firmware/ |
| budget   | price worst-case stack/RAM/flash of an ELF    | tools/cli/analysis/ |
| sign     | measure + sign a firmware image               | tools/cli/firmware/ |
| package  | build the Arduino zip / prebuilt UF2 / Tier C | tools/release/, tools/build/ |
| contract | inspect / decode host contracts               | tools/cli/learning/ |
| project  | create/explain/build/run/report/shrink apps   | tools/cli/project/ |
| firmware | generate exact native/Arduino firmware routes from one declaration | tools/cli/project/ |
| adapter  | scaffold and register a bounded adapter              | tools/cli/project/ |
| admit    | analyze workload admission and shedding              | tools/cli/analysis/ |
| shrink   | propose identity-bound capacity reductions           | tools/cli/analysis/ |
| verify-timing | model-check timing and lease invariants          | tools/cli/analysis/ |
| import-dts | import a bounded DeviceTree board profile           | tools/cli/interop/ |
| ros-msg  | generate a bounded ROS message bridge                 | tools/cli/interop/ |
| tutorials | validate the public tutorial ladder                  | tools/cli/learning/ |

Each command accepts its underlying tool's flags unchanged. The mapping is data, so
adding a command is one table row.
"""
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools"

COMMANDS = {
    "app": "cli/project/nobro_app.py",
    "flash": "cli/firmware/flash.py",
    "budget": "cli/analysis/static_budget.py",
    "sign": "cli/firmware/sign_firmware.py",
    "contract": "cli/learning/nobro_contract_tool.py",
    "project": "cli/project/nobro_project.py",
    "firmware": "cli/project/nobro_firmware_project.py",
    "adapter": "cli/project/nobro_adapter.py",
    "admit": "cli/analysis/nobro_admission.py",
    "shrink": "cli/analysis/nobro_shrink.py",
    "verify-timing": "cli/analysis/verify_timing_lease.py",
    "import-dts": "cli/interop/import_dts.py",
    "ros-msg": "cli/interop/ros_msg_gen.py",
    "tutorials": "cli/learning/tutorial_runner.py",
}
PACKAGE_KINDS = {
    "arduino": "release/package_arduino.py",
    "platformio": "release/package_platformio.py",
    "uf2": "release/package_prebuilt_uf2.py",
    "tierc": "build/build_libnobro.py",
}


def usage() -> int:
    print(__doc__.strip())
    return 2


def main() -> int:
    if len(sys.argv) < 2 or sys.argv[1] in ("-h", "--help", "help"):
        return usage()
    cmd, rest = sys.argv[1], sys.argv[2:]

    if cmd == "package":
        if not rest or rest[0] not in PACKAGE_KINDS:
            print(f"nobro package <{'|'.join(PACKAGE_KINDS)}> [tool flags]")
            return 2
        script, rest = PACKAGE_KINDS[rest[0]], rest[1:]
    elif cmd in COMMANDS:
        script = COMMANDS[cmd]
    else:
        print(f"unknown command '{cmd}'\n")
        return usage()

    return subprocess.run([sys.executable, str(TOOLS / script), *rest], cwd=ROOT).returncode


if __name__ == "__main__":
    sys.exit(main())
