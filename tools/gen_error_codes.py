#!/usr/bin/env python3
"""Generate the public NobroRTOS error index and reject diagnostic drift."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
REGISTRY = ROOT / "sdk" / "error-codes.json"
OUTPUT = ROOT / "docs" / "ERROR_CODES.md"
CODE = re.compile(r"^NOBRO-E([0-9]{3})$")
KEY = re.compile(r"^[a-z][a-z0-9-]*$")


def load() -> list[dict[str, str]]:
    document = json.loads(REGISTRY.read_text(encoding="utf-8"))
    if document.get("schema") != "nobro-error-codes-v1":
        raise ValueError("registry schema must be nobro-error-codes-v1")
    entries = document.get("codes")
    if not isinstance(entries, list) or not entries:
        raise ValueError("registry codes must be a non-empty array")
    required = {"code", "key", "surface", "message", "recovery"}
    codes: set[str] = set()
    pairs: set[tuple[str, str]] = set()
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict) or set(entry) != required:
            raise ValueError(f"entry {index} fields differ from the v1 schema")
        if not all(isinstance(entry[field], str) for field in required):
            raise ValueError(f"entry {index} fields must be strings")
        code = entry["code"]
        pair = (entry["surface"], entry["key"])
        if not CODE.fullmatch(code):
            raise ValueError(f"invalid code {code!r}")
        if not KEY.fullmatch(entry["key"]):
            raise ValueError(f"invalid key {entry['key']!r}")
        if entry["surface"] not in {"admission", "project", "app"}:
            raise ValueError(f"invalid surface {entry['surface']!r}")
        if code in codes or pair in pairs:
            raise ValueError(f"duplicate diagnostic identity at {code}")
        if not entry["message"].endswith(".") or not entry["recovery"].endswith("."):
            raise ValueError(f"{code} message and recovery must be sentences")
        codes.add(code)
        pairs.add(pair)
    numbers = [int(CODE.fullmatch(entry["code"]).group(1)) for entry in entries]
    if numbers != sorted(numbers):
        raise ValueError("registry codes must be numerically ordered")
    return entries


def render(entries: list[dict[str, str]]) -> str:
    lines = [
        "# Error codes",
        "",
        "This index is generated from `sdk/error-codes.json`. The code and first",
        "sentence are stable across bindings; details may add the failing task, field,",
        "observed value, or limit. Fix the first reported error, then run the command again.",
        "",
        "| Code | Surface | Meaning | First recovery step |",
        "|---|---|---|---|",
    ]
    lines.extend(
        f"| `{item['code']}` | {item['surface']} | {item['message']} | {item['recovery']} |"
        for item in entries
    )
    lines.append("")
    return "\n".join(lines)


def parity_errors(entries: list[dict[str, str]]) -> list[str]:
    errors: list[str] = []
    by_surface = {
        surface: {
            item["key"]: (item["code"], item["message"])
            for item in entries
            if item["surface"] == surface
        }
        for surface in ("admission", "project", "app")
    }

    rust = (ROOT / "core/crates/nobro_admission/src/lib.rs").read_text(encoding="utf-8")
    for code, message in by_surface["admission"].values():
        if f'"{code} {message}"' not in rust:
            errors.append(f"Rust admission diagnostic differs for {code}")

    sys.path.insert(0, str(ROOT / "tools"))
    import nobro_diagnostics  # noqa: PLC0415

    if nobro_diagnostics.surface("project") != by_surface["project"]:
        errors.append("project diagnostic table differs from registry")

    sys.path.insert(0, str(ROOT / "bindings/python"))
    from nobro_rtos.diagnostics import APP_DIAGNOSTICS  # noqa: PLC0415

    if APP_DIAGNOSTICS != by_surface["app"]:
        errors.append("Python app diagnostic table differs from registry")

    c_header = (ROOT / "bindings/c/include/nobro_app.h").read_text(encoding="utf-8")
    rust_app = (
        (ROOT / "core/crates/nobro_kernel/src/graph.rs").read_text(encoding="utf-8")
        + (ROOT / "core/crates/nobro_kernel/src/c_app.rs").read_text(encoding="utf-8")
    )
    cpp_header = (ROOT / "bindings/cpp/include/nobro_app.hpp").read_text(encoding="utf-8")
    arduino = (ROOT / "packages/arduino/src/NobroRTOS.h").read_text(encoding="utf-8")
    if '#include "nobro_app.h"' not in cpp_header:
        errors.append("C++11 facade no longer inherits the C diagnostic renderer")
    for key in (
        "app-state", "app-name", "app-period", "app-task-capacity",
        "app-wire-capacity", "app-endpoint", "app-duplicate-task",
        "app-options", "app-admission", "app-step", "app-duplicate-wire",
        "app-self-wire",
    ):
        code, message = by_surface["app"][key]
        if code not in rust_app or message not in rust_app:
            errors.append(f"Rust app diagnostic differs for {code}")
    for key in (
        "app-state", "app-name", "app-period", "app-task-capacity",
        "app-wire-capacity", "app-endpoint", "app-duplicate-task",
        "app-options", "app-admission", "app-step",
    ):
        code, message = by_surface["app"][key]
        if code not in c_header or message not in c_header:
            errors.append(f"C app diagnostic differs for {code}")
    for key in (
        "app-name", "app-period", "app-task-capacity", "app-wire-capacity",
        "app-endpoint", "app-duplicate-task", "app-options", "app-admission",
        "app-duplicate-wire", "app-self-wire",
    ):
        code, message = by_surface["app"][key]
        if code not in arduino or message not in arduino:
            errors.append(f"Arduino app diagnostic differs for {code}")

    manifest = json.loads((ROOT / "sdk/sdk-manifest.json").read_text(encoding="utf-8"))
    if manifest.get("error_code_registry") != "sdk/error-codes.json":
        errors.append("SDK manifest does not publish the error-code registry")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    try:
        entries = load()
        expected = render(entries)
        errors = parity_errors(entries)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"ERROR CODE INDEX: FAIL ({error})")
        return 1
    if args.check:
        actual = OUTPUT.read_text(encoding="utf-8") if OUTPUT.is_file() else ""
        if actual != expected:
            errors.append("docs/ERROR_CODES.md is stale; run tools/gen_error_codes.py")
    else:
        OUTPUT.write_text(expected, encoding="utf-8", newline="\n")
    for error in errors:
        print(f"FAIL: {error}")
    print(
        f"ERROR CODE INDEX: {'PASS' if not errors else 'FAIL'} "
        f"({len(entries)} stable codes)"
    )
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
