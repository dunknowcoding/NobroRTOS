#!/usr/bin/env python3
"""Compile every public Arduino package example for representative architectures."""

import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXAMPLES = os.path.join(ROOT, "packages", "arduino", "examples")
LIBRARY = os.path.join(ROOT, "packages", "arduino")
FQBNS = os.environ.get(
    "NOBRO_ARDUINO_FQBNS",
    "arduino:avr:uno,arduino:renesas_uno:unor4wifi,esp32:esp32:esp32s3,arduinonrf:nrf52:promicro_nrf52840",
).split(",")


def main():
    cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
    if not cli:
        print("ARDUINO COMPILE: FAIL (arduino-cli not found)")
        return 1
    failures = []
    examples = [path for path in sorted(os.listdir(EXAMPLES))
                if os.path.isdir(os.path.join(EXAMPLES, path))]
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
