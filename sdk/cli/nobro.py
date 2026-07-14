#!/usr/bin/env python3
"""The NobroRTOS SDK command - one entry point for the user-facing tools.

    python sdk/cli/nobro.py <command> [args...]

| Command  | Does | Forwards to |
| -------- | ---- | ----------- |
| app      | validate / generate firmware from an app.json | tools/nobro_app.py |
| flash    | flash an image (jlink / uf2 / arduino)        | tools/flash.py |
| budget   | price worst-case stack/RAM/flash of an ELF    | tools/static_budget.py |
| sign     | measure + sign a firmware image               | tools/sign_firmware.py |
| package  | build the Arduino zip / prebuilt UF2 / Tier C | tools/package_*.py, build_libnobro.py |
| contract | inspect / decode host contracts               | tools/nobro_contract_tool.py |
| project  | create/explain/build/run/report/shrink apps   | tools/nobro_project.py |
| firmware | generate/build nRF firmware from app.nobro   | tools/nobro_firmware_project.py |

Each command accepts its underlying tool's flags unchanged. The mapping is data, so
adding a command is one table row.
"""
import runpy
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools"

COMMANDS = {
    "app": "nobro_app.py",
    "flash": "flash.py",
    "budget": "static_budget.py",
    "sign": "sign_firmware.py",
    "contract": "nobro_contract_tool.py",
    "project": "nobro_project.py",
    "firmware": "nobro_firmware_project.py",
}
PACKAGE_KINDS = {
    "arduino": "package_arduino.py",
    "uf2": "package_prebuilt_uf2.py",
    "tierc": "build_libnobro.py",
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
