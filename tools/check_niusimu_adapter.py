#!/usr/bin/env python3
"""Compile the Nobro bridge against one exact, external NiusIMU checkout."""

import argparse
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "packages" / "arduino"
PIN = "7c8248b8d37294be6cc545c3cf549e907ccbc955"
VERSION = "0.3.0"

CASES = {
    "mpu6050_i2c": r'''#include <GY521.h>
#include <NobroNiusIMU.h>
GY521 sensor;
nobro::NiusImuAdapter imu(sensor, NOBRO_IMU_MPU6050, 0x68);
void setup() {
  nobro_imu_sample_t sample = {};
  nobro_imu_calibration_t calibration = imu.calibration();
  (void)imu.beginI2C(Wire, 0x68);
  (void)imu.identity();
  (void)imu.sample(sample);
  (void)imu.setCalibration(calibration);
  (void)imu.recover();
  (void)imu.diagnostics();
}

void loop() {}
''',
    "mpu9250_spi": r'''#include <GY9250.h>
#include <NobroNiusIMU.h>
#include <SPI.h>
GY9250 sensor;
nobro::NiusImuAdapter imu(sensor, NOBRO_IMU_MPU9250, SS);
void setup() {
  nobro_imu_sample_t sample = {};
  (void)imu.beginSPI(SPI, SS);
  (void)imu.sample(sample);
  (void)imu.recover();
}

void loop() {}
''',
    "gy91_composite": r'''#include <GY91.h>
#include <NobroNiusIMU.h>
GY91 module;
nobro::NiusImuAdapter imu(module.imu(), NOBRO_IMU_MPU9250, 0x68);
void setup() {
  nobro_imu_sample_t sample = {};
  (void)module.begin();
  (void)imu.sample(sample);
  (void)imu.recover();
}
void loop() {}
''',
}

FAKE_NIUSIMU = r'''#pragma once
#include <stdint.h>
#include <math.h>
class TwoWire {};
class SPIClass {};
namespace nimu {
struct Vec3 { float x = 0; float y = 0; float z = 0; };
struct IMUData { Vec3 accel; Vec3 gyro; Vec3 mag; float temperature = 0; uint32_t timestamp = 0; };
struct IMUCalibration {
  Vec3 accelBias; Vec3 accelScale; Vec3 gyroBias; Vec3 magBias; Vec3 magScale;
  uint16_t magic = 0x4D49;
};
class IMUSensor {
public:
  virtual ~IMUSensor() {}
  virtual bool begin() = 0;
  virtual bool beginI2C(TwoWire&, uint8_t) = 0;
  virtual bool beginSPI(SPIClass&, uint8_t) = 0;
  virtual uint8_t whoAmI() = 0;
  virtual bool update() = 0;
  virtual const IMUData& data() const = 0;
  virtual bool hasMagnetometer() const = 0;
  virtual IMUCalibration getCalibration() const = 0;
  virtual void setCalibration(const IMUCalibration&) = 0;
};
}
'''

SELFTEST_SOURCE = r'''#include <assert.h>
#include "NobroNiusIMU.h"
class Mock : public nimu::IMUSensor {
public:
  bool ready = true;
  bool update_ok = true;
  int default_begins = 0;
  int i2c_begins = 0;
  int spi_begins = 0;
  nimu::IMUData value;
  nimu::IMUCalibration calibration_value;
  bool begin() override { ++default_begins; return ready; }
  bool beginI2C(TwoWire&, uint8_t) override { ++i2c_begins; return ready; }
  bool beginSPI(SPIClass&, uint8_t) override { ++spi_begins; return ready; }
  uint8_t whoAmI() override { return 0x71; }
  bool update() override { return update_ok; }
  const nimu::IMUData& data() const override { return value; }
  bool hasMagnetometer() const override { return true; }
  nimu::IMUCalibration getCalibration() const override { return calibration_value; }
  void setCalibration(const nimu::IMUCalibration& value) override { calibration_value = value; }
};
int main() {
  Mock sensor;
  sensor.value.accel.x = 0.3f; sensor.value.accel.y = 0.4f;
  sensor.value.gyro.z = 1.5f; sensor.value.mag.x = 2.25f;
  sensor.value.temperature = 21.5f; sensor.value.timestamp = 1234;
  SPIClass spi;
  nobro::NiusImuAdapter adapter(sensor, NOBRO_IMU_MPU9250, 7);
  assert(adapter.beginSPI(spi, 7));
  nobro_imu_sample_t sample = {};
  assert(adapter.sample(sample));
  assert(sample.accel_mg[0] == 300 && sample.accel_mag_mg == 500);
  assert(sample.gyro_mdps[2] == 1500 && sample.mag_milli_ut[0] == 2250);
  assert(sample.temperature_centi_c == 2150 && sample.timestamp_us == 1234);
  assert(adapter.recover() && sensor.spi_begins == 2 && sensor.default_begins == 0);
  nobro_imu_calibration_t bad = {};
  assert(!adapter.setCalibration(bad));
  sensor.update_ok = false;
  assert(!adapter.sample(sample));
  nobro_imu_diagnostics_t diagnostics = adapter.diagnostics();
  assert(diagnostics.samples == 1 && diagnostics.read_errors == 1);
  assert(diagnostics.last_event == NOBRO_IMU_EVENT_READ_ERROR);
  sensor.ready = false;
  assert(!adapter.recover() && sensor.spi_begins == 3);
  assert(adapter.diagnostics().last_event == NOBRO_IMU_EVENT_RECOVERY_EXHAUSTED);
}
'''


