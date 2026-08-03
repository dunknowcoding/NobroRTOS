#!/usr/bin/env python3
"""Generate and execute the bounded W137 feature covering matrix."""

from __future__ import annotations

import argparse
from itertools import combinations, product
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
CORE = ROOT / "core"


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def pairwise(parameters: list[tuple[str, tuple[object, ...]]]) -> list[dict[str, object]]:
    """Deterministic greedy covering array for every pair of parameter values."""

    names = [name for name, _ in parameters]
    candidates = [dict(zip(names, values)) for values in product(*(values for _, values in parameters))]
    uncovered = {
        ((left, candidate[left]), (right, candidate[right]))
        for candidate in candidates
        for left, right in combinations(names, 2)
    }
    selected = []
    while uncovered:
        best = max(
            candidates,
            key=lambda candidate: sum(
                ((left, candidate[left]), (right, candidate[right])) in uncovered
                for left, right in combinations(names, 2)
            ),
        )
        covered = {
            ((left, best[left]), (right, best[right]))
            for left, right in combinations(names, 2)
        }
        require(bool(covered & uncovered), "covering generator made no progress")
        uncovered -= covered
        selected.append(best)
        candidates.remove(best)
    return selected


def assert_pairwise(parameters: list[tuple[str, tuple[object, ...]]], rows: list[dict[str, object]]) -> None:
    names = [name for name, _ in parameters]
    for left, right in combinations(names, 2):
        expected = set(product(dict(parameters)[left], dict(parameters)[right]))
        observed = {(row[left], row[right]) for row in rows}
        require(observed == expected, f"missing {left} x {right} coverage")


def run(command: list[str], *, should_pass: bool = True) -> None:
    result = subprocess.run(command, cwd=CORE, capture_output=True, text=True, encoding="utf-8", errors="replace")
    if (result.returncode == 0) != should_pass:
        tail = "\n".join(((result.stdout or "") + (result.stderr or "")).splitlines()[-20:])
        expectation = "pass" if should_pass else "fail closed"
        raise ValueError(f"expected {expectation}: {' '.join(command)}\n{tail}")


def kernel_cases() -> list[dict[str, object]]:
    parameters = [
        ("board", ("board-promicro-nosd", "board-promicro-s140")),
        ("preemptive", (False, True)),
        ("capacity", (False, True)),
        ("trace", (False, True)),
    ]
    rows = pairwise(parameters)
    assert_pairwise(parameters, rows)
    return rows


def execute() -> None:
    host = subprocess.check_output(["rustc", "-vV"], text=True, encoding="utf-8")
    host = next(line.split(":", 1)[1].strip() for line in host.splitlines() if line.startswith("host:"))
    for row in kernel_cases():
        features = [
            "nobro-kernel/portable-atomic-cs",
            "nobro-kernel/hal-profile",
            f"nobro-hal/{row['board']}",
        ]
        if row["preemptive"]:
            features.append("nobro-kernel/preemptive")
        if row["capacity"]:
            features.append("nobro-kernel/capacity-report")
        if row["trace"]:
            features.append("nobro-kernel/trace-hooks")
        run([
            "cargo", "check", "--locked", "--target", host,
            "-p", "nobro-kernel", "-p", "nobro-hal", "--no-default-features",
            "--features", ",".join(features),
        ])

    usb_backends = (
        "backend-nrf-usbd",
        "backend-ra-usbfs",
        "backend-usb-serial-jtag-esp32c3",
        "backend-usb-serial-jtag-esp32p4",
        "backend-usb-serial-jtag-esp32s3",
    )
    for backend, reports in product(usb_backends, (False, True)):
        features = [backend] + (["host-reports"] if reports else [])
        run([
            "cargo", "check", "--locked", "--target", host,
            "-p", "nobro-usb", "--no-default-features", "--features", ",".join(features),
        ])
    run([
        "cargo", "check", "--locked", "--target", host,
        "-p", "nobro-usb", "--no-default-features",
    ], should_pass=False)
    run([
        "cargo", "check", "--locked", "--target", host,
        "-p", "nobro-usb", "--no-default-features", "--features",
        "backend-ra-usbfs,backend-usb-serial-jtag-esp32c3",
    ], should_pass=False)

    for alloc, zigbee in product((False, True), repeat=2):
        features = []
        if alloc:
            features.append("alloc")
        if zigbee:
            features.append("zigbee-aps")
        command = [
            "cargo", "check", "--locked", "--target", host,
            "-p", "nobro-wireless", "--no-default-features",
        ]
        if features:
            command += ["--features", ",".join(features)]
        run(command)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    rows = kernel_cases()
    require(len(rows) < 16, "covering array was not reduced")
    if not args.selftest:
        execute()
    print(f"feature covering: PASS ({len(rows)} kernel rows, 10 USB rows, 4 wireless rows)")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"feature covering: FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
