#!/usr/bin/env python3
"""Compile and execute bounded INA Series and DiFinders Arduino facades."""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import sys
import tempfile


ROOT = pathlib.Path(__file__).resolve().parents[3]
INCLUDE = ROOT / "packages" / "arduino" / "src"

ARDUINO = r'''#pragma once
#include <stddef.h>
#include <stdint.h>
extern uint32_t fake_micros;
inline uint32_t micros() { return fake_micros; }
'''

INA = r'''#pragma once
#include <stdint.h>
class InaBridge3221 {
 public:
  int begins = 0;
  void begin(int, int, uint32_t) { ++begins; }
  float readBusVoltage(uint8_t channel) { return 3.0f + channel; }
  float readShuntVoltage(uint8_t channel) { return 0.001f * channel; }
  float readCurrent(uint8_t channel) { return 0.1f * channel; }
  float readPower(uint8_t channel) { return 0.4f * channel; }
};
'''

DIFINDERS = r'''#pragma once
#include <stdint.h>
namespace DiFinders {
enum class SensorStatus : uint8_t { Ok, Timeout, OutOfRange, NotReady, Disabled, Error };
enum class DetectionState : uint8_t { Inactive, Active };
struct RangeReading {
  SensorStatus status = SensorStatus::NotReady;
  uint16_t distanceMm = 0;
  uint16_t rawValue = 0;
  uint32_t timestampMs = 0;
};
struct ProximityReading {
  SensorStatus status = SensorStatus::NotReady;
  DetectionState state = DetectionState::Inactive;
  uint16_t strengthPermille = 0;
  uint16_t rawValue = 0;
  uint32_t timestampMs = 0;
  bool detected() const { return state == DetectionState::Active; }
};
struct MotionReading {
  SensorStatus status = SensorStatus::NotReady;
  DetectionState state = DetectionState::Inactive;
  bool rose = false;
  bool fell = false;
  uint32_t timestampMs = 0;
  uint32_t lastActiveMs = 0;
  bool detected() const { return state == DetectionState::Active; }
};
}
'''

