#!/usr/bin/env python3
"""One-command hardware bring-up + eval for NobroRTOS (J-Link boards).

Builds a hardware demo app, flashes it to an nRF52840 over SEGGER J-Link, lets it
run, reads the app's fixed `NOBRO_*` eval report struct out of RAM with `mem32`,
decodes it, and prints PASS/FAIL with every field. This automates the manual flow
in docs/HARDWARE_BRINGUP.md so a newcomer runs a single command:

    python tools/nobro_hw_eval.py imu        # GY-9250 IMU on board1
    python tools/nobro_hw_eval.py sal        # kernel + servo PWM + sensor SAL
    python tools/nobro_hw_eval.py sched      # deadline jitter / PPI latency / PWM
    python tools/nobro_hw_eval.py imu --board board5   # GY-91, S140 layout

Requirements: rustup target thumbv7em-none-eabihf, llvm-tools-preview, and a SEGGER
J-Link with JLink.exe installed (the default Windows driver - no Zadig/WinUSB swap).

Exit code 0 only when the report's all_pass field is 1, so this is CI/task friendly.
"""
import argparse
import glob
import os
import re
import subprocess
import sys
import tempfile

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CORE = os.path.join(REPO, "core")
WORK = os.path.join(REPO, "_work")
TARGET_DIR = os.path.join(WORK, "cargo-target")
RELEASE = os.path.join(TARGET_DIR, "thumbv7em-none-eabihf", "release")

# app -> build + report metadata. Field lists mirror nobro_kernel::eval structs.
COMMON_HEAD = ["magic", "version", "completed", "all_pass"]
APPS = {
    "imu": {
        "package": "imu-i2c-demo",
        "bin": {"board1": "imu_i2c_demo", "board5": "imu_i2c_demo_board5"},
        "symbol": "NOBRO_IMU_HW_EVAL_REPORT",
        "magic": 0x4E424E33,
        "fields": COMMON_HEAD + ["board_id_tag", "who_am_i", "dev_addr", "i2c_devices",
                                 "bmp280_present", "imu_reads", "imu_errors", "accel_mag_mg",
                                 "gyro_mag_mdps", "temp_centi_c", "checksum"],
    },
    "sal": {
        "package": "sal-adapter-demo",
        "bin": {"board1": "sal_adapter_demo"},
        "symbol": "NOBRO_SAL_EVAL_REPORT",
        "magic": 0x4E424E32,
        "fields": COMMON_HEAD + ["servo_steps", "servo_readback_ok", "imu_samples",
                                 "imu_plausible", "checksum"],
    },
    "sched": {
        "package": "resource-sched-demo",
        "bin": {"board1": "resource_sched_demo"},
        "symbol": "NOBRO_EVAL_REPORT",
        "magic": 0x4E424E31,
        "fields": COMMON_HEAD + ["scene_a_pass", "scene_a_max_jitter_us", "scene_a_ticks",
                                 "scene_a_misses", "scene_a_i2c_reads", "scene_b_pass",
                                 "scene_c_pass", "scene_c_max_latency_us", "scene_c_samples",
                                 "scene_d_pass", "scene_d_pwm_hz", "scene_d_pin",
                                 "scene_d_flash_start", "checksum"],
    },
}
BOARD_FLASH = {"board1": 0x1000, "board5": 0x26000}
BOARD_FEATURES = {"board1": [], "board5": ["board-nicenano-s140"]}
EXE = ".exe" if os.name == "nt" else ""


def run(cmd, **kw):
    print("+", " ".join(cmd) if isinstance(cmd, list) else cmd)
    return subprocess.run(cmd, **kw)


def llvm_bin():
    sysroot = subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip()
    hits = glob.glob(os.path.join(sysroot, "lib", "rustlib", "*", "bin"))
    if not hits:
        sys.exit("llvm-tools not found - run: rustup component add llvm-tools-preview")
    return hits[0]


def tool(binbase, name):
    return os.path.join(binbase, name + EXE)


