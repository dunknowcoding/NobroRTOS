#!/usr/bin/env python3
"""Board provisioning automation (M79): build -> flash -> read report -> verify, one cmd.

Builds an app for thumbv7em, objcopies to a bin, flashes it via J-Link to the app start,
reads its NOBRO_* report, and checks all_pass (report word[3]) == 1. Automates the manual
J-Link bring-up flow. Requires J-Link + a board on SWD.

Usage:
  provision_board.py <crate> <bin> <report_symbol> [--app-start 0x1000]
"""
import argparse
import os
import re
import subprocess
import sys
import time

ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), ".."))
CORE = os.path.join(ROOT, "core")
WORK = os.path.join(ROOT, "_work")
TARGET = "thumbv7em-none-eabihf"
DEVICE = "nRF52840_xxAA"
JLINK = r"C:\Program Files\SEGGER\JLink_V924a\JLink.exe"


def sh(cmd, cwd=None, env=None):
    return subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True)


def tool(name):
    sysroot = sh(["rustc", "--print", "sysroot"]).stdout.strip()
    for d in os.listdir(os.path.join(sysroot, "lib", "rustlib")):
        cand = os.path.join(sysroot, "lib", "rustlib", d, "bin", name + ".exe")
        if os.path.exists(cand):
            return cand
    return name


def jlink(script):
    path = os.path.join(WORK, "provision.jlink")
    open(path, "w").write(script)
    return sh([JLINK, "-device", DEVICE, "-if", "SWD", "-speed", "4000",
               "-autoconnect", "1", "-NoGui", "1", "-CommandFile", path])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("crate")
    ap.add_argument("binname")
    ap.add_argument("symbol")
    ap.add_argument("--app-start", default="0x1000")
    args = ap.parse_args()

    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = os.path.join(WORK, "ct2")

    print(f"[1/4] build {args.crate}")
    r = sh(["cargo", "build", "-p", args.crate, "--release", "--target", TARGET], cwd=CORE, env=env)
    if r.returncode != 0:
        print(r.stderr[-500:])
        return 1

    elf = os.path.join(WORK, "ct2", TARGET, "release", args.binname)
    addr = None
    for line in sh([tool("llvm-nm"), elf]).stdout.splitlines():
        if args.symbol in line:
            addr = "0x" + line.split()[0]
    if not addr:
        print(f"symbol {args.symbol} not found")
        return 1
    bin_path = os.path.join(WORK, args.binname + ".bin")
    sh([tool("llvm-objcopy"), "-O", "binary", elf, bin_path])
    print(f"[2/4] report symbol {args.symbol} @ {addr}")

    print("[3/4] flash via J-Link")
    win_bin = bin_path.replace("/", "\\")
    jlink(f"si SWD\nspeed 4000\nconnect\nhalt\nloadbin {win_bin},{args.app_start}\nr\ng\nq\n")
    time.sleep(2)

    print("[4/4] read + verify report")
    out = jlink(f"si SWD\nspeed 4000\nconnect\nhalt\nmem32 {addr},4\ng\nq\n").stdout
    m = re.search(re.escape(addr[2:].lower()) + r"\s*=\s*([0-9a-fA-F]{8})\s+([0-9a-fA-F]{8})\s+([0-9a-fA-F]{8})\s+([0-9a-fA-F]{8})", out, re.I)
    if not m:
        print("could not read report"); return 1
    magic, _ver, _done, all_pass = (int(m.group(i), 16) for i in (1, 2, 3, 4))
    ok = all_pass == 1
    print(f"   magic={magic:08X} all_pass={all_pass}")
    print(f"RESULT: {'PROVISIONED OK' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
