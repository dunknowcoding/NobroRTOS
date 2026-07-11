#!/usr/bin/env python3
"""One-command hardware bring-up + eval for NobroRTOS (J-Link profiles).

Builds a hardware demo app, flashes it to an nRF52840 over SEGGER J-Link, lets it
run, reads the app's fixed `NOBRO_*` eval report struct out of RAM with `mem32`,
decodes it, and prints PASS/FAIL with every field. This automates the manual flow
in docs/GETTING_STARTED.md so a newcomer runs a single command:

    python tools/nobro_hw_eval.py imu --profile nosd
    python tools/nobro_hw_eval.py sal        # kernel + servo PWM + sensor SAL
    python tools/nobro_hw_eval.py sched      # deadline jitter / PPI latency / PWM
    python tools/nobro_hw_eval.py imu --profile s140
    python tools/nobro_hw_eval.py udi --profile s140 --backend arduino  # UDI backend swap
    python tools/nobro_hw_eval.py udi --profile s140 --backend arduino --flash uf2

Flashing is bootloader-safe on both layouts (no-SoftDevice app @ 0x1000, S140 app @
0x26000): the app image is extracted from Intel HEX and clamped to the flash window, so
it can never include the MBR (sector 0) or reach the bootloader (~0xF4000). Two paths:
  --flash jlink (default): SWD `loadbin` of the flash-only image, then reset/run/read.
  --flash uf2            : drag-and-drop over the DFU drive (also refreshes bootloader
                           settings); enters DFU via J-Link GPREGRET if needed.
(An earlier `objcopy -O binary` gap-filled apps whose report lives in RAM into a ~512MB
image; loadbin-ing that at the app base erased the bootloader. That path is gone.)

Requirements: rustup target thumbv7em-none-eabihf, llvm-tools-preview, and a SEGGER
J-Link with JLink.exe installed (the default Windows driver - no Zadig/WinUSB swap).

Exit code 0 only when the report's all_pass field is 1, so this is CI/task friendly.
"""
import argparse
import glob
import os
import re
import shutil
import string
import struct
import subprocess
import sys
import tempfile
import time

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
        "bin": {"nosd": "imu_i2c_demo", "s140": "imu_i2c_demo_s140"},
        "symbol": "NOBRO_IMU_HW_EVAL_REPORT",
        "magic": 0x4E424E33,
        "fields": COMMON_HEAD + ["board_id_tag", "who_am_i", "dev_addr", "i2c_devices",
                                 "bmp280_present", "imu_reads", "imu_errors", "accel_mag_mg",
                                 "gyro_mag_mdps", "temp_centi_c", "checksum"],
    },
    "sal": {
        "package": "sal-adapter-demo",
        "bin": {"nosd": "sal_adapter_demo"},
        "symbol": "NOBRO_SAL_EVAL_REPORT",
        "magic": 0x4E424E32,
        "fields": COMMON_HEAD + ["servo_steps", "servo_readback_ok", "imu_samples",
                                 "imu_plausible", "checksum"],
    },
    "eh": {  # IMU read through the embedded-hal I2c trait (driver-compat proof)
        "package": "eh-imu-demo",
        "bin": {"nosd": "eh_imu_demo"},
        "symbol": "NOBRO_IMU_HW_EVAL_REPORT",
        "magic": 0x4E424E33,
        "fields": COMMON_HEAD + ["board_id_tag", "who_am_i", "dev_addr", "i2c_devices",
                                 "bmp280_present", "imu_reads", "imu_errors", "accel_mag_mg",
                                 "gyro_mag_mdps", "temp_centi_c", "checksum"],
    },
    "sched": {
        "package": "resource-sched-demo",
        "bin": {"nosd": "resource_sched_demo"},
        "symbol": "NOBRO_EVAL_REPORT",
        "magic": 0x4E424E31,
        "fields": COMMON_HEAD + ["scene_a_pass", "scene_a_max_jitter_us", "scene_a_ticks",
                                 "scene_a_misses", "scene_a_i2c_reads", "scene_b_pass",
                                 "scene_c_pass", "scene_c_max_latency_us", "scene_c_samples",
                                 "scene_d_pass", "scene_d_pwm_hz", "scene_d_pin",
                                 "scene_d_flash_start", "checksum"],
    },
    # Universal Driver Interface proof: same app + same physical IMU, backend chosen by
    # --backend (native HAL / embedded-hal / Arduino-library shim) behind one ImuSal trait.
    # The report's backend_id (1/2/3) records which transport sealed the PASS.
    "udi": {
        "package": "udi-imu-demo",
        "bin": {"nosd": "udi_imu_demo", "s140": "udi_imu_demo_s140"},
        "symbol": "NOBRO_UDI_IMU_REPORT",
        "magic": 0x4E554449,
        "fields": COMMON_HEAD + ["backend_id", "who_am_i", "accel_mag_mg", "reads",
                                 "errors", "temp_centi_c", "checksum"],
        "backends": True,
    },
    # Measured kernel-op WCET: DWT max cycles per core operation (Wave 17).
    "wcet": {
        "package": "kernel-wcet-demo",
        "bin": {"nosd": "kernel_wcet_demo", "s140": "kernel_wcet_demo_s140"},
        "symbol": "NOBRO_WCET_REPORT",
        "magic": 0x4E574354,
        "fields": COMMON_HEAD + ["iters", "mailbox_cyc", "alarm_cyc", "quota_cyc",
                                 "authorize_cyc", "lease_cyc", "event_flags_cyc",
                                 "critical_section_cyc", "mailbox_worst_cyc",
                                 "alarm_worst_cyc", "checksum"],
    },
    # Negative stack-overflow test (MEM-01): the kernel StackGuardTable must
    # survive shallow recursion, trip + attribute on deep recursion, and re-arm.
    "stack": {
        "package": "stack-guard-demo",
        "bin": {"nosd": "stack_guard_demo", "s140": "stack_guard_demo_s140"},
        "symbol": "NOBRO_STACK_REPORT",
        "magic": 0x4E53474B,
        "fields": COMMON_HEAD + ["intact_after_shallow", "tripped_after_deep",
                                 "attributed_module", "rearmed_ok", "guard_addr",
                                 "checksum"],
    },
    # Negative MPU test (MEM-02): a KernelMpuPlan region must fault exactly once,
    # capture an attributed MpuFaultRecord, and recover cleanly.
    "mpu": {
        "package": "mpu-guard-demo",
        "bin": {"nosd": "mpu_guard_demo", "s140": "mpu_guard_demo_s140"},
        "symbol": "NOBRO_MPU_REPORT",
        "magic": 0x4E4D5055,
        "fields": COMMON_HEAD + ["write_before_ok", "faults_caught", "write_after_ok",
                                 "fault_module", "fault_was_data_access", "fault_addr",
                                 "checksum"],
    },
    # Bounded async executor HW proof: same checks as host unit tests, no HAL.
    "async": {
        "package": "async-exec-demo",
        "bin": {"nosd": "async_exec_demo"},
        "symbol": "NOBRO_ASYNC_EXEC_REPORT",
        "magic": 0x4E424153,
        "fields": COMMON_HEAD + ["spawn_pass", "capacity_pass", "stall_pass",
                                 "rounds_used", "tasks_completed", "checksum"],
    },
    # Transactional typed-database persistence: the runner resets the target repeatedly
    # and requires every post-first boot to recover while the boot counter advances.
    "database": {
        "package": "db-persist-demo",
        "bin": {"nosd": "db_persist"},
        "symbol": "NOBRO_DB_PERSIST_REPORT",
        "magic": 0x4E444250,
        "fields": COMMON_HEAD + ["recovered", "boot_count", "rows", "image_len",
                                 "checksum"],
        "reset_cycles": 3,
    },
}
PROFILE_FLASH = {"nosd": 0x1000, "s140": 0x26000}
PROFILE_FEATURES = {"nosd": [], "s140": ["board-nicenano-s140"]}
# Board (bootloader-layout) feature per profile, used by apps that also pick a backend.
PROFILE_BOARD_FEATURE = {"nosd": "board-promicro-nosd", "s140": "board-nicenano-s140"}
EXE = ".exe" if os.name == "nt" else ""

