#!/usr/bin/env python3
"""Validate exact market-board intake and optionally build admitted targets."""

from __future__ import annotations

import argparse
import copy
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
from urllib.parse import urlparse


ROOT = pathlib.Path(__file__).resolve().parents[3]
REGISTRY = ROOT / "core" / "boards" / "candidate_families.json"
EXAMPLE = ROOT / "packages" / "arduino" / "examples" / "BeginnerApp"
LIBRARY = ROOT / "packages" / "arduino"

EXPECTED_BOARDS = {
    "avr128db48-curiosity-nano",
    "attiny3227-curiosity-nano",
    "ch32v003f4p6-evt-r0",
    "nucleo-c031c6",
    "nucleo-g071rb",
    "nucleo-g474re",
    "esp32-c6-devkitm-1",
    "esp32-h2-devkitm-1",
    "nrf54l15-dk",
    "nucleo-h563zi",
    "nucleo-h743zi2",
    "nucleo-n657x0-q",
    "teensy-4-1",
    "arduino-giga-r1-wifi",
    "esp32-p4-function-ev-board",
}
EXPECTED_COMPILED = {
    "esp32-c6-devkitm-1": ("esp32:esp32", "3.3.10", "esp32:esp32:esp32c6"),
    "esp32-h2-devkitm-1": ("esp32:esp32", "3.3.10", "esp32:esp32:esp32h2"),
}
PRIMARY_DOMAINS = {
    "docs.arduino.cc",
    "docs.espressif.com",
    "docs.nordicsemi.com",
    "www.microchip.com",
    "www.pjrc.com",
    "www.st.com",
    "www.wch-ic.com",
}
ALLOWED_STATES = {"discovered", "cataloged", "target-compiled"}
ALLOWED_BANDS = {"low", "mid", "high"}
GATE = "arduino-market-intake-target-build"


def load_registry() -> dict:
    return json.loads(REGISTRY.read_text(encoding="utf-8"))


def sorted_unique_strings(value: object) -> bool:
    return (
        isinstance(value, list)
        and bool(value)
        and all(isinstance(item, str) and item for item in value)
        and value == sorted(set(value))
    )


def validate(registry: dict) -> list[str]:
    errors: list[str] = []
    if registry.get("schema") != "nobro-candidate-families-v2":
        errors.append("candidate registry schema drift")
    states = registry.get("intake_states")
    if not isinstance(states, dict) or set(states) != ALLOWED_STATES:
        errors.append("intake state vocabulary drift")
    if not sorted_unique_strings(registry.get("selection_principles")):
        errors.append("selection principles must be sorted unique strings")
    decisions = registry.get("selection_decisions")
    if not isinstance(decisions, list) or len(decisions) < 4:
        errors.append("selection decisions are incomplete")

    rows = registry.get("market_boards")
    if not isinstance(rows, list):
        return errors + ["market_boards must be a list"]
    by_id = {row.get("id"): row for row in rows if isinstance(row, dict)}
    if len(by_id) != len(rows):
        errors.append("market board ids are missing or duplicated")
    if set(by_id) != EXPECTED_BOARDS:
        errors.append(
            "market board inventory drift: "
            f"missing={sorted(EXPECTED_BOARDS - set(by_id))} "
            f"extra={sorted(set(by_id) - EXPECTED_BOARDS)}"
        )

    compiled: set[str] = set()
    for board_id, row in by_id.items():
        prefix = f"market_boards.{board_id}"
        state = row.get("state")
        if state not in ALLOWED_STATES:
            errors.append(f"{prefix}: invalid state")
        if row.get("band") not in ALLOWED_BANDS:
            errors.append(f"{prefix}: invalid candidate band")
        for field in ("vendor", "board", "soc", "architecture", "boot_recovery"):
            if not isinstance(row.get(field), str) or not row[field].strip():
                errors.append(f"{prefix}: missing {field}")
        if not sorted_unique_strings(row.get("nonduplicative_value")):
            errors.append(f"{prefix}: nonduplicative value must be sorted and unique")
        reference = row.get("official_reference")
        parsed = urlparse(reference) if isinstance(reference, str) else None
        if (
            parsed is None
            or parsed.scheme != "https"
            or parsed.hostname not in PRIMARY_DOMAINS
        ):
            errors.append(f"{prefix}: official reference is not an allowlisted primary source")
        upstream = row.get("upstream")
        if not isinstance(upstream, dict):
            errors.append(f"{prefix}: missing upstream route")
            continue
        for field in ("name", "url", "framework", "version", "fqbn"):
            if not isinstance(upstream.get(field), str) or not upstream[field]:
                errors.append(f"{prefix}.upstream: missing {field}")
        if not str(upstream.get("url", "")).startswith("https://"):
            errors.append(f"{prefix}.upstream: source must use HTTPS")
        if row.get("support_claims") != []:
            errors.append(f"{prefix}: candidate improperly claims support")

        compile_gate = row.get("compile_gate")
        if state == "target-compiled":
            compiled.add(board_id)
            expected = EXPECTED_COMPILED.get(board_id)
            if expected is None:
                errors.append(f"{prefix}: compiled state lacks an expected exact route")
            elif (
                upstream.get("name") != "arduino-esp32"
                or upstream.get("version") != expected[1]
                or upstream.get("fqbn") != expected[2]
                or compile_gate != GATE
            ):
                errors.append(f"{prefix}: exact compiled route drift")
        elif compile_gate is not None:
            errors.append(f"{prefix}: non-compiled candidate cannot name a compile gate")
    if compiled != set(EXPECTED_COMPILED):
        errors.append(f"compiled candidate set drift: {sorted(compiled)}")
    return errors


