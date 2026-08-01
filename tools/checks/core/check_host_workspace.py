#!/usr/bin/env python3
"""Fail-closed host validation for every package in the core workspace.

Every workspace package is explicitly classified below. Portable libraries are
tested and linted on rustc's native host. Firmware binaries remain target-only
because their startup, linker, interrupt, and peripheral contracts are not
meaningfully validated by a host stub. A newly added or renamed package makes
this command fail until its validation route is chosen deliberately.
"""

import argparse
import json
import os
import subprocess
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
CORE = os.path.join(ROOT, "core")

HOST_PACKAGES = (
    "nobro-adapter-audio-esp32s3-es8311",
    "nobro-adapter-bmp280",
    "nobro-adapter-camera-niuscam",
    "nobro-adapter-icm45686",
    "nobro-adapter-ina3221",
    "nobro-adapter-motion-ai",
    "nobro-adapter-mpu9250-imu",
    "nobro-adapter-nn-motion-ai",
    "nobro-adapter-radio-comms",
    "nobro-adapter-robo-servo",
    "nobro-adapter-ros-imu-bridge",
    "nobro-adapter-sensor-stub",
    "nobro-adapter-sensors-esp32-adc-continuous",
    "nobro-adapter-servo-esp32-ledc",
    "nobro-adapter-servo-esp32-rmt",
    "nobro-adapter-wireless-ble-arduino-ble",
    "nobro-adapter-wireless-ble-arduino-esp",
    "nobro-adapter-wireless-wifi-arduino-esp",
    "nobro-adapter-wireless-wifi-arduino-esp8266",
    "nobro-adapter-wireless-wifi-arduino-wifis3",
    "nobro-admission",
    "nobro-ai",
    "nobro-audio",
    "nobro-camera",
    "nobro-classic",
    "nobro-control",
    "nobro-crypto",
    "nobro-database",
    "nobro-device",
    "nobro-host",
    "nobro-imu",
    "nobro-kernel",
    "nobro-ml",
    "nobro-net",
    "nobro-nn",
    "nobro-power",
    "nobro-sal",
    "nobro-secure",
    "nobro-sensor",
    "nobro-services",
    "nobro-servo",
    "nobro-storage",
    "nobro-wireless",
    "portable-adapter-conformance",
)

# The value names the target gate responsible for the package. These exclusions
# are not waivers: target builds remain in tools/checks/ci_matrix.sh.
TARGET_ONLY = {
    "ai-imu-demo": "cross-MCU application target builds",
    "ai-runtime-demo": "cross-MCU application target builds",
    "ai-usb-demo": "nRF52840 USB application link builds",
    "async-exec-demo": "cross-MCU application target builds",
    "ble-adv-demo": "cross-MCU application target builds",
    "boot-slot-demo": "board boot slot adapter target build",
    "cc2530-gateway": "cross-MCU application target builds",
    "cc2530-link": "cross-MCU application target builds",
    "closed-loop-demo": "cross-MCU application target builds",
    "config-actuator-demo": "cross-MCU application target builds",
    "control-loop-demo": "cross-MCU application target builds",
    "crash-dump-demo": "cross-MCU application target builds",
    "db-persist-demo": "cross-MCU application target builds",
    "eh-imu-demo": "cross-MCU application target builds",
    "flash-log-demo": "cross-MCU application target builds",
    "gesture-demo": "cross-MCU application target builds",
    "imu-i2c-demo": "cross-MCU application target builds",
    "isolation-demo": "nRF52840 PSP/PendSV target build",
    "kv-store-demo": "cross-MCU application target builds",
    "lease-demo": "cross-MCU application target builds",
    "motion3-demo": "cross-MCU application target builds",
    "mpu-guard-demo": "cross-MCU application target builds",
    "mvk-ppi-timestamp": "cross-MCU application target builds",
    "nn-inference-demo": "cross-MCU application target builds",
    "nobro-c-abi-demo": "Tier-C prebuilt library and link",
    "nobro-c-abi-module": "Tier-C prebuilt library and link",
    "nobro-hal": "cross-MCU HAL target and provider lifecycle gates",
    "nobro-eh-i2c": "embedded-hal I2C target builds",
    "nobro-eh-spi": "embedded-hal SPI target builds",
    "nobro-usb": "USB backend host-feature and target-link gates",
    "nobro-tierc": "Tier-C prebuilt library and link",
    "nrf-usbd": "vendored nRF USBD regression and target-link gates",
    "pwm-bank-demo": "cross-MCU application target builds",
    "radio-comms-demo": "cross-MCU application target builds",
    "radio-link-demo": "cross-MCU application target builds",
    "recovery-demo": "cross-MCU application target builds",
    "ros-imu-demo": "cross-MCU application target builds",
    "rtc-sleep-demo": "cross-MCU application target builds",
    "spi-imu-demo": "cross-MCU application target builds",
    "stack-guard-demo": "cross-MCU application target builds",
    "supervision-demo": "cross-MCU application target builds",
    "telemetry-ring-demo": "cross-MCU application target builds",
    "tri-radio": "cross-MCU application target builds",
    "udi-imu-demo": "cross-MCU application target builds",
    "usb-cdc-demo": "nRF52840 USB application link builds",
    "usb-stack-demo": "cross-MCU application target builds",
    "watchdog-demo": "cross-MCU application target builds",
}