# nRF52840 code flash is 0..1MB. The bootloader + its settings live near the top
# (~0xF4000+), so we never let a flash image run past this window.
FLASH_END = 0x00100000
MAX_APP_BYTES = 0x60000  # ~384KB; any demo app is far smaller than this
# UF2 (drag-and-drop DFU) container constants for the Adafruit nRF52 bootloader.
UF2_FAMILY = 0xADA52840
UF2_MAGIC0, UF2_MAGIC1, UF2_MAGICEND = 0x0A324655, 0x9E5D5157, 0x0AB16F30
UF2_FLAG_FAMILY = 0x00002000
# POWER->GPREGRET; 0x57 is the bootloader's "stay in UF2 DFU" magic (self double-tap).
GPREGRET = 0x4000051C
DFU_MAGIC_UF2 = 0x57


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
    """Locate the J-Link CLI from an explicit argument, PATH, or standard Unix locations."""
    if explicit:
        return explicit
    patterns = [
        "/opt/SEGGER/JLink*/JLinkExe",
        "/opt/SEGGER/JLink/JLinkExe",
        "/Applications/SEGGER/JLink*/JLinkExe",
    ]
    for pat in patterns:
        hits = sorted(glob.glob(pat))
        if hits:
            return hits[-1]
    for name in ("JLinkExe", "JLink.exe", "JLink"):
        found = shutil.which(name)
        if found:
            return found
    sys.exit("J-Link CLI not found - pass --jlink <path to JLink.exe / JLinkExe>. "
             "(probe-rs users: flash/read with probe-rs instead; see docs/GETTING_STARTED.md.)")


