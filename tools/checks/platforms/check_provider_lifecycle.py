#!/usr/bin/env python3
"""Run the host lifecycle contract for every promoted feature backend."""

from __future__ import annotations

import argparse
import pathlib
import subprocess
import sys


ROOT = pathlib.Path(__file__).resolve().parents[3]
CORE = ROOT / "core"
PACKAGES = (
    "nobro-device",
    "nobro-adapter-wireless-ble-arduino-ble",
    "nobro-adapter-wireless-ble-arduino-esp",
    "nobro-adapter-wireless-wifi-arduino-esp",
    "nobro-adapter-wireless-wifi-arduino-wifis3",
    "nobro-adapter-sensors-esp32-adc-continuous",
    "nobro-adapter-audio-esp32s3-es8311",
    "nobro-adapter-camera-niuscam",
)


def run(command: list[str], *, cwd: pathlib.Path) -> None:
    print("+", " ".join(command))
    subprocess.run(command, cwd=cwd, check=True)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    args = parser.parse_args()

    run([sys.executable, "tools/checks/integrations/check_arduino_facade.py"], cwd=ROOT)
    cargo = ["cargo", "test", "--locked", "--target", args.target]
    for package in PACKAGES:
        cargo.extend(("-p", package))
    run(cargo, cwd=CORE)
    print(
        "PROVIDER LIFECYCLE: PASS "
        "(generation/cleanup receipts plus facade, BLE, WiFi, persistent ADC, audio, and camera backends)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
