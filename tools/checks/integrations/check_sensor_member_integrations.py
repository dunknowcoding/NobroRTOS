#!/usr/bin/env python3
"""Gate exact INA Series and DiFinders checkouts against Nobro Arduino facades."""

from __future__ import annotations

import argparse
import pathlib
import shutil
import subprocess
import sys
import tempfile


ROOT = pathlib.Path(__file__).resolve().parents[3]
PACKAGE = ROOT / "packages" / "arduino"
INA_PIN = "c806fb8d1ad75c85f86413aee5a1306eff191918"
INA_VERSION = "0.5.4"
DIFINDERS_PIN = "1ccffa5cf8f34c7ab933f85c7956021a4692256e"
DIFINDERS_VERSION = "0.1.2"

INA_SKETCH = r'''#include <INA_Series_Sensor.h>
#include <NobroInaSeries.h>
InaBridge219 ina219("INA219", 0x40);
InaBridge3221 ina3221("INA3221", 0x40);
nobro::InaSeriesAdapter<InaBridge219> one(ina219, SDA, SCL);
nobro::Ina3221Adapter three(ina3221, SDA, SCL);
void setup() {
  nobro_power_channel_sample_t samples[3] = {};
  size_t written = 0;
  (void)one.begin(); (void)one.sample(samples[0]);
  (void)three.begin(); (void)three.sampleAll(samples, 3, written);
}
void loop() {}
'''

DIFINDERS_SKETCH = r'''#include <DiFinders.h>
#include <NobroDiFinders.h>
DiFinders::GY_US42 i2c_sensor;
DiFinders::TFMini uart_sensor(Serial);
DiFinders::TF03CanSensor can_sensor;
DiFinders::WT53R_485 rs485_sensor(Serial);
bool mount_i2c(DiFinders::GY_US42 &sensor) {
  sensor.begin(); return sensor.ready();
}
nobro::DiFinderRangingAdapter<DiFinders::GY_US42> i2c_range(
    i2c_sensor, NOBRO_SENSOR_TRANSPORT_I2C, mount_i2c);
nobro::DiFinderRangingAdapter<DiFinders::TFMini> uart_range(
    uart_sensor, NOBRO_SENSOR_TRANSPORT_UART);
nobro::DiFinderRangingAdapter<DiFinders::TF03CanSensor> can_range(
    can_sensor, NOBRO_SENSOR_TRANSPORT_CAN);
nobro::DiFinderRangingAdapter<DiFinders::WT53R_485> rs485_range(
    rs485_sensor, NOBRO_SENSOR_TRANSPORT_RS485);
void setup() {
  nobro_ranging_sample_t sample = {};
  (void)i2c_range.begin(); (void)uart_range.adoptReady();
  (void)can_range.adoptReady(); (void)rs485_range.adoptReady();
  (void)i2c_range.sample(sample); (void)uart_range.sample(sample);
  (void)can_range.sample(sample); (void)rs485_range.sample(sample);
}
void loop() {}
'''


def output(command: list[str], cwd: pathlib.Path | None = None) -> str:
    result = subprocess.run(command, cwd=cwd, text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError((result.stdout + result.stderr).strip())
    return result.stdout.strip()


def verify(library: pathlib.Path, name: str, version: str, pin: str) -> None:
    properties = (library / "library.properties").read_text(encoding="utf-8")
    if f"name={name}" not in properties or f"version={version}" not in properties:
        raise RuntimeError(f"{name} must be version {version}")
    if output(["git", "rev-parse", "HEAD"], library) != pin:
        raise RuntimeError(f"{name} checkout differs from pin {pin}")
    if output(["git", "status", "--porcelain"], library):
        raise RuntimeError(f"{name} checkout has local modifications")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ina", type=pathlib.Path, required=True)
    parser.add_argument("--difinders", type=pathlib.Path, required=True)
    parser.add_argument("--fqbn", action="append", required=True)
    args = parser.parse_args()
    cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
    if not cli:
        print("SENSOR MEMBER INTEGRATIONS: FAIL (arduino-cli not found)")
        return 1
    try:
        ina = args.ina.resolve(strict=True)
        difinders = args.difinders.resolve(strict=True)
        verify(ina, "INA Series Sensor", INA_VERSION, INA_PIN)
        verify(difinders, "DiFinders", DIFINDERS_VERSION, DIFINDERS_PIN)
        cases = (("ina", INA_SKETCH, ina), ("difinders", DIFINDERS_SKETCH, difinders))
        with tempfile.TemporaryDirectory(prefix="nobro-sensor-integrations-") as temp:
            base = pathlib.Path(temp)
            for fqbn in args.fqbn:
                for name, source, library in cases:
                    sketch = base / f"{name}_{len(list(base.iterdir()))}"
                    sketch.mkdir()
                    (sketch / f"{sketch.name}.ino").write_text(source, encoding="utf-8")
                    output([
                        cli, "compile", "--fqbn", fqbn, "--library", str(PACKAGE),
                        "--library", str(library), str(sketch),
                    ], ROOT)
                    print(f"  PASS {fqbn} {name}")
    except (OSError, RuntimeError) as error:
        print(f"SENSOR MEMBER INTEGRATIONS: FAIL ({error})")
        return 1
    print(
        "SENSOR MEMBER INTEGRATIONS: PASS "
        f"({len(args.fqbn)} architectures x 2 pinned members)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