SOURCE = r'''#include <assert.h>
#include "NobroInaSeries.h"
#include "NobroDiFinders.h"

uint32_t fake_micros = 100;

class SingleIna {
 public:
  int begins = 0;
  void begin(int, int, uint32_t) { ++begins; }
  float readBusVoltage() { return 5.0f; }
  float readShuntVoltage() { return 0.002f; }
  float readCurrent() { return 0.25f; }
  float readPower() { return 1.25f; }
};

class SpiIna {
 public:
  uint32_t last_hz = 0;
  void begin(uint32_t hz) { last_hz = hz; }
  float readBusVoltage() { return 3.3f; }
  float readShuntVoltage() { return 0.001f; }
  float readCurrent() { return 0.1f; }
  float readPower() { return 0.33f; }
};

struct RangeSensor {
  DiFinders::RangeReading value;
  DiFinders::RangeReading read() { return value; }
};
struct PresenceSensor {
  DiFinders::ProximityReading value;
  DiFinders::ProximityReading read() { return value; }
};
struct MotionSensor {
  DiFinders::MotionReading value;
  DiFinders::MotionReading read() { return value; }
};
template <typename T> bool mount(T &) { return true; }

int main() {
  SingleIna single;
  nobro::InaSeriesAdapter<SingleIna> one(single, 8, 9);
  assert(one.begin() == NOBRO_POWER_MONITOR_OK);
  nobro_power_channel_sample_t power = {};
  assert(one.sample(power, 200) == NOBRO_POWER_MONITOR_OK);
  assert(power.bus_uv == 5000000 && power.shunt_uv == 2000);
  assert(power.current_ua == 250000 && power.power_uw == 1250000);
  assert(power.sequence == 1 && one.begin() == NOBRO_POWER_MONITOR_INVALID_CONFIG);
  fake_micros = 0xfffffff0u;
  assert(one.sample(power, 0x10u) == NOBRO_POWER_MONITOR_OK);
  assert(one.sample(power, 0xffffffefu) == NOBRO_POWER_MONITOR_DEADLINE_MISS);
  one.quiesce(); assert(one.recover() == NOBRO_POWER_MONITOR_OK && single.begins == 2);

  SpiIna spi;
  nobro::InaSeriesSpiAdapter<SpiIna> spi_adapter(spi, 8000000);
  assert(spi_adapter.begin() == NOBRO_POWER_MONITOR_OK && spi.last_hz == 8000000);

  InaBridge3221 chip;
  nobro::Ina3221Adapter three(chip, 8, 9);
  assert(three.begin() == NOBRO_POWER_MONITOR_OK);
  nobro_power_channel_sample_t channels[3] = {};
  size_t written = 0;
  fake_micros = 100;
  assert(three.sampleAll(channels, 3, written, 200) == NOBRO_POWER_MONITOR_OK);
  assert(written == 3 && channels[2].channel == 3);
  assert(channels[2].bus_uv == 6000000 && channels[2].shunt_uv == 3000);
  assert(three.sampleChannel(4, power) == NOBRO_POWER_MONITOR_INVALID_CHANNEL);

  RangeSensor range;
  range.value.status = DiFinders::SensorStatus::Ok;
  range.value.distanceMm = 321; range.value.rawValue = 99; range.value.timestampMs = 7;
  nobro::DiFinderRangingAdapter<RangeSensor> ranging(
      range, NOBRO_SENSOR_TRANSPORT_CAN, mount<RangeSensor>);
  assert(ranging.begin());
  nobro_ranging_sample_t ranged = {};
  assert(ranging.sample(ranged, 200));
  assert(ranged.distance_mm == 321 && ranged.raw == 99 && ranged.timestamp_us == 7000);
  assert(ranging.transport() == NOBRO_SENSOR_TRANSPORT_CAN);

  PresenceSensor proximity;
  proximity.value.status = DiFinders::SensorStatus::Ok;
  proximity.value.state = DiFinders::DetectionState::Active;
  proximity.value.strengthPermille = 850;
  nobro::DiFinderPresenceAdapter<PresenceSensor> presence(
      proximity, NOBRO_SENSOR_TRANSPORT_I2C, mount<PresenceSensor>);
  assert(presence.begin());
  nobro_presence_sample_t present = {};
  assert(presence.sample(present) && present.detected && present.strength_permille == 850);
  presence.quiesce(); assert(presence.state() == NOBRO_SENSOR_PROVIDER_SUSPENDED);
  assert(presence.recover());

  MotionSensor motion;
  motion.value.status = DiFinders::SensorStatus::Ok;
  motion.value.state = DiFinders::DetectionState::Active;
  motion.value.rose = true;
  nobro::DiFinderPresenceAdapter<MotionSensor> moving(
      motion, NOBRO_SENSOR_TRANSPORT_RS485, mount<MotionSensor>);
  assert(moving.begin());
  assert(moving.sample(present) && present.rose && !present.fell);

  RangeSensor adopted;
  nobro::DiFinderRangingAdapter<RangeSensor> no_mount(
      adopted, NOBRO_SENSOR_TRANSPORT_UART);
  assert(!no_mount.begin() && no_mount.state() == NOBRO_SENSOR_PROVIDER_FAULTED);
  nobro::DiFinderRangingAdapter<RangeSensor> preinitialized(
      adopted, NOBRO_SENSOR_TRANSPORT_UART);
  assert(preinitialized.adoptReady());
}
'''


def main() -> int:
    compiler = shutil.which("g++") or shutil.which("g++.exe")
    if not compiler:
        print("SENSOR MEMBER FACADES: FAIL (g++ not found)")
        return 1
    try:
        with tempfile.TemporaryDirectory(prefix="nobro-sensor-members-") as temp:
            base = pathlib.Path(temp)
            (base / "Arduino.h").write_text(ARDUINO, encoding="utf-8")
            (base / "INA_Series_Sensor.h").write_text(INA, encoding="utf-8")
            (base / "DiFinders.h").write_text(DIFINDERS, encoding="utf-8")
            source = base / "selftest.cpp"
            source.write_text(SOURCE, encoding="utf-8")
            binary = base / ("selftest.exe" if sys.platform == "win32" else "selftest")
            command = [
                compiler, "-std=c++11", "-Wall", "-Wextra", "-Werror",
                "-I", str(base), "-I", str(INCLUDE), str(source), "-o", str(binary),
            ]
            result = subprocess.run(command, text=True, capture_output=True)
            if result.returncode:
                raise RuntimeError((result.stdout + result.stderr).strip())
            result = subprocess.run([str(binary)], text=True, capture_output=True)
            if result.returncode:
                raise RuntimeError((result.stdout + result.stderr).strip())
    except (OSError, RuntimeError) as error:
        print(f"SENSOR MEMBER FACADES: FAIL ({error})")
        return 1
    print("SENSOR MEMBER FACADES: PASS (INA I2C/SPI/3-channel + ranging/presence lifecycle/deadline)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
