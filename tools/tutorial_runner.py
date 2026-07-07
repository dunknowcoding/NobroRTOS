#!/usr/bin/env python3
"""Validate public NobroRTOS tutorial assets without creating build outputs."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
TOOLS = ROOT / "tools"
if str(TOOLS) not in sys.path:
    sys.path.insert(0, str(TOOLS))

import nobro_app  # noqa: E402
import verify_timing_lease  # noqa: E402


def require(path: Path) -> None:
    if not path.exists():
        raise FileNotFoundError(str(path.relative_to(ROOT)))


def check_book() -> dict:
    book = ROOT / "docs" / "book" / "README.md"
    require(book)
    text = book.read_text(encoding="utf-8")
    chapters = [
        "01-contracts-first.md",
        "02-local-validation.md",
        "03-build-a-device-app.md",
        "04-ai-robot-iot.md",
        "05-diagnostics-recovery.md",
    ]
    missing = []
    for chapter in chapters:
        chapter_path = book.parent / chapter
        if not chapter_path.exists() or chapter not in text:
            missing.append(chapter)
    return {"passing": not missing, "missing": missing, "chapters": len(chapters)}


def check_hello_device() -> dict:
    app_path = ROOT / "tutorials" / "hello-device" / "app.json"
    require(app_path)
    app = json.loads(app_path.read_text(encoding="utf-8"))
    errors = nobro_app.validate(app)
    skeleton = nobro_app.generate_rust(app)
    passing = not errors and "SERVO_SG90" in skeleton and "sensor imu" in skeleton
    return {
        "passing": passing,
        "errors": errors,
        "skeleton_lines": len(skeleton.splitlines()),
    }


def check_verifier() -> dict:
    args = SimpleNamespace(
        resources=2,
        owners=2,
        depth=4,
        tolerance_us=2,
        jitter_span_us=3,
    )
    result = verify_timing_lease.run(args)
    return {
        "passing": bool(result["passing"]),
        "lease_transitions": result["lease"]["transitions_checked"],
        "timing_sequences": result["timing"]["sequences_checked"],
    }


def run() -> dict:
    checks = {
        "book": check_book(),
        "hello_device": check_hello_device(),
        "verifier": check_verifier(),
    }
    return {
        "passing": all(check["passing"] for check in checks.values()),
        "checks": checks,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--json", action="store_true", help="print machine-readable JSON")
    args = parser.parse_args()

    result = run()
    if args.json:
        print(json.dumps(result, indent=2, sort_keys=True))
    else:
        for name, check in result["checks"].items():
            print(f"{name}: {'PASS' if check['passing'] else 'FAIL'}")
        print(f"RESULT: {'PASS' if result['passing'] else 'FAIL'}")
    return 0 if result["passing"] else 1


if __name__ == "__main__":
    sys.exit(main())