def output(command: list[str], cwd: pathlib.Path | None = None) -> str:
    result = subprocess.run(command, cwd=cwd, text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError((result.stdout + result.stderr).strip())
    return result.stdout.strip()


def verify_checkout(library: pathlib.Path) -> None:
    if not (library / "library.properties").is_file():
        raise RuntimeError(f"not an Arduino library: {library}")
    properties = (library / "library.properties").read_text(encoding="utf-8")
    if f"version={VERSION}" not in properties or "name=NiusIMU" not in properties:
        raise RuntimeError(f"NiusIMU version must be exactly {VERSION}")
    revision = output(["git", "rev-parse", "HEAD"], library)
    if revision != PIN:
        raise RuntimeError(f"NiusIMU checkout is {revision}; expected pinned {PIN}")
    if output(["git", "status", "--porcelain"], library):
        raise RuntimeError("NiusIMU checkout has local modifications")
    matrix = json.loads((ROOT / "core" / "ecosystem" / "integration_matrix.json").read_text(encoding="utf-8"))
    domain = next(item for item in matrix["domains"] if item["id"] == "nobro_imu")
    member = next(item for item in domain["library_members"] if item["id"] == "niusimu")
    actual_sensors = sorted(path.name for path in (library / "src" / "sensors").iterdir() if path.is_dir())
    actual_boards = sorted(path.name for path in (library / "src" / "boards").iterdir() if path.is_dir())
    if actual_sensors != sorted(member["upstream_sensor_drivers"]):
        raise RuntimeError("NiusIMU sensor inventory differs from the ecosystem matrix")
    if actual_boards != sorted(member["upstream_board_modules"]):
        raise RuntimeError("NiusIMU board/module inventory differs from the ecosystem matrix")


def selftest() -> int:
    compiler = shutil.which("g++") or shutil.which("g++.exe")
    if not compiler:
        print("NIUSIMU ADAPTER SELFTEST: FAIL (g++ not found)")
        return 1
    try:
        with tempfile.TemporaryDirectory(prefix="nobro-niusimu-host-") as temp:
            base = pathlib.Path(temp)
            (base / "NiusIMU.h").write_text(FAKE_NIUSIMU, encoding="utf-8")
            source = base / "selftest.cpp"
            source.write_text(SELFTEST_SOURCE, encoding="utf-8")
            binary = base / ("selftest.exe" if sys.platform == "win32" else "selftest")
            output([compiler, "-std=c++11", "-Wall", "-Wextra", "-Werror",
                    "-I", str(base), "-I", str(PACKAGE / "src"), str(source),
                    "-o", str(binary)])
            output([str(binary)])
    except (OSError, RuntimeError) as error:
        print(f"NIUSIMU ADAPTER SELFTEST: FAIL ({error})")
        return 1
    print("NIUSIMU ADAPTER SELFTEST: PASS (units, SPI recovery, diagnostics, calibration reject)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--library", type=pathlib.Path)
    parser.add_argument("--fqbn", action="append")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if args.library is None or not args.fqbn:
        parser.error("--library and at least one --fqbn are required unless --selftest is used")
    cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
    if not cli:
        print("NIUSIMU ADAPTER: FAIL (arduino-cli not found)")
        return 1
    try:
        library = args.library.resolve(strict=True)
        verify_checkout(library)
        with tempfile.TemporaryDirectory(prefix="nobro-niusimu-") as temp:
            base = pathlib.Path(temp)
            for fqbn in args.fqbn:
                for case, source in CASES.items():
                    sketch = base / case
                    sketch.mkdir(exist_ok=True)
                    (sketch / f"{case}.ino").write_text(source, encoding="utf-8")
                    command = [cli, "compile", "--fqbn", fqbn, "--library",
                               str(PACKAGE), "--library", str(library), str(sketch)]
                    output(command, ROOT)
                    print(f"  PASS {fqbn} {case}")
    except (OSError, RuntimeError) as error:
        print(f"NIUSIMU ADAPTER: FAIL ({error})")
        return 1
    print(f"NIUSIMU ADAPTER: PASS ({len(args.fqbn)} architectures x {len(CASES)} representative paths; 37 sensor drivers + 25 board modules; pin {PIN[:12]})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
