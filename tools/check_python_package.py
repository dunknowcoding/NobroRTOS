#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Build and import the NobroRTOS Core Python wheel in a temporary directory."""

from __future__ import annotations

import importlib
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import zipfile


ROOT = Path(__file__).resolve().parents[1]


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="nobro-core-wheel-") as temporary:
        output = Path(temporary)
        source = output / "source"
        (source / "docs").mkdir(parents=True)
        (source / "tools").mkdir()
        for relative in ("pyproject.toml", "LICENSE"):
            shutil.copy2(ROOT / relative, source / relative)
        shutil.copy2(ROOT / "docs" / "PYTHON.md", source / "docs" / "PYTHON.md")
        shutil.copy2(ROOT / "tools" / "nobro_core.py", source / "tools" / "nobro_core.py")
        shutil.copytree(ROOT / "tools" / "nobro_rtos", source / "tools" / "nobro_rtos")
        completed = subprocess.run(
            [
                sys.executable,
                "-m",
                "pip",
                "wheel",
                str(source),
                "--no-deps",
                "--wheel-dir",
                str(output),
            ],
            cwd=ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            check=False,
        )
        if completed.returncode != 0:
            print(completed.stdout, end="", file=sys.stderr)
            print("PYTHON PACKAGE: FAIL (wheel build)", file=sys.stderr)
            return 1
        wheels = list(output.glob("nobro_rtos-1.0.1-*.whl"))
        if len(wheels) != 1:
            print("PYTHON PACKAGE: FAIL (unexpected wheel identity)", file=sys.stderr)
            return 1
        wheel = wheels[0]
        with zipfile.ZipFile(wheel) as archive:
            names = set(archive.namelist())
            required = {
                "nobro_core.py",
                "nobro_rtos/__init__.py",
                "nobro_rtos/__main__.py",
            }
            if not required.issubset(names):
                print("PYTHON PACKAGE: FAIL (public modules missing)", file=sys.stderr)
                return 1
            entry_points = [
                name for name in names if name.endswith(".dist-info/entry_points.txt")
            ]
            if len(entry_points) != 1:
                print("PYTHON PACKAGE: FAIL (entry points missing)", file=sys.stderr)
                return 1
            commands = archive.read(entry_points[0]).decode("utf-8")
            if ("nobro = nobro_rtos:main" not in commands or
                    "nobro-core = nobro_core:main" not in commands):
                print("PYTHON PACKAGE: FAIL (command surface drift)", file=sys.stderr)
                return 1
        sys.path.insert(0, str(wheel))
        core = importlib.import_module("nobro_core")
        package = importlib.import_module("nobro_rtos")
        if package.__version__ != "1.0.1" or package.SCHEMA != core.SCHEMA:
            print("PYTHON PACKAGE: FAIL (compatibility entry point)", file=sys.stderr)
            return 1
        if package.selftest() != 0:
            print("PYTHON PACKAGE: FAIL (wheel selftest)", file=sys.stderr)
            return 1
        wheel_name = wheel.name
    print(f"PYTHON PACKAGE: PASS ({wheel_name})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
