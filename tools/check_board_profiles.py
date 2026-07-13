#!/usr/bin/env python3
"""Validate categorized data-first board profiles in core/boards/*/*/board.json.

Checks required fields and boot/capacity invariants so a board profile cannot silently
drift from a coherent layout. Pure stdlib; exit 0 = all profiles valid.
"""
import glob, json, os, sys

LAYOUT_FLASH_START = {
    "NoSoftDevice": 0x1000,        # nRF52840, app behind the bootloader
    "SoftDeviceS140V6": 0x26000,   # nRF52840 + S140 v6
    "Esp32IdfApp": 0x10000,        # ESP32 IDF app partition (factory slot)
    "Rp2350ImageDef": 0x10000000,  # RP2350 XIP flash base (IMAGE_DEF block)
}
PLATFORM_RAM = {                   # (ram_start, max_end) sanity window per platform
    "nrf52840": (0x2000_0000, 0x2004_0000),
    "esp32c3": (0x3FC8_0000, 0x3FCE_0000),
    "rp2350": (0x2000_0000, 0x2008_2000),
    "ra4m1": (0x2000_0000, 0x2000_8000),
    "samd21": (0x2000_0000, 0x2000_8000),
    "stm32f4": (0x2000_0000, 0x2002_0000),
    "imxrt1062": (0x2020_0000, 0x2028_0000),
    "cortex_m": (0x2000_0000, 0x2000_8000),
}
ROOT = os.path.join(os.path.dirname(__file__), "..", "core", "boards")

def as_int(v):
    return int(v, 0) if isinstance(v, str) else int(v)

def check(path):
    d = json.load(open(path))
    errs = []
    for k in ("board_id", "platform_id", "feature", "boot", "capacity", "pins"):
        if k not in d:
            errs.append(f"missing '{k}'")
    if errs:
        return errs
    boot, cap = d["boot"], d["capacity"]
    layout = boot.get("layout")
    start = as_int(boot["app_flash_start"])
    if layout in LAYOUT_FLASH_START and start != LAYOUT_FLASH_START[layout]:
        errs.append(f"{layout} app_flash_start should be {LAYOUT_FLASH_START[layout]:#x}, got {start:#x}")
    if cap["flash_budget_bytes"] > as_int(boot["app_flash_len_bytes"]):
        errs.append("flash_budget exceeds app flash region")
    if cap["ram_budget_bytes"] > as_int(boot["ram_len_bytes"]):
        errs.append("ram_budget exceeds ram region")
    plat = d.get("platform_id")
    if plat in PLATFORM_RAM:
        lo, hi = PLATFORM_RAM[plat]
        start = as_int(boot["ram_start"])
        if not (lo <= start and start + as_int(boot["ram_len_bytes"]) <= hi):
            errs.append(f"RAM window out of {plat} range")
    else:
        errs.append(f"unknown platform_id '{plat}'")
    return errs

def main():
    profiles = sorted(glob.glob(os.path.join(ROOT, "*", "*", "board.json")))
    if not profiles:
        print("no board profiles found"); return 1
    bad = 0
    for p in profiles:
        errs = check(p)
        name = "/".join(os.path.normpath(p).split(os.sep)[-3:-1])
        if errs:
            bad += 1
            print(f"[FAIL] {name}: " + "; ".join(errs))
        else:
            d = json.load(open(p))
            print(f"[ OK ] {name}: {d['board_id']} ({d['boot']['layout']}, app@{d['boot']['app_flash_start']})")
    print(f"RESULT: {'PASS' if bad == 0 else 'FAIL'} ({len(profiles)-bad}/{len(profiles)} valid)")
    return 1 if bad else 0

if __name__ == "__main__":
    sys.exit(main())
