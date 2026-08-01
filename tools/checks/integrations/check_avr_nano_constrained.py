#!/usr/bin/env python3
"""Verify the exact ATmega328P Nano constrained composition and candidate truth."""

from __future__ import annotations

import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile


ROOT = pathlib.Path(__file__).resolve().parents[3]
PACKAGE = ROOT / "packages" / "arduino"
BOARD = ROOT / "core" / "boards" / "avr" / "nano-v3-atmega328p" / "board.json"
CANDIDATES = ROOT / "core" / "boards" / "candidate_families.json"
MATRIX = ROOT / "core" / "boards" / "platform_tiers.json"
ARDUINO_HEADER = PACKAGE / "src" / "NobroAvrNano.h"
PLATFORMIO_HEADER = ROOT / "packages" / "platformio" / "include" / "NobroAvrNano.h"
FQBN = "arduino:avr:nano:cpu=atmega328old"

FULL = r'''#include <NobroAvrNano.h>
nobro::AvrNanoApp app;
nobro::AvrNanoDeadline deadline;
nobro::AvrNanoGpio gpio(13);
nobro::AvrNanoInterrupt irq(2);
nobro::AvrNanoPwm pwm(3);
nobro::AvrNanoAdc adc(A0);
nobro::AvrNanoI2c i2c;
nobro::AvrNanoSpi spi(10);
nobro::AvrNanoUart uart;
volatile bool exercise = false;
void callback() {}
void setup() {
  nobro::TaskId a = app.sensor("sense", 10);
  nobro::TaskId b = app.control("control", 20);
  nobro::TaskId c = app.service("report", 100);
  nobro::TaskId d = app.service("health", 500);
  app.wire(a, b).wire(b, c).wire(d, c);
  if (!app.admit() || !exercise) return;
  bool level = false;
  uint16_t sample = 0;
  uint8_t tx[1] = {0}, rx[1] = {0};
  deadline.armAfterUs(1000); deadline.due(); deadline.cancel();
  gpio.begin(OUTPUT); gpio.write(true); gpio.read(level);
  irq.attach(callback, CHANGE); irq.detach();
  pwm.begin(); pwm.setDuty(128);
  adc.begin(); adc.read(sample);
  i2c.begin(); i2c.probe(0x68);
  spi.begin(); spi.transfer(tx, rx, 1);
  uart.begin(); uart.write(tx, 1); uart.read(rx, 1);
  nobro::AvrNanoPower::cooperativeIdle();
  if (exercise) nobro::AvrNanoReset::restartApplication();
}
void loop() {}
'''

NEGATIVE = r'''#include <NobroAvrNano.h>
void setup() {}
void loop() {}
'''


