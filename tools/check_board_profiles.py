#!/usr/bin/env python3
"""Validate data-first board profiles in core/boards/*/board.json.

Checks required fields and boot/capacity invariants so a board profile cannot silently
drift from a coherent layout. Pure stdlib; exit 0 = all profiles valid.
"""
import glob, json, os, sys

LAYOUT_FLASH_START = {"NoSoftDevice": 0x1000, "SoftDeviceS140V6": 0x26000}
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
    if not (0 <= as_int(boot["ram_start"]) <= 0x2004_0000):
        errs.append("ram_start out of nRF52840 RAM range")
    return errs

def main():
    profiles = sorted(glob.glob(os.path.join(ROOT, "*", "board.json")))
    if not profiles:
        print("no board profiles found"); return 1
    bad = 0
    for p in profiles:
        errs = check(p)
        name = os.path.basename(os.path.dirname(p))
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
