#!/usr/bin/env python3
"""Compile and execute the bounded RoboServo Arduino facade."""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[3]
FACADE = ROOT / "packages" / "arduino" / "src"

FAKE_ROBO = r'''#ifndef ROBO_SERVO_H
#define ROBO_SERVO_H
#include <stdint.h>
#define ROBOSERVO_MIN_FREQUENCY 40
#define ROBOSERVO_MAX_FREQUENCY 400
#define ROBOSERVO_INVALID_SERVO 255
enum RoboServoType { SERVO_TYPE_180 = 180 };
uint32_t micros();
class RoboServo {
public:
  bool fail_attach = false;
  bool attached_value = false;
  int pulse = 0;
  uint8_t attach(int, int, int, RoboServoType, int) {
    attached_value = !fail_attach;
    return fail_attach ? ROBOSERVO_INVALID_SERVO : 0;
  }
  void writeMicroseconds(int value) { if (attached_value) pulse = value; }
  int readMicroseconds() const { return pulse; }
  bool attached() const { return attached_value; }
  void release() { pulse = 0; }
  void detach() { attached_value = false; pulse = 0; }
};
#endif
'''

TEST = r'''#include <assert.h>
#include <stdint.h>
static uint32_t now_us = 10;
uint32_t micros() { return now_us++; }
#include <NobroRoboServo.h>

int main() {
  RoboServo servo;
  nobro::RoboServoAdapter actuator(servo);
  assert(actuator.begin(5) == NOBRO_SERVO_OK);
  nobro_servo_receipt_t receipt = {};
  assert(actuator.command(0, 1500, 10, 20, &receipt) == NOBRO_SERVO_OK);
  assert(receipt.sequence == 1 && receipt.pulse_us == 1500);
  assert(actuator.command(1, 1500, 10, 20, &receipt) == NOBRO_SERVO_INVALID_COMMAND);
  assert(actuator.command(0, 3000, 10, 20, &receipt) == NOBRO_SERVO_INVALID_COMMAND);
  assert(actuator.command(0, 1500, 21, 20, &receipt) == NOBRO_SERVO_DEADLINE_MISS);
  actuator.quiesce();
  assert(actuator.state() == NOBRO_SERVO_SUSPENDED);
  assert(actuator.command(0, 1500, 10, 20, &receipt) == NOBRO_SERVO_NOT_READY);
  assert(actuator.recover() == NOBRO_SERVO_OK);
  assert(actuator.lastPulseUs() == 1500);
  actuator.release();
  assert(actuator.state() == NOBRO_SERVO_DOWN && !servo.attached());

  RoboServo failed;
  failed.fail_attach = true;
  nobro::RoboServoAdapter rejected(failed);
  assert(rejected.begin(5) == NOBRO_SERVO_TRANSPORT_ERROR);
  return 0;
}
'''


def main() -> int:
    cxx = next(
        (path for name in ("g++", "clang++", "c++") if (path := shutil.which(name))),
        None,
    )
    if not cxx:
        print("SERVO FACADE: FAIL (no C++ compiler)")
        return 1
    with tempfile.TemporaryDirectory(prefix="nobro-servo-facade-") as temp:
        root = pathlib.Path(temp)
        (root / "RoboServo.h").write_text(FAKE_ROBO, encoding="utf-8")
        source = root / "test.cpp"
        source.write_text(TEST, encoding="utf-8")
        binary = root / ("test.exe" if sys.platform == "win32" else "test")
        built = subprocess.run(
            [
                cxx,
                "-std=c++11",
                "-Wall",
                "-Wextra",
                "-Werror",
                f"-I{root}",
                f"-I{FACADE}",
                str(source),
                "-o",
                str(binary),
            ],
            capture_output=True,
            text=True,
        )
        if built.returncode:
            print("SERVO FACADE: FAIL (compile)")
            print((built.stdout + built.stderr).strip())
            return 1
        ran = subprocess.run([str(binary)], capture_output=True, text=True)
        if ran.returncode:
            print(f"SERVO FACADE: FAIL (runtime {ran.returncode})")
            print((ran.stdout + ran.stderr).strip())
            return 1
    print("SERVO FACADE: PASS (bounds, deadlines, receipts, lifecycle, recovery)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