def run(command: list[str], *, expect_success: bool = True) -> str:
    completed = subprocess.run(
        command,
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    output = completed.stdout + completed.stderr
    if (completed.returncode == 0) != expect_success:
        raise RuntimeError(output.strip() or f"unexpected exit for {command!r}")
    return output


def make_sketch(root: pathlib.Path, name: str, source: str) -> pathlib.Path:
    path = root / name
    path.mkdir()
    (path / f"{name}.ino").write_text(source, encoding="utf-8")
    return path


def validate_metadata() -> None:
    board = json.loads(BOARD.read_text(encoding="utf-8"))
    generation = board.get("firmware_generation", {})
    if (generation.get("support") != "arduino-composition" or
            generation.get("fqbn") != FQBN or
            generation.get("header") != "packages/arduino/src/NobroAvrNano.h"):
        raise RuntimeError("Nano firmware-generation contract drift")
    if board.get("boot", {}).get("app_flash_len_bytes") != 30720:
        raise RuntimeError("Nano Optiboot application boundary drift")
    if board.get("capacity") != {
        "flash_budget_bytes": 16384,
        "ram_budget_bytes": 1024,
        "sample_pool_slots": 2,
        "max_modules": 4,
    }:
        raise RuntimeError("Nano constrained capacity drift")

    matrix = json.loads(MATRIX.read_text(encoding="utf-8"))
    platform = matrix.get("platforms", {}).get("atmega328p", {})
    if platform.get("tier") != "constrained":
        raise RuntimeError("Nano must be an explicit constrained composition")
    claims = set(platform.get("compositions", {}).get("arduino", {}).get("claims", {}))
    required = {
        "timebase", "deadline", "gpio", "irq", "uart", "byte_io",
        "adc", "pwm", "i2c", "spi", "reset", "power",
    }
    if claims != required:
        raise RuntimeError(f"Nano claim set drift: {sorted(claims)}")
    limitations = platform.get("limitations", "").lower()
    for excluded in ("native usb", "multicore", "memory isolation", "dma"):
        if excluded not in limitations:
            raise RuntimeError(f"missing explicit Nano limitation: {excluded}")
    if "application re-entry only" not in limitations or "watchdog" not in limitations:
        raise RuntimeError("Nano reset must remain an application-only contract")

    if ARDUINO_HEADER.read_bytes() != PLATFORMIO_HEADER.read_bytes():
        raise RuntimeError("Arduino/PlatformIO Nano headers drift")


def validate_candidates() -> None:
    registry = json.loads(CANDIDATES.read_text(encoding="utf-8"))
    if registry.get("schema") != "nobro-candidate-families-v1":
        raise RuntimeError("candidate-family schema drift")
    families = {entry.get("id"): entry for entry in registry.get("families", [])}
    required = {"avr-dx", "attiny-modern", "ch32v003", "stm32c0", "mcs51"}
    if not required.issubset(families):
        raise RuntimeError("candidate-family inventory drift")
    for name, entry in families.items():
        if (entry.get("state") not in {"candidate-not-supported", "feasibility-only"}
                or entry.get("claims") != []):
            raise RuntimeError(f"{name} improperly claims support")
        contract = entry.get("compile_contract", {})
        if (contract.get("exact_device_required") is not True or
                contract.get("exact_core_version_required") is not True):
            raise RuntimeError(f"{name} lacks an exact compile intake contract")
        upstream = entry.get("upstream", {})
        if not str(upstream.get("url", "")).startswith("https://"):
            raise RuntimeError(f"{name} lacks primary upstream provenance")
        if name in {"avr-dx", "attiny-modern"} and contract.get("status") != (
                "not-installed-not-run"):
            raise RuntimeError(f"{name} must remain an unrun candidate")
    if families["mcs51"].get("compile_contract", {}).get("status") != (
            "no-maintained-rust-target-admitted"):
        raise RuntimeError("8051 feasibility boundary drift")


def resource_usage(output: str) -> tuple[int, int]:
    flash = re.search(r"Sketch uses\s+(\d+)\s+bytes", output)
    ram = re.search(r"Global variables use\s+(\d+)\s+bytes", output)
    if not flash or not ram:
        raise RuntimeError("Arduino CLI did not report Nano flash/RAM use")
    return int(flash.group(1)), int(ram.group(1))


def main() -> int:
    try:
        validate_metadata()
        validate_candidates()
        cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
        if not cli:
            raise RuntimeError("arduino-cli not found")
        with tempfile.TemporaryDirectory(prefix="nobro-avr-nano-") as temporary:
            root = pathlib.Path(temporary)
            full = make_sketch(root, "full", FULL)
            output = run([
                cli, "compile", "--fqbn", FQBN, "--library", str(PACKAGE), str(full)
            ])
            flash, ram = resource_usage(output)
            if flash > 16384 or ram > 1024:
                raise RuntimeError(
                    f"four-task Nano envelope exceeds 16384/1024: {flash}/{ram}")
            print(f"  PASS {FQBN} full ({flash} flash, {ram} static RAM)")

            negative = make_sketch(root, "negative", NEGATIVE)
            failure = run([
                cli, "compile", "--fqbn", "arduino:avr:mega",
                "--library", str(PACKAGE), str(negative)
            ], expect_success=False)
            if "requires an Arduino ATmega328P board package" not in failure:
                raise RuntimeError("cross-MCU include did not fail closed")
            print("  PASS architecture guard")
    except (OSError, RuntimeError, ValueError) as error:
        print(f"AVR NANO CONSTRAINED: FAIL ({error})")
        return 1
    print("AVR NANO CONSTRAINED: PASS (exact Nano profile, four-task/four-wire "
          "envelope, bounded providers, candidate-only low-end intake)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
