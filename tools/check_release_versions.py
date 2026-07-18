#!/usr/bin/env python3
"""Release-version alignment gate.

Every distribution surface and the core workspace must carry the SAME semver
before anything ships: core/Cargo.toml, packages/arduino/library.properties,
packages/platformio/library.json, and bindings/python/pyproject.toml.
Publishing itself is owner-gated (accounts, irreversible); this gate only
guarantees the artifacts agree with each other.

    python tools/check_release_versions.py             # equality check (CI gate)
    python tools/check_release_versions.py --release   # also refuse -dev/0.0.0
"""
import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--release", action="store_true",
                    help="additionally refuse dev/zero versions")
    args = ap.parse_args()

    versions = {}
    core = (ROOT / "core" / "Cargo.toml").read_text(encoding="utf-8")
    workspace = core.split("[workspace.package]", 1)[1]
    versions["core"] = re.search(
        r'^version\s*=\s*"([^"]+)"', workspace, re.M
    ).group(1)
    props = (ROOT / "packages/arduino/library.properties").read_text(encoding="utf-8")
    versions["arduino"] = re.search(r"^version=(.+)$", props, re.M).group(1).strip()
    pio = json.loads((ROOT / "packages/platformio/library.json").read_text(encoding="utf-8"))
    versions["platformio"] = pio["version"]
    py = (ROOT / "bindings/python/pyproject.toml").read_text(encoding="utf-8")
    versions["python"] = re.search(r'^version\s*=\s*"([^"]+)"', py, re.M).group(1)

    for name, v in versions.items():
        print(f"  {name:11} {v}")
    errors = []
    if len(set(versions.values())) != 1:
        errors.append(f"version mismatch across surfaces: {versions}")
    if args.release:
        v = next(iter(versions.values()))
        if "dev" in v or v.startswith("0.0.0"):
            errors.append(f"{v} is not a releasable version")
        if not re.fullmatch(r"\d+\.\d+\.\d+", v):
            errors.append(f"{v} is not plain semver (Library Manager requires x.y.z)")
        changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
        if f"## {v} " not in changelog and f"## {v}\n" not in changelog:
            errors.append(f"CHANGELOG.md has no release section for {v}")
    for e in errors:
        print("  !", e)
    print(f"RESULT: {'PASS' if not errors else 'FAIL'}")
    return 0 if not errors else 1


if __name__ == "__main__":
    sys.exit(main())
