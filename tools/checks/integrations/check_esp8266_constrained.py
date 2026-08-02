#!/usr/bin/env python3
"""Verify the exact ESP8266 constrained composition and its fail-closed bounds."""

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
BOARD = ROOT / "core" / "boards" / "esp8266" / "wemos-d1-mini" / "board.json"
MATRIX = ROOT / "core" / "boards" / "platform_tiers.json"
FQBN = "esp8266:esp8266:d1_mini"

FULL = r'''#include <NobroEsp8266.h>
#include <NobroArduinoEsp8266WiFi.h>
nobro::Esp8266Deadline deadline;
nobro::Esp8266Gpio gpio(2);
nobro::Esp8266Interrupt irq(4);
nobro::Esp8266Pwm pwm(14);
nobro::Esp8266Adc adc(A0);
nobro::Esp8266I2c i2c;
nobro::Esp8266Spi spi(15);
nobro::Esp8266Uart uart;
nobro::ArduinoEsp8266WiFiStack wifi;
nobro::ArduinoEsp8266Network network;
volatile bool exercise = false;
void callback() {}
void setup() {
  if (!exercise) return;
  bool level = false;
  uint16_t sample = 0;
  uint8_t tx[1] = {0}, rx[1] = {0};
  size_t written = 0;
  nobro_wifi_network_t scans[1] = {};
  nobro_network_mount_receipt_t receipt = {};
  deadline.armAfterUs(1000); deadline.due(); deadline.cancel();
  gpio.begin(OUTPUT); gpio.write(true); gpio.read(level); gpio.isBootStrap();
  irq.attach(callback, CHANGE); irq.detach();
  pwm.begin(1000); pwm.setDuty(512); pwm.maxDuty();
  adc.read(sample); adc.maxSample();
  i2c.begin(); i2c.probe(0x40);
  spi.begin(); spi.transfer(tx, rx, 1);
  uart.begin(); uart.write(tx, 1); uart.read(rx, 1);
  wifi.mount(); wifi.beginScan(); wifi.pollScan(scans, 1, written);
  wifi.poll(); wifi.leave(); wifi.quiesce(); wifi.recover();
  network.mount(0, receipt); network.quiesce();
  nobro::Esp8266Power::cooperativeIdle();
  if (false) { nobro::Esp8266Power::deepSleepReset(1000000); nobro::Esp8266Reset::restart(); }
}
void loop() {}
'''

DISABLED = r'''#define NOBRO_ESP8266_WIFI_DISABLED 1
#include <NobroArduinoEsp8266WiFi.h>
#include <NobroRTOS.h>
nobro::NobroApp<1, 0> app;
void setup() { Serial.begin(115200); Serial.println(app.admit()); }
void loop() {}
'''

NEGATIVE = r'''#include <NobroEsp8266.h>
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


def sketch(root: pathlib.Path, name: str, source: str) -> pathlib.Path:
    path = root / name
    path.mkdir()
    (path / f"{name}.ino").write_text(source, encoding="utf-8")
    return path


def compile_case(cli: str, root: pathlib.Path, name: str, source: str) -> str:
    path = sketch(root, name, source)
    output = run([
        cli, "compile", "--fqbn", FQBN, "--library", str(PACKAGE), str(path)
    ])
    print(f"  PASS {FQBN} {name}")
    return output


def validate_metadata() -> None:
    board = json.loads(BOARD.read_text(encoding="utf-8"))
    expected_boot = {
        "layout": "Esp8266Arduino4M2M",
        "app_flash_start": 0,
        "app_flash_len_bytes": 1044464,
        "ram_start": "0x3FFE8000",
        "ram_len_bytes": 81920,
    }
    if board.get("boot") != expected_boot:
        raise RuntimeError("D1 Mini 4M2M memory profile drift")
    generation = board.get("firmware_generation", {})
    if generation.get("support") != "arduino-composition" or generation.get("fqbn") != FQBN:
        raise RuntimeError("D1 Mini Arduino composition is not selected exactly")
    matrix = json.loads(MATRIX.read_text(encoding="utf-8"))
    platform = matrix.get("platforms", {}).get("esp8266", {})
    if platform.get("tier") != "constrained":
        raise RuntimeError("ESP8266 must remain explicitly constrained")
    composition = platform.get("compositions", {}).get("arduino", {})
    claims = set(composition.get("claims", {}))
    required = {
        "timebase", "deadline", "gpio", "irq", "uart", "byte_io",
        "adc", "pwm", "i2c", "spi", "wifi_link", "reset", "power",
    }
    if claims != required:
        raise RuntimeError(f"ESP8266 claim set drift: {sorted(claims)}")
    if set(platform.get("parity_gaps", [])) != {
        "event", "dma_completion", "pulse", "watchdog", "rtc", "flash",
        "reset", "power", "cache", "lease",
    }:
        raise RuntimeError("ESP8266 full-board parity gap ledger drift")
    if set(platform.get("hardware_inapplicable", [])) != {"usb", "multicore"}:
        raise RuntimeError("ESP8266 hardware-inapplicable ledger drift")
    if platform.get("external_optional") != ["servo"]:
        raise RuntimeError("ESP8266 external-module ledger drift")
    limitations = platform.get("limitations", "").lower()
    for excluded in ("native usb", "multicore", "memory isolation", "dma"):
        if excluded not in limitations:
            raise RuntimeError(f"missing explicit ESP8266 limitation: {excluded}")


def main() -> int:
    try:
        validate_metadata()
        cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
        if not cli:
            raise RuntimeError("arduino-cli not found")
        with tempfile.TemporaryDirectory(prefix="nobro-esp8266-") as temporary:
            root = pathlib.Path(temporary)
            compile_case(cli, root, "disabled", DISABLED)
            output = compile_case(cli, root, "full", FULL)
            metrics = re.findall(r"used\s+(\d+)\s+/\s+(\d+) bytes", output)
            if len(metrics) < 2:
                raise RuntimeError("ESP8266 compile did not report RAM/IRAM resource bounds")
            negative = sketch(root, "negative", NEGATIVE)
            failure = run([
                cli, "compile", "--fqbn", "arduino:avr:uno",
                "--library", str(PACKAGE), str(negative)
            ], expect_success=False)
            if "requires the Arduino-ESP8266 board package" not in failure:
                raise RuntimeError("cross-architecture include did not fail closed")
            print("  PASS architecture guard")
            print("  RESOURCE " + ", ".join(f"{used}/{limit}" for used, limit in metrics))
    except (OSError, RuntimeError, ValueError) as error:
        print(f"ESP8266 CONSTRAINED: FAIL ({error})")
        return 1
    print("ESP8266 CONSTRAINED: PASS (exact D1 Mini profile, bounded providers, "
          "nonblocking WiFi lifecycle, explicit hardware exclusions)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