def compile_targets(registry: dict) -> list[str]:
    cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
    if cli is None:
        return ["arduino-cli is required for admitted candidate target builds"]
    core_result = subprocess.run(
        [cli, "core", "list"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if core_result.returncode != 0:
        return ["cannot inspect installed Arduino cores"]
    if not re.search(r"(?m)^esp32:esp32\s+3\.3\.10(?:\s|$)", core_result.stdout):
        return ["exact esp32:esp32 core 3.3.10 is not installed"]

    rows = {row["id"]: row for row in registry["market_boards"]}
    errors: list[str] = []
    with tempfile.TemporaryDirectory(prefix="nobro-market-intake-") as temporary:
        root = pathlib.Path(temporary)
        for board_id in sorted(EXPECTED_COMPILED):
            fqbn = rows[board_id]["upstream"]["fqbn"]
            build_path = root / board_id
            result = subprocess.run(
                [
                    cli,
                    "compile",
                    "--fqbn",
                    fqbn,
                    "--build-path",
                    str(build_path),
                    "--library",
                    str(LIBRARY),
                    str(EXAMPLE),
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
            output = result.stdout + result.stderr
            flash = re.search(r"Sketch uses\s+([0-9]+)\s+bytes", output)
            ram = re.search(r"Global variables use\s+([0-9]+)\s+bytes", output)
            if result.returncode != 0 or flash is None or ram is None:
                tail = " | ".join(output.splitlines()[-6:])
                errors.append(f"{board_id}: exact target build failed ({tail})")
                continue
            print(
                f"  PASS {board_id} fqbn={fqbn} "
                f"flash={flash.group(1)} static_ram={ram.group(1)}"
            )
    return errors


def selftest(registry: dict) -> None:
    if validate(registry):
        raise RuntimeError("canonical market-board registry does not validate")
    broken = copy.deepcopy(registry)
    broken["market_boards"][0]["support_claims"] = ["gpio"]
    if not any("improperly claims support" in item for item in validate(broken)):
        raise RuntimeError("candidate support-inflation negative did not fail")
    broken = copy.deepcopy(registry)
    broken["market_boards"][0]["official_reference"] = "https://example.com/board"
    if not any("allowlisted primary source" in item for item in validate(broken)):
        raise RuntimeError("primary-source negative did not fail")
    broken = copy.deepcopy(registry)
    next(
        row
        for row in broken["market_boards"]
        if row["id"] == "esp32-c6-devkitm-1"
    )["compile_gate"] = None
    if not any("exact compiled route drift" in item for item in validate(broken)):
        raise RuntimeError("compiled-route negative did not fail")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--compile", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    try:
        registry = load_registry()
        errors = validate(registry)
        if args.selftest:
            selftest(registry)
        if args.compile and not errors:
            errors.extend(compile_targets(registry))
    except (OSError, ValueError, RuntimeError) as error:
        errors = [str(error)]
    if errors:
        for error in errors:
            print(f"MARKET BOARD INTAKE: FAIL ({error})")
        return 1
    suffix = " + exact target builds" if args.compile else ""
    print(f"MARKET BOARD INTAKE: PASS (15 exact candidates{suffix}; no support claims)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
