#!/usr/bin/env python3
"""Ensure maintainer comparisons and lab material cannot enter user packages."""

import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "sdk" / "sdk-manifest.json"
FORBIDDEN_REFERENCES = (
    "tools/internal/",
    "tools/dev/",
    "core/baselines/",
    "core/fuzz/",
    "core/internal/",
    "measure_baselines.py",
    "measure_complex_runtime.py",
    "measure_embassy_variants.py",
    "measure_authoring.py",
    "nobro_hw_eval.py",
    "hil_matrix.py",
    "wasm_slot_spike.py",
    "kernel_wcet_demo",
    "kernel_selftest",
    "hil_fault_demo",
)


def overlaps(left: pathlib.PurePosixPath, right: pathlib.PurePosixPath) -> bool:
    return left == right or left in right.parents or right in left.parents


def validate() -> list[str]:
    errors: list[str] = []
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    public_tools = set(manifest.get("host_tools", []))
    excludes = {pathlib.PurePosixPath(item) for item in manifest.get("release_excludes", [])}
    includes = {pathlib.PurePosixPath(item) for item in manifest.get("core_distribution_roots", [])}
    required_excludes = {
        pathlib.PurePosixPath("_work"),
        pathlib.PurePosixPath("_maintainer"),
        pathlib.PurePosixPath("core/baselines"),
        pathlib.PurePosixPath("core/fuzz"),
        pathlib.PurePosixPath("core/internal"),
        pathlib.PurePosixPath("tools/internal"),
        pathlib.PurePosixPath("tools/dev"),
    }
    if not required_excludes <= excludes:
        errors.append(f"missing release excludes: {sorted(map(str, required_excludes-excludes))}")
    forbidden_tracked = (
        "_maintainer/", "core/baselines/", "core/fuzz/", "core/internal/",
        "tools/internal/", "tools/dev/", "docs/ENGINEERING.md",
        "tools/nobro_hw_eval.py", "tools/hil_matrix.py", "tools/wasm_slot_spike.py",
        "core/apps/kernel/kernel_wcet_demo/", "core/apps/kernel/kernel_selftest/",
        "core/apps/kernel/hil_fault_demo/",
    )
    tracked = subprocess.run(
        ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout.splitlines()
    leaked = [
        path for path in tracked
        if path.startswith(forbidden_tracked) and (ROOT / path).exists()
    ]
    if leaked:
        errors.append(f"maintainer-only files are tracked: {leaked[:5]}")
    for public in map(pathlib.PurePosixPath, public_tools):
        if any(overlaps(public, excluded) for excluded in excludes):
            errors.append(f"public tool overlaps excluded path: {public}")
        if not (ROOT / public).is_file():
            errors.append(f"public tool is missing: {public}")
    for included in includes:
        if any(overlaps(included, excluded) for excluded in excludes):
            errors.append(f"core distribution root overlaps excluded path: {included}")
        if not (ROOT / included).exists():
            errors.append(f"core distribution root is missing: {included}")
    if list((ROOT / "tools").glob("measure_*.py")):
        errors.append("comparison tools must not live on the public tools root")
    workflow = (ROOT / ".github" / "workflows" / "gates.yml").read_text(encoding="utf-8")
    for name in ("measure_authoring.py", "measure_baselines.py", "measure_complex_runtime.py", "measure_embassy_variants.py"):
        if f"tools/{name}" in workflow:
            errors.append(f"hosted workflow uses stale public comparison path: tools/{name}")
    if "arduino-cli core install arduinonrf:nrf52" in workflow:
        errors.append("hosted Linux workflow cannot install the Windows-only ArduinoNRF toolchain")
    for relative in tracked:
        path = ROOT / relative
        if relative in {
            ".gitignore", "sdk/sdk-manifest.json", "tools/check_release_boundary.py"
        } or not path.is_file():
            continue
        if path.suffix.lower() in {".png", ".jpg", ".uf2", ".zip"}:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for token in FORBIDDEN_REFERENCES:
            if token in text:
                errors.append(f"{relative} exposes maintainer-only token {token}")
    return errors


def main() -> int:
    errors = validate()
    for error in errors:
        print(f"RELEASE BOUNDARY: {error}")
    print(f"RELEASE BOUNDARY: {'PASS' if not errors else 'FAIL'}")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
