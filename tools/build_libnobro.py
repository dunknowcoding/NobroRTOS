#!/usr/bin/env python3
"""Tier C packaging: prebuilt `libnobro.a` a C developer links with no Rust installed.

  --build   cargo-build the nobro-tierc staticlib (nRF52840 no-SoftDevice layout),
            harvest the generated linker scripts (link.x/memory.x/defmt.x) from the
            cargo OUT_DIRs, and stage _work/tierc/ with the canonical C headers +
            a reference module + build.sh/build.cmd one-liners.
  --check   THE GATE: link the reference module and negative init/poll modules
            against the staged archive with arm-none-eabi-gcc and verify each ELF
            has the vector table + resolved module symbols. Portable kernel tests
            execute the corresponding fail-closed callback state transitions.
            Skips (PASS with a note) when
            arm-none-eabi-gcc or the staged archive is absent, so public clones
            without an Arm toolchain still gate clean.

    python tools/build_libnobro.py --build
    python tools/build_libnobro.py --check
"""
import argparse
import glob
import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CORE = os.path.join(ROOT, "core")
OUT = os.path.join(ROOT, "_work", "tierc")
TARGET = "thumbv7em-none-eabihf"
GCC_FLAGS = ["-mcpu=cortex-m4", "-mthumb", "-mfloat-abi=hard", "-mfpu=fpv4-sp-d16"]


def find_gcc():
    for name in ("arm-none-eabi-gcc",):
        hit = shutil.which(name)
        if hit:
            return hit
    return None


def target_dir(env):
    return env.get("CARGO_TARGET_DIR", os.path.join(CORE, "target"))


def build() -> int:
    env = dict(os.environ)
    if subprocess.run(["cargo", "build", "--release", "-p", "nobro-tierc"],
                      cwd=CORE, env=env).returncode:
        return 1
    tdir = target_dir(env)
    rel = os.path.join(tdir, TARGET, "release")
    os.makedirs(OUT, exist_ok=True)
    shutil.copy(os.path.join(rel, "libnobro_tierc.a"), os.path.join(OUT, "libnobro.a"))
    # harvest the build-generated linker scripts this archive expects
    for script, pat in (("link.x", "cortex-m-rt-*"), ("defmt.x", "defmt-*")):
        hits = sorted(glob.glob(os.path.join(rel, "build", pat, "out", script)))
        if not hits:
            sys.exit(f"generated {script} not found under {rel}/build")
        shutil.copy(hits[-1], os.path.join(OUT, script))
    shutil.copy(os.path.join(CORE, "memory-nosd.x"), os.path.join(OUT, "memory.x"))
    for h in glob.glob(os.path.join(ROOT, "bindings", "c", "include", "*.h")):
        shutil.copy(h, OUT)
    shutil.copy(os.path.join(ROOT, "bindings", "c", "examples", "imu_module.c"), OUT)
    link_cmd = ("arm-none-eabi-gcc " + " ".join(GCC_FLAGS) +
                " your_module.c -Wl,--whole-archive libnobro.a -Wl,--no-whole-archive"
                " -T link.x -T defmt.x -nostartfiles -lm -o firmware.elf")
    with open(os.path.join(OUT, "build.sh"), "w", newline="\n") as f:
        f.write("#!/bin/sh\n# Tier C: C module + prebuilt NobroRTOS runtime, no Rust.\n"
                + link_cmd.replace("your_module.c", "${1:-imu_module.c}") + "\n")
    with open(os.path.join(OUT, "build.cmd"), "w", newline="\r\n") as f:
        f.write("@echo off\r\nrem Tier C: C module + prebuilt NobroRTOS runtime, no Rust.\r\n"
                + link_cmd.replace("your_module.c", "%1") + "\r\n")
    print(f"staged {OUT}: libnobro.a + link.x/defmt.x/memory.x + headers + imu_module.c + build.sh/.cmd")
    return 0


def check() -> int:
    gcc = find_gcc()
    archive = os.path.join(OUT, "libnobro.a")
    if gcc is None or not os.path.exists(archive):
        why = "arm-none-eabi-gcc missing" if gcc is None else "staged archive missing (run --build)"
        print(f"SKIP link test: {why}")
        print("RESULT: PASS (skipped)")
        return 0
    negative_sources = {
        "tierc_init_fail.c": (
            '#include "nobro_app.h"\n'
            "int32_t nobro_app_init(void) { return -11; }\n"
            "int32_t nobro_app_poll(void) { return 0; }\n"
        ),
        "tierc_poll_fail.c": (
            '#include "nobro_app.h"\n'
            "int32_t nobro_app_init(void) { return 0; }\n"
            "int32_t nobro_app_poll(void) { return -12; }\n"
        ),
    }
    for name, source in negative_sources.items():
        with open(os.path.join(OUT, name), "w", newline="\n") as f:
            f.write(source)

    nm = shutil.which("arm-none-eabi-nm") or gcc.replace("gcc", "nm")
    for source in ("imu_module.c", *negative_sources):
        stem = os.path.splitext(source)[0]
        elf = os.path.join(OUT, stem + ".elf")
        cmd = ([gcc] + GCC_FLAGS +
               [os.path.join(OUT, source), "-I", OUT,
                "-Wl,--whole-archive", archive, "-Wl,--no-whole-archive",
                "-T", os.path.join(OUT, "link.x"), "-T", os.path.join(OUT, "defmt.x"),
                "-nostartfiles", "-lm", "-o", elf])
        print("+", " ".join(os.path.basename(c) if os.sep in c else c for c in cmd))
        r = subprocess.run(cmd, cwd=OUT, capture_output=True, text=True)
        if r.returncode:
            print(r.stderr[-1500:])
            print(f"RESULT: FAIL (link {source})")
            return 1
        syms = subprocess.run([nm, elf], capture_output=True, text=True).stdout
        need = ["Reset", "nobro_app_init", "nobro_app_poll", "NOBRO_IMU_HEALTH_REPORT"]
        missing = [s for s in need if s not in syms]
        if missing:
            print("missing symbols:", missing)
            print(f"RESULT: FAIL (symbols {source})")
            return 1
        size = os.path.getsize(elf)
        print(f"linked {os.path.basename(elf)} ({size} bytes); required symbols resolved")
    print("RESULT: PASS")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--build", action="store_true")
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    if args.build:
        rc = build()
        return rc or check()
    return check()


if __name__ == "__main__":
    sys.exit(main())
