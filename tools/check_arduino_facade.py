#!/usr/bin/env python3
"""Compile and execute allocation-free NobroApp positive/negative contracts."""
import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
HEADER = ROOT / "packages" / "arduino" / "src"
SOURCE = r'''
#include <cassert>
#include "NobroRTOS.h"
int main() {
  nobro::NobroApp<3, 1> ok;
  nobro::TaskId motor = ok.control("motor", 5);
  nobro::TaskId imu = ok.sensor("imu", 10);
  ok.connect(imu, motor).budget(motor, 800);
  assert(ok.admit() && ok.taskCount() == 2 && ok.channelCount() == 1);

  nobro::NobroApp<1, 1> capacity;
  capacity.control("a", 1);
  assert(!capacity.sensor("b", 1).valid());
  assert(!capacity.admit() && capacity.error() == nobro::APP_TASK_CAPACITY);

  nobro::NobroApp<1, 1> deadline;
  nobro::TaskId task = deadline.control("a", 1);
  deadline.budget(task, 1001);
  assert(!deadline.admit() && deadline.error() == nobro::APP_BUDGET_EXCEEDS_PERIOD);

  nobro::NobroApp<1, 1> memory(13000, 3300);
  memory.service("large", 10);
  assert(!memory.admit() && memory.error() == nobro::APP_RESOURCE_BUDGET);
}
'''


def main() -> int:
    compiler = shutil.which("g++") or shutil.which("g++.exe")
    if not compiler:
        print("ARDUINO FACADE: FAIL (g++ not found)")
        return 1
    with tempfile.TemporaryDirectory() as tmp:
        source = pathlib.Path(tmp) / "facade.cpp"
        binary = pathlib.Path(tmp) / ("facade.exe" if sys.platform == "win32" else "facade")
        source.write_text(SOURCE, encoding="utf-8")
        compiled = subprocess.run([compiler, "-std=c++11", "-Wall", "-Wextra",
                                   "-Werror", "-I", str(HEADER), str(source), "-o",
                                   str(binary)], capture_output=True, text=True)
        if compiled.returncode:
            print((compiled.stdout + compiled.stderr).strip())
            print("ARDUINO FACADE: FAIL (compile)")
            return 1
        executed = subprocess.run([str(binary)], capture_output=True, text=True)
        if executed.returncode:
            print((executed.stdout + executed.stderr).strip())
            print("ARDUINO FACADE: FAIL (contracts)")
            return 1
    print("ARDUINO FACADE: PASS (admit/capacity/deadline/resource negatives)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
