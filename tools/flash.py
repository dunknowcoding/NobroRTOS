#!/usr/bin/env python3
"""Unified flashing abstraction (M90): one command for every flashing path on the bench.

Backends:
  jlink    - SEGGER J-Link SWD load of a raw .bin at an address (nRF52840 dev board)
  uf2      - copy a .uf2 onto a UF2 bootloader drive (nice!nano-class, Pico 2)
  arduino  - arduino-cli upload of a prebuilt build dir to a COM port (ESP32/AVR/R4)

Each backend builds the exact external command; --dry-run prints it without touching
hardware, so the abstraction is testable everywhere and scriptable in CI.

Examples:
  python3 tools/flash.py jlink --bin _work/kernel_selftest.bin --addr 0x1000
  python3 tools/flash.py uf2 --file firmware.uf2 --drive J:
  python3 tools/flash.py arduino --port <PORT> --fqbn esp32:esp32:esp32c3 --build-dir .build/x
  python3 tools/flash.py all --dry-run   (smoke: show every backend's command)
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile

import os as _os
# Override with the JLINK_EXE env var; the default is the common Windows install path.
JLINK_EXE = _os.environ.get("JLINK_EXE", r"C:\Program Files\SEGGER\JLink\JLink.exe")


def cmd_jlink(args):
    script = (
        "si SWD\nspeed 4000\nconnect\nhalt\n"
        f"loadbin {os.path.abspath(args.bin)},{args.addr}\nr\ng\nq\n"
    )
    if args.dry_run:
        return ([JLINK_EXE, "-device", args.device, "-if", "SWD", "-speed", "4000",
                 "-autoconnect", "1", "-NoGui", "1", "-CommandFile", "<script>"],
                script)
    with tempfile.NamedTemporaryFile("w", suffix=".jlink", delete=False) as f:
        f.write(script)
        path = f.name
    try:
        cmd = [JLINK_EXE, "-device", args.device, "-if", "SWD", "-speed", "4000",
               "-autoconnect", "1", "-NoGui", "1", "-CommandFile", path]
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=120).stdout
        ok = "O.K." in out
        print(out.strip().splitlines()[-1] if out.strip() else "(no output)")
        return ok
    finally:
        os.unlink(path)


def cmd_uf2(args):
    src, dst = os.path.abspath(args.file), os.path.join(args.drive + "\\", "")
    if args.dry_run:
        return (["copy", src, dst], None)
    if not os.path.exists(dst):
        print(f"UF2 drive {args.drive} not present (put the board in bootloader mode)")
        return False
    # Raw byte copy WITHOUT chmod: UF2 bootloaders consume the file and dismount the
    # drive mid-write, so shutil.copy's copymode step would raise even on success.
    data = open(src, "rb").read()
    try:
        with open(os.path.join(dst, os.path.basename(src)), "wb") as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
    except OSError:
        pass  # dismount during/after the final block = the bootloader accepted it
    print(f"copied {os.path.basename(src)} -> {args.drive} ({len(data)} bytes)")
    return True


def cmd_arduino(args):
    cmd = ["arduino-cli", "upload", "-p", args.port, "--fqbn", args.fqbn,
           "--input-dir", args.build_dir]
    if args.dry_run:
        return (cmd, None)
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=300)
    tail = (r.stdout + r.stderr).strip().splitlines()
    print(tail[-1] if tail else "(no output)")
    return r.returncode == 0


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("backend", choices=["jlink", "uf2", "arduino", "all"])
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--bin", default="_work/app.bin")
    ap.add_argument("--addr", default="0x1000")
    ap.add_argument("--device", default="nRF52840_xxAA")
    ap.add_argument("--file", default="firmware.uf2")
    ap.add_argument("--drive", default="J:", help="UF2 bootloader drive letter (yours may differ)")
    ap.add_argument("--port", default=None, help="serial port for arduino uploads")
    ap.add_argument("--fqbn", default="esp32:esp32:esp32c3")
    ap.add_argument("--build-dir", default=".build/app")
    args = ap.parse_args()

    if args.backend == "all":
        args.dry_run = True
        ok = True
        for name, fn in [("jlink", cmd_jlink), ("uf2", cmd_uf2), ("arduino", cmd_arduino)]:
            cmd, extra = fn(args)
            print(f"[{name:7}] {' '.join(cmd)}")
            if extra:
                for line in extra.strip().splitlines():
                    print(f"          | {line}")
        print("RESULT: PASS (dry-run, all backends constructed)")
        return 0 if ok else 1

    fn = {"jlink": cmd_jlink, "uf2": cmd_uf2, "arduino": cmd_arduino}[args.backend]
    result = fn(args)
    if args.dry_run:
        cmd, extra = result
        print(" ".join(cmd))
        if extra:
            print(extra)
        return 0
    print(f"RESULT: {'PASS' if result else 'FAIL'}")
    return 0 if result else 1


if __name__ == "__main__":
    sys.exit(main())
