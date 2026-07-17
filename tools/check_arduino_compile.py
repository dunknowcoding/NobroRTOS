#!/usr/bin/env python3
"""Compile every public Arduino package example for representative architectures."""

import os
import shutil
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXAMPLES = os.path.join(ROOT, "packages", "arduino", "examples")
LIBRARY = os.path.join(ROOT, "packages", "arduino")
EXPECTED_EXAMPLES = (
    "BeginnerApp",
    "ProviderApp",
    "ReportReader",
    "RobotIoTApp",
)
DEFAULT_FQBNS = (
    "arduino:avr:uno,arduino:renesas_uno:unor4wifi,esp32:esp32:esp32s3,"
    "arduinonrf:nrf52:promicro_nrf52840:usbcdc=enabled"
)


def split_fqbns(value):
    """Split board lists without breaking Arduino option lists.

    Arduino FQBNs use commas inside the optional board-option suffix, for
    example ``pkg:arch:board:bootloader=nicenano,usbcdc=enabled``.  The CI
    environment historically also used commas between boards, so a plain
    ``str.split(",")`` incorrectly turns the second option into a fake FQBN.

    Keep the legacy comma-separated board list, but treat comma tokens without
    the required ``package:architecture:board`` prefix as continuations of the
    previous FQBN.  Semicolons are also accepted as an unambiguous separator for
    future multi-option lists.
    """
    groups = value.split(";") if ";" in value else value.split(",")
    fqbns = []
    for token in (part.strip() for part in groups):
        if not token:
            continue
        if token.count(":") >= 2 or not fqbns:
            fqbns.append(token)
        else:
            fqbns[-1] = f"{fqbns[-1]},{token}"
    return fqbns


FQBNS = split_fqbns(os.environ.get("NOBRO_ARDUINO_FQBNS", DEFAULT_FQBNS))
DEFAULT_ATTEMPTS = 3


def compile_attempts():
    raw = os.environ.get("NOBRO_ARDUINO_COMPILE_ATTEMPTS", str(DEFAULT_ATTEMPTS))
    try:
        attempts = int(raw)
    except ValueError:
        return None
    return attempts if 1 <= attempts <= 5 else None


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
    attempts = compile_attempts()
    if attempts is None:
        print("ARDUINO COMPILE: FAIL "
              "(NOBRO_ARDUINO_COMPILE_ATTEMPTS must be an integer from 1 to 5)")
        return 1
    failures = []
    for fqbn in FQBNS:
        for example in examples:
            diagnostics = []
            for attempt in range(1, attempts + 1):
                result = subprocess.run(
                    [cli, "compile", "--fqbn", fqbn, "--library", LIBRARY,
                     os.path.join(EXAMPLES, example)],
                    cwd=ROOT, capture_output=True, text=True,
                )
                if result.returncode == 0:
                    break
                diagnostics.append(
                    f"attempt {attempt}/{attempts}: "
                    + "\n".join((result.stdout + result.stderr).splitlines()[-5:])
                )
                if attempt < attempts:
                    print(f"  RETRY {fqbn} {example} ({attempt}/{attempts})")
                    time.sleep(1)
            print(f"  {'PASS' if result.returncode == 0 else 'FAIL'} {fqbn} {example}")
            if result.returncode:
                failures.append((f"{fqbn} {example}", diagnostics))
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