def find_jlink(explicit):
    if explicit:
        return explicit
    for base in (r"C:\Program Files\SEGGER", r"C:\Program Files (x86)\SEGGER"):
        hits = sorted(glob.glob(os.path.join(base, "JLink*", "JLink.exe")))
        if hits:
            return hits[-1]
    sys.exit("JLink.exe not found - pass --jlink C:\\path\\to\\JLink.exe")


def main():
    ap = argparse.ArgumentParser(description="Flash + read a NobroRTOS hardware eval report.")
    ap.add_argument("app", choices=APPS.keys())
    ap.add_argument("--board", choices=BOARD_FLASH.keys(), default="board1")
    ap.add_argument("--run-secs", type=int, default=14)
    ap.add_argument("--jlink", default=None)
    ap.add_argument("--no-build", action="store_true")
    args = ap.parse_args()

    meta = APPS[args.app]
    if args.board not in meta["bin"]:
        sys.exit(f"app '{args.app}' has no {args.board} binary")
    binname = meta["bin"][args.board]
    flash_at = BOARD_FLASH[args.board]
    env = dict(os.environ, CARGO_TARGET_DIR=TARGET_DIR)

    if not args.no_build:
        feats = BOARD_FEATURES[args.board]
        cmd = ["cargo", "build", "-p", meta["package"], "--bin", binname, "--release"]
        if feats:
            cmd += ["--no-default-features", "--features", ",".join(feats)]
        if run(cmd, cwd=CORE, env=env).returncode:
            sys.exit("build failed")

    elf = os.path.join(RELEASE, binname)
    binpath = os.path.join(WORK, f"{binname}.bin")
    lbin = llvm_bin()
    run([tool(lbin, "llvm-objcopy"), "-O", "binary", elf, binpath], check=True)

    nm = subprocess.check_output([tool(lbin, "llvm-nm"), elf], text=True)
    addr = None
    for line in nm.splitlines():
        if line.strip().endswith(meta["symbol"]):
            addr = int(line.split()[0], 16)
            break
    if addr is None:
        sys.exit(f"symbol {meta['symbol']} not found in {elf}")
    print(f"report {meta['symbol']} @ 0x{addr:08X}, flashing {binname} @ 0x{flash_at:X}")

    nwords = len(meta["fields"])
    script = (
        f"si SWD\nspeed 4000\nconnect\n"
        f"loadbin {binpath},0x{flash_at:X}\nr\ng\nsleep {args.run_secs * 1000}\nh\n"
        f"mem32 0x{addr:08X},{nwords:X}\nq\n"
    )
    with tempfile.NamedTemporaryFile("w", suffix=".jlink", delete=False) as f:
        f.write(script)
        jscript = f.name
    jlink = find_jlink(args.jlink)
    out = subprocess.run([jlink, "-device", "nRF52840_xxAA", "-if", "SWD", "-speed",
                          "4000", "-autoconnect", "1", "-nogui", "1", "-CommandFile", jscript],
                         capture_output=True, text=True).stdout
    os.unlink(jscript)

    words = []
    for line in out.splitlines():
        m = re.match(r"\s*[0-9A-Fa-f]{8}\s*=\s*(.+)", line)
        if m:
            words += [int(w, 16) for w in m.group(1).split()]
    words = words[:nwords]
    if len(words) < nwords:
        print(out)
        sys.exit(f"short read: got {len(words)}/{nwords} words (board powered? IMU wired?)")

    fields = dict(zip(meta["fields"], words))
    print(f"\n=== {args.app} on {args.board} ===")
    for name, val in fields.items():
        print(f"  {name:22} = {val} (0x{val:X})")

    ok = fields["magic"] == meta["magic"] and fields["all_pass"] == 1 and fields["completed"] == 1
    print(f"\n{'PASS' if ok else 'FAIL'}: all_pass={fields['all_pass']} "
          f"magic={'ok' if fields['magic'] == meta['magic'] else 'BAD'}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
