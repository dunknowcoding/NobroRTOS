#!/usr/bin/env python3
"""Ensure maintainer comparisons and lab material cannot enter user packages."""

import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "sdk" / "sdk-manifest.json"
FORBIDDEN_REFERENCES = (
    "tools/internal/",
    "core/baselines/",
    "measure_baselines.py",
    "measure_complex_runtime.py",
    "measure_embassy_variants.py",
    "measure_authoring.py",
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
        pathlib.PurePosixPath("core/baselines"),
        pathlib.PurePosixPath("tools/internal"),
        pathlib.PurePosixPath("tools/dev"),
        pathlib.PurePosixPath("_work"),
    }
    if not required_excludes <= excludes:
        errors.append(f"missing release excludes: {sorted(map(str, required_excludes-excludes))}")
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
    comparison = ROOT / "tools" / "internal" / "comparison"
    expected = {
        "measure_authoring.py", "measure_baselines.py", "measure_complex_runtime.py",
        "measure_embassy_variants.py", "baseline_budgets.json",
    }
    actual = {item.name for item in comparison.iterdir() if item.is_file()}
    if actual != expected:
        errors.append(f"internal comparison inventory drift: {sorted(actual)}")
    if list((ROOT / "tools").glob("measure_*.py")):
        errors.append("comparison tools must not live on the public tools root")
    for surface in [ROOT / "packages", ROOT / "sdk" / "cli"]:
        for path in surface.rglob("*"):
            if not path.is_file() or path.suffix.lower() in {".png", ".jpg", ".uf2", ".zip"}:
                continue
            try:
                text = path.read_text(encoding="utf-8")
            except UnicodeDecodeError:
                continue
            for token in FORBIDDEN_REFERENCES:
                if token in text:
                    errors.append(f"{path.relative_to(ROOT)} exposes internal comparison token {token}")
    return errors


def main() -> int:
    errors = validate()
    for error in errors:
        print(f"RELEASE BOUNDARY: {error}")
    print(f"RELEASE BOUNDARY: {'PASS' if not errors else 'FAIL'}")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