def elf_flash_bytes(elf, lbin):
    """Return {addr: byte} for the ELF's *flash* content only (addr < FLASH_END).

    We go through Intel HEX rather than `objcopy -O binary`: a raw binary is gap-filled
    from the lowest to the highest section LMA, so an app whose report / .uninit lands at
    a RAM address (0x2000_xxxx) balloons to a ~512MB image. `loadbin`-ing that at the app
    base then runs straight over the bootloader and bricks the board. HEX carries only
    real records; we additionally drop anything outside the flash window as a hard net.
    Works for both layouts (nosd app @ 0x1000, s140 app @ 0x26000)."""
    with tempfile.NamedTemporaryFile("w", suffix=".hex", delete=False) as f:
        hexpath = f.name
    run([tool(lbin, "llvm-objcopy"), "-O", "ihex", elf, hexpath], check=True)
    mem, base = {}, 0
    for line in open(hexpath):
        line = line.strip()
        if not line.startswith(":"):
            continue
        b = bytes.fromhex(line[1:])
        n, off, rt, payload = b[0], (b[1] << 8) | b[2], b[3], b[4:4 + b[0]]
        if rt == 0x00:                                    # data
            for i, byte in enumerate(payload):
                a = base + off + i
                if a < FLASH_END:
                    mem[a] = byte
        elif rt == 0x02:                                  # extended segment
            base = ((payload[0] << 8) | payload[1]) << 4
        elif rt == 0x04:                                  # extended linear
            base = ((payload[0] << 8) | payload[1]) << 16
        elif rt == 0x01:                                  # EOF
            break
    os.unlink(hexpath)
    return mem


def flash_image(mem, flash_at):
    """Contiguous (base, bytes) for the flash content, with guards that make it
    impossible to hand `loadbin` an image that would reach into the bootloader.
    The MBR (sector 0) is never included, so `loadbin` at 0x1000/0x26000 leaves it
    intact on both bootloader layouts."""
    if not mem:
        sys.exit("no flash content parsed from ELF")
    lo, hi = min(mem), max(mem)
    if lo < flash_at:
        sys.exit(f"image byte at 0x{lo:X} is below the app base 0x{flash_at:X} "
                 "(would overwrite MBR/bootloader) - aborting")
    size = hi - flash_at + 1
    if size > MAX_APP_BYTES:
        sys.exit(f"refusing to flash oversized image ({size} bytes > "
                 f"0x{MAX_APP_BYTES:X}); check ELF sections")
    buf = bytearray(size)
    for a, byte in mem.items():
        buf[a - flash_at] = byte
    return flash_at, bytes(buf)


