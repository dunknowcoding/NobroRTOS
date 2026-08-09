#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Verify that Cargo, Arduino, and PlatformIO describe one public release."""

from __future__ import annotations

import json
from pathlib import Path
import sys
import tomllib


ROOT = Path(__file__).resolve().parents[1]


def properties(path: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#"):
            continue
        key, separator, value = line.partition("=")
        if not separator:
            raise ValueError(f"malformed property: {line}")
        result[key] = value
    return result


def main() -> int:
    failures: list[str] = []
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    version = cargo["workspace"]["package"]["version"]
    arduino = properties(ROOT / "library.properties")
    platformio = json.loads((ROOT / "library.json").read_text(encoding="utf-8"))
    python_package = tomllib.loads(
        (ROOT / "pyproject.toml").read_text(encoding="utf-8")
    )
    if version != "1.0.0":
        failures.append(f"Cargo version is {version}, expected 1.0.0")
    if arduino.get("version") != version:
        failures.append("Arduino version differs from Cargo")
    if platformio.get("version") != version:
        failures.append("PlatformIO version differs from Cargo")
    if python_package.get("project", {}).get("version") != version:
        failures.append("Python version differs from Cargo")
    if arduino.get("architectures") != "*":
        failures.append("Arduino architecture scope must remain explicit")
    if arduino.get("includes") != "NobroRTOSCore.h":
        failures.append("Arduino public header is not exact")
    if platformio.get("headers") != "NobroRTOSCore.h":
        failures.append("PlatformIO public header is not exact")
    if platformio.get("license") != "GPL-3.0-only":
        failures.append("PlatformIO license is not GPL-3.0-only")
    required = (
        ROOT / "src" / "NobroRTOSCore.h",
        ROOT / "examples" / "BlinkCore" / "BlinkCore.ino",
        ROOT / "tools" / "nobro_core.py",
        ROOT / "LICENSE",
        ROOT / "README.md",
    )
    for path in required:
        if not path.is_file():
            failures.append(f"missing package file: {path.relative_to(ROOT)}")
    if failures:
        print("PACKAGE METADATA: FAIL")
        for failure in failures:
            print(f"- {failure}")
        return 1
    print(
        f"PACKAGE METADATA: PASS "
        f"(v{version}, Arduino + PlatformIO + Python + Cargo)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
