#!/usr/bin/env python3
"""Measure default-stable, tuned-stable, and nightly-static Embassy variants.

Nightly absence is an explicit ``unavailable`` result, not a hidden row. Stable
variants remain required. Output is ignored evidence under ``_work``.
"""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
DIRECTORY = ROOT / "core" / "baselines" / "embassy-complex"
OUT = ROOT / "_work" / "evidence" / "embassy-variants.json"
TARGET = "thumbv7em-none-eabihf"


def build(label: str, toolchain: str | None, features: tuple[str, ...]) -> dict:
    from measure_baselines import elf_sizes

    target_dir = ROOT / "_work" / "embassy-variants" / label
    cargo = ["cargo"] if toolchain is None else ["cargo", f"+{toolchain}"]
    command = cargo + ["build", "--release"]
    if features:
        command += ["--features", ",".join(features)]
    env = dict(os.environ, CARGO_TARGET_DIR=str(target_dir))
    completed = subprocess.run(
        command, cwd=DIRECTORY, env=env, capture_output=True, text=True
    )
    if completed.returncode:
        return {"failed": True, "diagnostic": completed.stderr[-1200:]}
    elf = target_dir / TARGET / "release" / "embassy-complex"
    return elf_sizes(elf)


def nightly_available() -> bool:
    if shutil.which("rustup") is None:
        return False
    listed = subprocess.run(
        ["rustup", "toolchain", "list"], capture_output=True, text=True
    ).stdout
    target = subprocess.run(
        ["rustup", "target", "list", "--toolchain", "nightly", "--installed"],
        capture_output=True, text=True,
    )
    return "nightly" in listed and TARGET in target.stdout


def selftest() -> int:
    unavailable = {"status": "unavailable", "reason": "nightly_or_target_missing"}
    assert unavailable["status"] != "skipped"
    assert TARGET.startswith("thumbv7em")
    print("EMBASSY VARIANTS SELFTEST: PASS")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    results = {
        "default-stable": build("default-stable", None, ()),
        "tuned-stable-1024": build("tuned-stable-1024", None, ("arena-1024",)),
    }
    if nightly_available():
        results["nightly-static"] = build("nightly-static", "nightly", ("nightly-static",))
    else:
        results["nightly-static"] = {
            "status": "unavailable",
            "reason": "nightly_or_target_missing",
        }
    failures = [name for name, result in results.items() if result.get("failed")]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps({"target": TARGET, "results": results}, indent=2), encoding="utf-8")
    print(json.dumps(results, indent=2))
    print(f"evidence: {OUT}")
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
