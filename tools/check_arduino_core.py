#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Compile and execute the portable Arduino/PlatformIO Core header contract."""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]

HARNESS = r'''#include <NobroRTOSCore.h>
#include <assert.h>
#include <stddef.h>
#include <stdint.h>

using namespace nobro::core;

static uint8_t trace[8];
static size_t trace_length;

static void record(void *context)
{
    trace[trace_length++] = *static_cast<uint8_t *>(context);
}

int main()
{
    uint8_t high = 1;
    uint8_t periodic = 2;
    uint8_t event = 3;
    const Task tasks[] = {
        Task::periodic(1, 0, 10, 10, 1, record, &high),
        Task::periodic(2, 3, 20, 20, 2, record, &periodic, 5),
        Task::event(3, 7, 1, record, &event),
    };
    Scheduler<3> scheduler;
    assert(scheduler.begin(tasks, 100) == ADMITTED);
    assert(scheduler.releaseDue(100) == 1);
    assert(scheduler.markReadyById(3));
    assert(scheduler.runReady() == 2);
    assert(trace_length == 2 && trace[0] == 1 && trace[1] == 3);
    uint32_t release = 0;
    assert(scheduler.nextRelease(100, release) && release == 105);
    assert(scheduler.releaseDue(105) == 1);
    assert(scheduler.runNext() && trace[2] == 2);
    assert(scheduler.isIdle());
    assert(!scheduler.markReady(3));

    Scheduler<2> invalid;
    const Task duplicate[] = {
        Task::event(1, 0, 1, record, &high),
        Task::event(1, 1, 1, record, &event),
    };
    assert(invalid.begin(duplicate, 0) == DUPLICATE_ID);
    const Task overloaded[] = {
        Task::periodic(1, 0, 10, 10, 6, record, &high),
        Task::periodic(2, 1, 10, 10, 6, record, &event),
    };
    assert(invalid.begin(overloaded, 0) == UTILIZATION_EXCEEDED);

    Scheduler<1> wrapping;
    const Task wrap_task[] = {
        Task::periodic(9, 0, 10, 10, 1, record, &high, 5),
    };
    assert(wrapping.begin(wrap_task, UINT32_MAX - 4u) == ADMITTED);
    assert(wrapping.releaseDue(0) == 1);
    return 0;
}
'''


def main() -> int:
    requested = os.environ.get("CXX")
    compiler = shutil.which(requested) if requested else None
    if compiler is None:
        compiler = next(
            (found for name in ("c++", "g++", "clang++")
             if (found := shutil.which(name)) is not None),
            None,
        )
    if compiler is None:
        print("ARDUINO CORE HOST GATE: FAIL (no C++ compiler)", file=sys.stderr)
        return 1
    with tempfile.TemporaryDirectory(prefix="nobro-rtos-core-cpp-") as temporary:
        root = Path(temporary)
        source = root / "main.cpp"
        program = root / ("core-test.exe" if os.name == "nt" else "core-test")
        source.write_text(HARNESS, encoding="utf-8")
        command = [
            compiler,
            "-std=c++11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
            f"-I{(ROOT / 'src').resolve()}",
            str(source),
            "-o",
            str(program),
        ]
        completed = subprocess.run(command, text=True, capture_output=True)
        if completed.returncode != 0:
            print(completed.stdout, end="", file=sys.stderr)
            print(completed.stderr, end="", file=sys.stderr)
            print("ARDUINO CORE HOST GATE: FAIL (compile)", file=sys.stderr)
            return 1
        executed = subprocess.run([program], check=False)
        if executed.returncode != 0:
            print(
                f"ARDUINO CORE HOST GATE: FAIL (exit {executed.returncode})",
                file=sys.stderr,
            )
            return 1
    print(f"ARDUINO CORE HOST GATE: PASS ({Path(compiler).name})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
