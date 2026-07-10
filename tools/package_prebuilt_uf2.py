#!/usr/bin/env python3
"""Prebuilt-UF2 bundle for the no-code loop (UX rung 0).

The CircuitPython-class promise: a first-time user flashes ONE prebuilt UF2 by
drag-and-drop, opens the web-flasher report console, and watches the board explain
itself in plain sentences - zero toolchain. This tool builds that bundle and gates it.

  --build   cargo-build usb_cdc_demo_s140, extract a bootloader-safe flash image from
            the ELF (reusing nobro_hw_eval's IHEX/clamp logic), wrap it as UF2
            (family 0xADA52840, app @ 0x26000), and bundle it with the starter
            app.json + a README into _work/prebuilt/
  --check   validate the committed reference manifest (packages/block-editor/
            prebuilt.json): constants match the flash layout, the starter app.json
            passes the catalog validator, the no-code doc exists. If a built UF2 is
            present, structurally verify it too (magic, family, address window).

    python tools/package_prebuilt_uf2.py --build
    python tools/package_prebuilt_uf2.py --check
"""
import argparse
import hashlib
import json
import os
import shutil
import struct
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import nobro_hw_eval as hw  # elf_flash_bytes / flash_image / make_uf2 + constants

MANIFEST = os.path.join(ROOT, "packages", "block-editor", "prebuilt.json")
OUT_DIR = os.path.join(ROOT, "_work", "prebuilt")
APP_BASE_S140 = 0x26000


def load_manifest():
    with open(MANIFEST, encoding="utf-8") as f:
        return json.load(f)


def build() -> int:
    man = load_manifest()
    env = dict(os.environ, CARGO_TARGET_DIR=hw.TARGET_DIR)
    cmd = ["cargo", "build", "-p", man["package"], "--bin", man["binary"], "--release",
           "--no-default-features", "--features", ",".join(man["features"])]
    print("+", " ".join(cmd))
    if subprocess.run(cmd, cwd=hw.CORE, env=env).returncode:
        return 1
    elf = os.path.join(hw.RELEASE, man["binary"])
    mem = hw.elf_flash_bytes(elf, hw.llvm_bin())
    base, image = hw.flash_image(mem, APP_BASE_S140)  # guard: never below app base
    uf2 = hw.make_uf2(mem)
    os.makedirs(OUT_DIR, exist_ok=True)
    uf2_path = os.path.join(OUT_DIR, man["uf2_name"])
    with open(uf2_path, "wb") as f:
        f.write(uf2)
    shutil.copy(os.path.join(ROOT, man["starter_app_json"]),
                os.path.join(OUT_DIR, "app.json"))
    with open(os.path.join(OUT_DIR, "README.txt"), "w", encoding="utf-8") as f:
        f.write(
            "NobroRTOS no-code starter\n"
            "1. Double-tap RESET on your S140-bootloader nRF52840 board (UF2 drive appears).\n"
            f"2. Drag {man['uf2_name']} onto the drive. The board reboots as a USB serial device.\n"
            "3. Open packages/web-flasher/index.html, click 'Open report console', pick the port.\n"
            "   The board explains itself: 'CDC: all checks passing, ...'.\n"
            "4. Design your own app in packages/block-editor (exports app.json - this folder\n"
            "   has the starter). Building that app needs the toolchain; see docs/GETTING_STARTED.md.\n"
        )
    digest = hashlib.sha256(uf2).hexdigest()
    print(f"bundle: {OUT_DIR}")
    print(f"  {man['uf2_name']}: {len(uf2)} bytes ({len(uf2)//512} UF2 blocks), sha256={digest[:16]}..")
    print(f"  image: {len(image)} bytes @ 0x{base:X}")
    print("RESULT: PASS")
    return 0


def check() -> int:
    errors = []
    man = load_manifest()
    if man.get("uf2_family") != hw.UF2_FAMILY:
        errors.append(f"manifest uf2_family 0x{man.get('uf2_family', 0):08X} != 0x{hw.UF2_FAMILY:08X}")
    if man.get("app_base") != APP_BASE_S140:
        errors.append(f"manifest app_base != 0x{APP_BASE_S140:X}")
    if "s140" not in " ".join(man.get("features", [])):
        errors.append("manifest features do not select the S140 layout")
    starter = os.path.join(ROOT, man["starter_app_json"])
    if not os.path.isfile(starter):
        errors.append(f"starter app.json missing: {man['starter_app_json']}")
    else:
        r = subprocess.run([sys.executable, os.path.join(ROOT, "tools", "nobro_app.py"), starter],
                           capture_output=True, text=True)
        if r.returncode:
            errors.append("starter app.json fails the catalog validator")
    if not os.path.isfile(os.path.join(ROOT, "docs", "GETTING_STARTED.md")):
        errors.append("docs/GETTING_STARTED.md missing")

    uf2_path = os.path.join(OUT_DIR, man.get("uf2_name", ""))
    if os.path.isfile(uf2_path):
        data = open(uf2_path, "rb").read()
        if len(data) % 512:
            errors.append("UF2 size not block-aligned")
        for off in range(0, len(data), 512):
            m0, m1, _fl, addr, _len, _i, _n, fam = struct.unpack_from("<8I", data, off)
            if (m0, m1) != (hw.UF2_MAGIC0, hw.UF2_MAGIC1):
                errors.append(f"bad UF2 magic at block {off//512}")
                break
            if fam != hw.UF2_FAMILY:
                errors.append("wrong UF2 family")
                break
            if not (APP_BASE_S140 <= addr < hw.FLASH_END - 0xC000):
                errors.append(f"block addr 0x{addr:X} outside the safe app window")
                break

    for e in errors:
        print("FAIL:", e)
    print(f"RESULT: {'PASS' if not errors else 'FAIL'} "
          f"(manifest + starter validated{'; UF2 verified' if os.path.isfile(uf2_path) else ''})")
    return 1 if errors else 0


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