def make_uf2(mem):
    """Pack flash bytes into a UF2 for the Adafruit nRF52840 bootloader. The bootloader
    validates targetAddr per block and only writes the app region, so this never touches
    the MBR or the bootloader itself - the safe path for both nosd and s140 boards."""
    lo, hi = min(mem), max(mem)
    start, end = (lo // 256) * 256, ((hi // 256) + 1) * 256
    pages = [(a, bytes(mem.get(a + i, 0) for i in range(256)))
             for a in range(start, end, 256)]
    out = bytearray()
    for i, (addr, chunk) in enumerate(pages):
        hdr = struct.pack("<IIIIIIII", UF2_MAGIC0, UF2_MAGIC1, UF2_FLAG_FAMILY,
                          addr, 256, i, len(pages), UF2_FAMILY)
        out += hdr + chunk + b"\x00" * (476 - 256) + struct.pack("<I", UF2_MAGICEND)
    return bytes(out)


def dfu_drives():
    """Mounted UF2 bootloader volumes as [(path, info_text)] on Win/Linux/macOS."""
    hits = []
    if os.name == "nt":
        for d in string.ascii_uppercase:
            info = f"{d}:\\INFO_UF2.TXT"
            if os.path.exists(info):
                hits.append((f"{d}:\\", open(info, errors="ignore").read()))
    else:
        for root in ("/media", "/run/media", "/Volumes"):
            for dirpath, _dirs, files in os.walk(root):
                if "INFO_UF2.TXT" in files:
                    p = os.path.join(dirpath, "INFO_UF2.TXT")
                    hits.append((dirpath, open(p, errors="ignore").read()))
    return hits


def jlink_script(jlink, body):
    """Run a J-Link command body; return stdout. Read-only bodies never risk the
    bootloader, and we always ExitOnError so a wedged probe can't hang forever."""
    with tempfile.NamedTemporaryFile("w", suffix=".jlink", delete=False) as f:
        f.write(f"si SWD\nspeed 4000\nconnect\n{body}\nqc\n")
        path = f.name
    try:
        out = subprocess.run(
            [jlink, "-device", "nRF52840_xxAA", "-if", "SWD", "-speed", "4000",
             "-autoconnect", "1", "-nogui", "1", "-ExitOnError", "1", "-CommandFile", path],
            capture_output=True, text=True, timeout=180).stdout
    finally:
        os.unlink(path)
    return out


def enter_dfu(jlink):
    """Force the Adafruit bootloader into UF2 DFU (self double-tap): set the GPREGRET
    magic and reset. Same mechanism on nosd and s140 boards."""
    jlink_script(jlink, f"w4 0x{GPREGRET:08X} 0x{DFU_MAGIC_UF2:02X}\nr\ng")


def read_report(jlink, addr, nwords, run_ms):
    """Reset-run the flashed app, halt, and read the report struct (read-only)."""
    out = jlink_script(jlink, f"r\ng\nsleep {run_ms}\nh\nmem32 0x{addr:08X},{nwords:X}")
    words = []
    for line in out.splitlines():
        m = re.match(r"\s*[0-9A-Fa-f]{8}\s*=\s*(.+)", line)
        if m:
            words += [int(w, 16) for w in m.group(1).split()]
    return words[:nwords], out


def main():
    ap = argparse.ArgumentParser(description="Flash + read a NobroRTOS hardware eval report.")
    ap.add_argument("app", choices=APPS.keys())
    ap.add_argument("--profile", choices=PROFILE_FLASH.keys(), default="nosd")
    ap.add_argument("--backend", choices=["native", "eh", "arduino"], default="native",
                    help="IMU transport for backend-selectable apps (udi): native HAL, "
                         "embedded-hal, or the Arduino-library shim")
    ap.add_argument("--run-secs", type=int, default=14)
    ap.add_argument("--jlink", default=None)
    ap.add_argument("--no-build", action="store_true")
    ap.add_argument("--json-out", default=None, metavar="PATH",
                    help="also write the decoded report as JSON (fleet-evidence input, "
                         "e.g. _work/evidence/hw/udi_eh.json)")
    ap.add_argument("--flash", choices=["jlink", "uf2"], default="jlink",
                    help="jlink: SWD loadbin (flash-only image, MBR/bootloader-safe on "
                         "both nosd @0x1000 and s140 @0x26000). uf2: drag-and-drop over "
                         "the DFU drive (also updates bootloader settings; needs the "
                         "board in DFU or a J-Link to enter it).")
    args = ap.parse_args()

    meta = APPS[args.app]
    if args.profile not in meta["bin"]:
        sys.exit(f"app '{args.app}' has no {args.profile} binary")
    binname = meta["bin"][args.profile]
    flash_at = PROFILE_FLASH[args.profile]
    env = dict(os.environ, CARGO_TARGET_DIR=TARGET_DIR)

    if not args.no_build:
        cmd = ["cargo", "build", "-p", meta["package"], "--bin", binname, "--release"]
        if meta.get("backends"):
            # Backend-selectable app: pin both the bootloader layout and the transport.
            feats = [PROFILE_BOARD_FEATURE[args.profile], f"backend-{args.backend}"]
            cmd += ["--no-default-features", "--features", ",".join(feats)]
        else:
            feats = PROFILE_FEATURES[args.profile]
            if feats:
                cmd += ["--no-default-features", "--features", ",".join(feats)]
        if run(cmd, cwd=CORE, env=env).returncode:
            sys.exit("build failed")

    elf = os.path.join(RELEASE, binname)
    lbin = llvm_bin()

    nm = subprocess.check_output([tool(lbin, "llvm-nm"), elf], text=True)
    addr = None
    for line in nm.splitlines():
        if line.strip().endswith(meta["symbol"]):
            addr = int(line.split()[0], 16)
            break
    if addr is None:
        sys.exit(f"symbol {meta['symbol']} not found in {elf}")

    # Flash-only image (never the MBR at sector 0, never past the bootloader).
    mem = elf_flash_bytes(elf, lbin)
    base, img = flash_image(mem, flash_at)
    print(f"report {meta['symbol']} @ 0x{addr:08X}, app 0x{base:X}-0x{base + len(img):X} "
          f"({len(img)} bytes) via {args.flash}")

    jlink = find_jlink(args.jlink)
    nwords = len(meta["fields"])
    run_ms = args.run_secs * 1000

    if args.flash == "uf2":
        uf2path = os.path.join(WORK, f"{binname}.uf2")
        with open(uf2path, "wb") as f:
            f.write(make_uf2(mem))
        if not dfu_drives():
            print("no DFU drive present - entering DFU via J-Link GPREGRET...")
            enter_dfu(jlink)
            for _ in range(15):
                time.sleep(1)
                if dfu_drives():
                    break
        drives = dfu_drives()
        if not drives:
            sys.exit("no UF2 DFU drive found (double-tap reset, or check the board)")
        drive = drives[0][0]
        print(f"copying {os.path.basename(uf2path)} -> {drive}")
        shutil.copy(uf2path, drive)
        time.sleep(args.run_secs + 4)  # bootloader flashes, resets, app runs
        words, out = read_report(jlink, addr, nwords, run_ms)
    else:  # jlink: bootloader-safe loadbin of the flash-only image
        binpath = os.path.join(WORK, f"{binname}.bin")
        with open(binpath, "wb") as f:
            f.write(img)
        out = jlink_script(jlink, f"loadbin {binpath},0x{base:X}\nr\ng\n"
                                  f"sleep {run_ms}\nh\nmem32 0x{addr:08X},{nwords:X}")
        words = []
        for line in out.splitlines():
            m = re.match(r"\s*[0-9A-Fa-f]{8}\s*=\s*(.+)", line)
            if m:
                words += [int(w, 16) for w in m.group(1).split()]
        words = words[:nwords]

    if len(words) < nwords:
        print(out)
        sys.exit(f"short read: got {len(words)}/{nwords} words (board powered? IMU wired?)")

    reports = [dict(zip(meta["fields"], words))]
    for _ in range(1, meta.get("reset_cycles", 1)):
        cycle_words, cycle_out = read_report(jlink, addr, nwords, run_ms)
        if len(cycle_words) < nwords:
            print(cycle_out)
            sys.exit(f"short read after reset: got {len(cycle_words)}/{nwords} words")
        reports.append(dict(zip(meta["fields"], cycle_words)))

    fields = reports[-1]
    label = f"{args.app} on {args.profile}"
    if meta.get("backends"):
        label += f" (backend={args.backend})"
    print(f"\n=== {label} ===")
    for name, val in fields.items():
        print(f"  {name:22} = {val} (0x{val:X})")

    ok = all(report["magic"] == meta["magic"]
             and report["all_pass"] == 1
             and report["completed"] == 1
             for report in reports)
    if meta.get("reset_cycles"):
        ok = (ok
              and all(report["recovered"] == 1 for report in reports[1:])
              and all(later["boot_count"] > earlier["boot_count"]
                      for earlier, later in zip(reports, reports[1:])))
        print("  reset_boot_counts      =", [report["boot_count"] for report in reports])
    print(f"\n{'PASS' if ok else 'FAIL'}: all_pass={fields['all_pass']} "
          f"magic={'ok' if fields['magic'] == meta['magic'] else 'BAD'}")

    if args.json_out:
        import json
        record = {"app": args.app, "profile": args.profile,
                  "backend": getattr(args, "backend", None),
                  "ok": ok, "all_pass": fields.get("all_pass"), "fields": fields,
                  "reset_reports": reports if len(reports) > 1 else None}
        os.makedirs(os.path.dirname(os.path.abspath(args.json_out)), exist_ok=True)
        with open(args.json_out, "w", encoding="utf-8") as f:
            json.dump(record, f, indent=2)
        print(f"json: {args.json_out}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
