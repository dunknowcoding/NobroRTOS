#!/usr/bin/env python3
"""Compile every public Arduino package example for representative architectures."""

import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXAMPLES = os.path.join(ROOT, "packages", "arduino", "examples")
LIBRARY = os.path.join(ROOT, "packages", "arduino")
EXPECTED_EXAMPLES = (
    "BeginnerApp",
    "ProviderApp",
    "ReportReader",
    "RobotIoTApp",
)
FQBNS = [
    fqbn.strip()
    for fqbn in os.environ.get(
        "NOBRO_ARDUINO_FQBNS",
        "arduino:avr:uno,arduino:renesas_uno:unor4wifi,esp32:esp32:esp32s3,arduinonrf:nrf52:promicro_nrf52840:usbcdc=enabled",
    ).split(",")
]


def configuration_error(fqbns, examples):
    if not fqbns or any(not fqbn for fqbn in fqbns):
        return "at least one non-empty FQBN is required"
    missing = sorted(set(EXPECTED_EXAMPLES) - set(examples))
    unexpected = sorted(set(examples) - set(EXPECTED_EXAMPLES))
    if missing or unexpected:
        details = []
        if missing:
            details.append(f"missing={','.join(missing)}")
        if unexpected:
            details.append(f"unexpected={','.join(unexpected)}")
        return f"public example inventory mismatch ({'; '.join(details)})"
    return None


def main():
    try:
        examples = [path for path in sorted(os.listdir(EXAMPLES))
                    if os.path.isdir(os.path.join(EXAMPLES, path))]
    except OSError as error:
        print(f"ARDUINO COMPILE: FAIL (cannot inspect examples: {error})")
        return 1
    error = configuration_error(FQBNS, examples)
    if error:
        print(f"ARDUINO COMPILE: FAIL ({error})")
        return 1
    cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
    if not cli:
        print("ARDUINO COMPILE: FAIL (arduino-cli not found)")
        return 1
    failures = []
    for fqbn in FQBNS:
        for example in examples:
            result = subprocess.run(
                [cli, "compile", "--fqbn", fqbn, "--library", LIBRARY,
                 os.path.join(EXAMPLES, example)],
                cwd=ROOT, capture_output=True, text=True,
            )
            print(f"  {'PASS' if result.returncode == 0 else 'FAIL'} {fqbn} {example}")
            if result.returncode:
                failures.append((f"{fqbn} {example}",
                                 (result.stdout + result.stderr).splitlines()[-5:]))
    if failures:
        for fqbn, lines in failures:
            print(f"--- {fqbn} ---")
            print("\n".join(lines))
        print("ARDUINO COMPILE: FAIL")
        return 1
    print(f"ARDUINO COMPILE: PASS ({len(FQBNS)} architectures x {len(examples)} examples)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