def host_target():
    override = os.environ.get("HOST_TARGET")
    if override:
        return override
    output = subprocess.check_output(
        ["rustc", "-vV"], text=True, encoding="utf-8", errors="replace"
    )
    return next(
        line.split(":", 1)[1].strip()
        for line in output.splitlines()
        if line.startswith("host:")
    )


def metadata():
    output = subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
        cwd=CORE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    return json.loads(output)


def validate_inventory(data):
    packages = {package["name"]: package for package in data["packages"]}
    host = set(HOST_PACKAGES)
    target = set(TARGET_ONLY)
    errors = []

    overlap = host & target
    if overlap:
        errors.append(f"classified as both host and target-only: {sorted(overlap)}")
    missing = set(packages) - host - target
    if missing:
        errors.append(f"unclassified workspace packages: {sorted(missing)}")
    stale = (host | target) - set(packages)
    if stale:
        errors.append(f"stale inventory entries: {sorted(stale)}")

    for name in host & set(packages):
        kinds = {
            kind
            for item in packages[name]["targets"]
            for kind in item.get("kind", [])
        }
        if not kinds & {"lib", "rlib", "staticlib"}:
            errors.append(f"host package has no library target: {name}")
    for name in target & set(packages):
        if not TARGET_ONLY[name].strip():
            errors.append(f"target-only package has no named gate: {name}")

    if errors:
        raise SystemExit("host workspace inventory failed:\n- " + "\n- ".join(errors))


def run_host_gate(command):
    result = subprocess.run(command, cwd=CORE)
    if result.returncode:
        raise SystemExit(result.returncode)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--inventory-only",
        action="store_true",
        help="validate classification without compiling",
    )
    parser.add_argument(
        "--target-only",
        action="store_true",
        help="check every target-only package for the supplied bare-metal target",
    )
    parser.add_argument(
        "--target",
        default="thumbv7em-none-eabihf",
        help="bare-metal target used with --target-only",
    )
    args = parser.parse_args()

    validate_inventory(metadata())
    if args.inventory_only:
        print(
            f"host workspace inventory: {len(HOST_PACKAGES)} host, "
            f"{len(TARGET_ONLY)} target-only"
        )
        return 0
    if args.target_only:
        selected = [item for name in TARGET_ONLY for item in ("-p", name)]
        run_host_gate(["cargo", "check", "--locked", "--target", args.target, *selected])
        print(
            f"target-only workspace: {len(TARGET_ONLY)} packages checked for {args.target}"
        )
        return 0

    target = host_target()
    selected = [item for name in HOST_PACKAGES for item in ("-p", name)]
    run_host_gate(["cargo", "test", "--locked", "--target", target, *selected])
    run_host_gate(
        [
            "cargo",
            "clippy",
            "--locked",
            "--no-deps",
            "--target",
            target,
            *selected,
            "--",
            "-D",
            "warnings",
        ]
    )
    print(
        f"host workspace: {len(HOST_PACKAGES)} packages tested/linted; "
        f"{len(TARGET_ONLY)} target-only packages explicitly gated"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
