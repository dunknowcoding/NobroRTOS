#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Run the same pinned public Core command manifest locally and on GitHub."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "ci" / "core-matrix.json"


def run(name: str, command: list[str]) -> None:
    print(f"[RUN] {name}", flush=True)
    completed = subprocess.run(command, cwd=ROOT, check=False)
    if completed.returncode != 0:
        raise RuntimeError(f"{name} exited {completed.returncode}")
    print(f"[ OK] {name}", flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--with-sdcc", action="store_true")
    args = parser.parse_args()
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    if manifest.get("schema") != 1:
        print("CORE CI: FAIL (unknown command-manifest schema)", file=sys.stderr)
        return 1
    toolchain = manifest["rust_toolchain"]
    gates = {
        "public-boundary": [sys.executable, "tools/check_public_boundary.py"],
        "package-metadata": [sys.executable, "tools/check_package_metadata.py"],
        "python-package": [sys.executable, "tools/check_python_package.py"],
        "byte-host": [
            sys.executable,
            "tools/check_byte_core.py",
            *(["--require-sdcc"] if args.with_sdcc else []),
        ],
        "arduino-host": [sys.executable, "tools/check_arduino_core.py"],
        "rust-format": ["cargo", f"+{toolchain}", "fmt", "--all", "--", "--check"],
        "rust-test": ["cargo", f"+{toolchain}", "test", "--workspace", "--all-targets"],
        "rust-clippy": [
            "cargo",
            f"+{toolchain}",
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        "rust-package": [
            "cargo",
            f"+{toolchain}",
            "package",
            "-p",
            "nobro-rtos-core",
            "--allow-dirty",
            "--no-verify",
        ],
    }
    try:
        for gate in manifest["common_gates"]:
            if gate == "rust-targets":
                for target in manifest["rust_targets"]:
                    run(
                        f"rust-target-{target}",
                        [
                            "cargo",
                            f"+{toolchain}",
                            "check",
                            "-p",
                            "nobro-rtos-core",
                            "--target",
                            target,
                        ],
                    )
            else:
                run(gate, gates[gate])
    except (KeyError, OSError, RuntimeError) as error:
        print(f"CORE CI: FAIL ({error})", file=sys.stderr)
        return 1
    print("CORE CI: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
