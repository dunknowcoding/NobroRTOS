#!/usr/bin/env python3
"""Verify pinned wireless member libraries and representative portable builds."""

import argparse
import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "packages" / "arduino"
WIRELESS_PIN = "f40e76cccbcb0b5d0597f784ae5be1e6a52d46cb"
ZIGBEE_PIN = "4d4fb8f1afa7a4406d2a0bf399f6249681bc62b9"
MODULES = {"HC06", "HC12", "LoRa", "NRF24L01", "PN532", "RC522"}
STUBS = {"HC06", "HC12", "NRF24L01", "PN532"}

CASES = {
    "rc522": ("arduino:renesas_uno:unor4wifi", r'''#include <NiusWireless.h>
#include <NobroNiusWireless.h>
NiusRC522 device(SDA, 10, SCL, 11, 12);
nobro::NiusWirelessHealthAdapter wireless(device);
void setup() { if (false) { wireless.begin(); wireless.ready(); wireless.recover(); } }
void loop() {}
'''),
    "lora": ("esp32:esp32:esp32s3", r'''#include <NiusWireless.h>
#include <NobroNiusWireless.h>
NiusRFM95 device(10, 9, 2);
nobro::NiusLoRaAdapter wireless(device);
void setup() { uint8_t data[2] = {1, 2}; if (false) { wireless.begin(); wireless.send(data, 2); wireless.receive(data, 2); wireless.recover(); } }
void loop() {}
'''),
}

ZIGBEE_CASE = r'''#include <CC2530Radio.h>
CC2530Radio radio;
void setup() { if (false) radio.begin(); }
void loop() {}
'''


def run(command, cwd=ROOT):
    completed = subprocess.run(command, cwd=cwd, text=True, capture_output=True)
    if completed.returncode:
        raise RuntimeError((completed.stdout + completed.stderr).strip())
    return completed.stdout.strip()


def verify_checkout(path: pathlib.Path, pin: str, name: str, version: str) -> pathlib.Path:
    path = path.resolve(strict=True)
    properties = (path / "library.properties").read_text(encoding="utf-8")
    if f"name={name}" not in properties or f"version={version}" not in properties:
        raise RuntimeError(f"{name} must be exactly version {version}")
    if run(["git", "rev-parse", "HEAD"], path) != pin:
        raise RuntimeError(f"{name} checkout is not pinned to {pin}")
    if run(["git", "status", "--porcelain"], path):
        raise RuntimeError(f"{name} checkout has local modifications")
    return path


def verify_inventory(wireless: pathlib.Path, zigbee: pathlib.Path) -> None:
    actual = {path.name for path in (wireless / "src" / "modules").iterdir() if path.is_dir()}
    if actual != MODULES:
        raise RuntimeError(f"NiusWireless module inventory drift: {sorted(actual)}")
    for module in STUBS:
        sources = list((wireless / "src" / "modules" / module).glob("*.cpp"))
        if not sources or not any("Status: STUB" in source.read_text(encoding="utf-8", errors="replace")
                                  for source in sources):
            raise RuntimeError(f"{module} support status changed; audit before promotion")
    for module in MODULES - STUBS:
        sources = list((wireless / "src" / "modules" / module).glob("*.cpp"))
        if not sources or all("Status: STUB" in source.read_text(encoding="utf-8", errors="replace")
                              for source in sources):
            raise RuntimeError(f"{module} unexpectedly has no implementation")
    required = ["CC2530Radio.h", "ZigbeeNetwork.h", "ZigbeeAps.h", "ZigbeeZcl.h"]
    for name in required:
        if not (zigbee / "src" / name).is_file():
            raise RuntimeError(f"NiusZigbee surface missing {name}")


def compile_sketch(cli: str, library: pathlib.Path, case: str, fqbn: str,
                   source: str, base: pathlib.Path) -> None:
    sketch = base / case
    sketch.mkdir()
    (sketch / f"{case}.ino").write_text(source, encoding="utf-8")
    run([cli, "compile", "--fqbn", fqbn, "--library", str(PACKAGE),
         "--library", str(library), str(sketch)])
    print(f"  PASS {fqbn} {case}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--niuswireless", type=pathlib.Path, required=True)
    parser.add_argument("--niuszigbee", type=pathlib.Path, required=True)
    parser.add_argument("--compile", action="store_true")
    parser.add_argument("--compile-zigbee", action="store_true",
                        help="requires the Windows-only ArduinoNRF toolchain")
    args = parser.parse_args()
    try:
        wireless = verify_checkout(args.niuswireless, WIRELESS_PIN, "NiusWireless", "0.1.0")
        zigbee = verify_checkout(args.niuszigbee, ZIGBEE_PIN, "NiusZigbee", "1.0.0")
        verify_inventory(wireless, zigbee)
        if args.compile or args.compile_zigbee:
            cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
            if not cli:
                raise RuntimeError("arduino-cli not found")
            with tempfile.TemporaryDirectory(prefix="nobro-wireless-") as temp:
                base = pathlib.Path(temp)
                if args.compile:
                    for case, (fqbn, source) in CASES.items():
                        compile_sketch(cli, wireless, case, fqbn, source, base)
                if args.compile_zigbee:
                    compile_sketch(cli, zigbee, "zigbee", "arduinonrf:nrf52:promicro_nrf52840",
                                   ZIGBEE_CASE, base)
    except (OSError, RuntimeError) as error:
        print(f"WIRELESS INTEGRATIONS: FAIL ({error})")
        return 1
    print("WIRELESS INTEGRATIONS: PASS (RC522 + LoRa implemented; four stubs explicit; "
          "NiusZigbee CC2530 surface pinned)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
