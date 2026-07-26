#!/usr/bin/env python3
"""Fail closed when hosted evidence silently regains moving dependencies."""

from __future__ import annotations

import pathlib
import re
import sys


ROOT = pathlib.Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github" / "workflows" / "gates.yml"
MATRIX = ROOT / "tools" / "ci_matrix.sh"

PINNED_ACTIONS = {
    "actions/checkout": "3d3c42e5aac5ba805825da76410c181273ba90b1",
    "actions/setup-python": "5fda3b95a4ea91299a34e894583c3862153e4b97",
    "actions/upload-artifact": "043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
    "taiki-e/install-action": "3d7d7cd5ac7f994c1892ae0c06165095b9139094",
}

REQUIRED_WORKFLOW_TOKENS = (
    "cancel-in-progress: true",
    "python-version: \"3.11.9\"",
    "rustup toolchain install 1.97.0",
    "nightly-2026-07-03",
    "commit-hash: 2d8144b7880597b6e6d3dfd63a9a9efae3f533d3",
    "commit-hash: c397dae808f70caebab1fc4e11b3edf7e59f58c7",
    "cargo-audit@0.22.2,cargo-llvm-cov@0.8.7",
    "fallback: none",
    "--fail-under-lines 90",
    "arduino:avr@1.8.8",
    "arduino:renesas_uno@1.6.0",
    "esp32:esp32@3.3.10",
    "arduinonrf:nrf52@0.3.11",
    "28a8e119c498a25607821c36cb2dc49e8463941b261a0d99091baa7bc692dd2b",
    "fabe42e0eb04d00e776a66178299ff95a46c623dbc260f997e58fd514853dd40",
    "dd8625b3742b2f74ce406286baef8ee67525d63b25ea303ddf7473ed2cc31192",
    "006c89337eced277fdf4c1c3bf3aebe55c85e8d52cba8d412009717fb781b959",
    "29638ff054fdbb83d2844240f7ef7e576cb52629",
    "python -O tools/check_platform_tiers.py --selftest",
)


def check(workflow: str, matrix: str) -> list[str]:
    errors: list[str] = []
    uses = re.findall(r"(?m)^\s*-\s+uses:\s+([^@\s]+)@([^\s#]+)", workflow)
    if not uses:
        errors.append("workflow has no third-party actions")
    for action, revision in uses:
        if not re.fullmatch(r"[0-9a-f]{40}", revision):
            errors.append(f"{action} must use an immutable 40-hex revision")
            continue
        expected = PINNED_ACTIONS.get(action)
        if expected is None:
            errors.append(f"unreviewed third-party action {action}")
        elif revision != expected:
            errors.append(f"{action} revision drift")
    for action in PINNED_ACTIONS:
        if not any(found == action for found, _revision in uses):
            errors.append(f"missing pinned action {action}")

    for token in REQUIRED_WORKFLOW_TOKENS:
        if token not in workflow:
            errors.append(f"missing hermetic workflow token {token!r}")
    if "dtolnay/rust-toolchain@" in workflow:
        errors.append("moving rust-toolchain action reference is forbidden")
    if re.search(r"arduino-cli core install arduino:(?:avr|renesas_uno)\s*$", workflow, re.M):
        errors.append("Arduino core installs must include exact versions")

    jobs_text = workflow.partition("jobs:")[2]
    job_starts = list(re.finditer(r"(?m)^  ([A-Za-z0-9_-]+):\s*$", jobs_text))
    for index, match in enumerate(job_starts):
        job = match.group(1)
        end = job_starts[index + 1].start() if index + 1 < len(job_starts) else len(jobs_text)
        body = jobs_text[match.end():end]
        if "runs-on:" in body and "timeout-minutes:" not in body:
            errors.append(f"job {job} has no timeout")
    for step in (
        "comprehensive Rust gates",
        "portable coverage",
        "Miri portable safety boundary",
        "full reproducible cross-MCU matrix",
        "compile public package examples (AVR, UNO R4, ESP32-S3)",
        "compile public package examples (ArduinoNRF)",
    ):
        start = workflow.find(f"- name: {step}")
        end_candidates = [
            position
            for marker in ("\n      - name:", "\n      - uses:")
            if (position := workflow.find(marker, start + 1)) >= 0
        ]
        end = min(end_candidates) if end_candidates else len(workflow)
        if start < 0 or "timeout-minutes:" not in workflow[start:end]:
            errors.append(f"long-running step {step!r} has no timeout")

    checkout_lines = re.findall(r"(?m)^\s*git -C \S+ checkout (.+)$", workflow)
    for arguments in checkout_lines:
        if re.fullmatch(r"--detach [0-9a-f]{40}", arguments.strip()) is None:
            errors.append("external checkout must be detached at an exact commit")
    if workflow.count(" checkout --detach ") != workflow.count(" rev-parse HEAD)"):
        errors.append("each external checkout must verify its resulting HEAD")

    strict_lint = (
        "cargo +esp clippy --locked --release --lib --bins -- -D warnings"
    )
    if strict_lint not in matrix:
        errors.append("ESP32-S3 first-party strict lint gate is missing")
    if "core/vendor/nrf-usbd" in matrix and "clippy" in matrix:
        errors.append("vendored nRF PAC must not be included in first-party strict lint")
    return errors


def selftest() -> int:
    workflow = WORKFLOW.read_text(encoding="utf-8")
    matrix = MATRIX.read_text(encoding="utf-8")
    if check(workflow, matrix):
        raise AssertionError(check(workflow, matrix))
    moving = workflow.replace(PINNED_ACTIONS["actions/checkout"], "v7")
    if not any("40-hex" in error for error in check(moving, matrix)):
        raise AssertionError("moving action reference was accepted")
    no_floor = workflow.replace("--fail-under-lines 90", "--fail-under-lines 0")
    if not any("fail-under-lines" in error for error in check(no_floor, matrix)):
        raise AssertionError("coverage floor drift was accepted")
    no_lint = matrix.replace(
        "cargo +esp clippy --locked --release --lib --bins -- -D warnings",
        "cargo +esp check",
    )
    if not any("strict lint" in error for error in check(workflow, no_lint)):
        raise AssertionError("missing strict lint was accepted")
    print("CI HERMETICITY SELFTEST: PASS")
    return 0


def main() -> int:
    errors = check(
        WORKFLOW.read_text(encoding="utf-8"),
        MATRIX.read_text(encoding="utf-8"),
    )
    if "--selftest" in sys.argv[1:]:
        return selftest()
    for error in errors:
        print(f"CI HERMETICITY: {error}")
    if errors:
        print("RESULT: FAIL")
        return 1
    print("CI HERMETICITY: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
