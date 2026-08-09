#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Generate, host-test, and optionally SDCC-build NobroRTOS Core for 8-bit MCUs."""

from __future__ import annotations

import argparse
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
PORT = ROOT / "ports" / "byte"
GENERATOR_PATH = ROOT / "tools" / "nobro_core.py"


def load_generator():
    spec = importlib.util.spec_from_file_location("nobro_core_generator", GENERATOR_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load the Core generator")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GENERATOR = load_generator()


def run(command: list[str], cwd: Path) -> str:
    completed = subprocess.run(
        command,
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"command failed ({completed.returncode}): {' '.join(command)}\n"
            f"{completed.stdout or ''}"
        )
    return completed.stdout or ""


def native(path: Path) -> str:
    return path.resolve().as_posix()


HOST_HARNESS = r'''#include "nobro_core_config.h"
#include <assert.h>

static unsigned char order[4];
static unsigned char calls;
static unsigned char idle_calls;
static unsigned char watchdog_calls;

void sensor_step(void) { order[calls++] = 1u; }
void control_step(void) { order[calls++] = 2u; }
void nobro_app_idle(void) {
    ++idle_calls;
    nobro_core_abi.now = (unsigned char)(nobro_core_abi.now + nobro_core_abi.sleep_ticks);
}
void nobro_app_watchdog(void) { ++watchdog_calls; }

int main(void) {
    nobro_core_abi.now = 0u;
    nobro_core_reset();
    nobro_core_dispatch();
    assert(calls == 2u && order[0] == 1u && order[1] == 2u);
    assert(idle_calls == 1u && watchdog_calls == 1u);
    assert(nobro_core_abi.now == 10u && nobro_core_abi.sleep_ticks == 10u);

    nobro_core_abi.message = 0x11u; nobro_core_post();
    nobro_core_abi.message = 0x22u; nobro_core_post();
    nobro_core_abi.message = 0x33u; nobro_core_post();
    assert(nobro_core_abi.result == NOBRO_CORE_RESULT_REJECTED);
    assert((nobro_core_abi.faults & NOBRO_CORE_FAULT_MAILBOX_FULL) != 0u);
    nobro_core_take(); assert(nobro_core_abi.message == 0x11u);
    nobro_core_take(); assert(nobro_core_abi.message == 0x22u);
    nobro_core_take(); assert(nobro_core_abi.result == NOBRO_CORE_RESULT_EMPTY);
    nobro_core_recover(); assert(nobro_core_abi.faults == 0u);

    nobro_core_abi.now = 250u; nobro_core_reset(); nobro_core_dispatch();
    assert(nobro_core_abi.now == 4u);
    return 0;
}
'''


def host_gate(work: Path) -> None:
    compiler = next(
        (shutil.which(name) for name in ("cc", "gcc", "clang") if shutil.which(name)),
        None,
    )
    if compiler is None:
        raise RuntimeError("a host C compiler is required")
    useful = work / "useful"
    harness = useful / "host_harness.c"
    harness.write_text(HOST_HARNESS, encoding="ascii", newline="\n")
    executable = useful / ("host_gate.exe" if sys.platform == "win32" else "host_gate")
    run(
        [
            compiler,
            "-std=c89",
            "-pedantic-errors",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-O2",
            "-I",
            native(PORT / "include"),
            "-I",
            native(useful),
            native(PORT / "src" / "nobro_core.c"),
            native(useful / "nobro_core_config.c"),
            native(useful / "nobro_core_app.c"),
            native(harness),
            "-o",
            native(executable),
        ],
        ROOT,
    )
    run([str(executable)], ROOT)


def sdcc_gate(work: Path, required: bool) -> None:
    compiler = shutil.which("sdcc")
    if compiler is None:
        if required:
            raise RuntimeError("SDCC is required for this profile")
        print("CORE BYTE SDCC: SKIP (not installed)")
        return
    version = run([compiler, "--version"], ROOT).splitlines()[0]
    for name, program_limit, data_limit in (
        ("minimal", 512, 16),
        ("useful", 1024, 32),
    ):
        directory = work / name
        flags = [
            compiler,
            "-mmcs51",
            "--model-small",
            "--std-c99",
            "--opt-code-size",
            "--iram-size",
            "128",
            "--code-size",
            "2048",
            "-I",
            native(PORT / "include"),
            "-I",
            native(directory),
        ]
        sources = [
            (PORT / "src" / "nobro_core.c", "kernel.rel"),
            (directory / "nobro_core_config.c", "config.rel"),
            (directory / "nobro_core_app.c", "router.rel"),
            (PORT / "examples" / name / "app.c", "example.rel"),
        ]
        for source, output in sources:
            run([*flags, "-c", native(source), "-o", output], directory)
        run([*flags, *(output for _, output in sources), "-o", f"{name}.ihx"], directory)
        receipt = json.loads(
            (directory / "nobro_core_contract.json").read_text(encoding="ascii")
        )
        report = (directory / f"{name}.mem").read_text(encoding="utf-8")
        totals = GENERATOR.parse_size_report(
            report + f"\nKernel-owned data: {receipt['kernel_data_bytes']} bytes\n",
            "sdcc-mem",
        )
        if totals["program_bytes"] > program_limit or totals["data_bytes"] > data_limit:
            raise RuntimeError(f"{name} image exceeds the public Core gate: {totals}")
        print(
            f"CORE BYTE SDCC {name}: PASS ({totals['program_bytes']} program, "
            f"{totals['data_bytes']} kernel-data bytes; {version})"
        )


def exhaustive_tick_gate() -> None:
    for now in range(256):
        for release in range(256):
            due = ((now - release) & 0xFF) < 128
            future = (release - now) & 0xFF
            if due == (1 <= future <= 128):
                raise RuntimeError("8-bit wrapping-time partition drift")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--require-sdcc", action="store_true")
    args = parser.parse_args()
    try:
        GENERATOR.selftest()
        exhaustive_tick_gate()
        with tempfile.TemporaryDirectory(prefix="nobro-rtos-core-") as temporary:
            work = Path(temporary)
            minimal = GENERATOR.generate(
                PORT / "examples" / "minimal" / "app.json", work / "minimal"
            )
            useful = GENERATOR.generate(
                PORT / "examples" / "useful" / "app.json", work / "useful"
            )
            if minimal["kernel_data_bytes"] > 16 or useful["kernel_data_bytes"] > 32:
                raise RuntimeError("generated byte-kernel data budget drift")
            host_gate(work)
            sdcc_gate(work, args.require_sdcc)
    except (OSError, RuntimeError, ValueError) as error:
        print(f"CORE BYTE GATE: FAIL ({error})", file=sys.stderr)
        return 1
    print("CORE BYTE GATE: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
