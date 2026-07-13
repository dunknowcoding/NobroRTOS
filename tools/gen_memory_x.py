#!/usr/bin/env python3
"""Generate a linker memory.x from a data-first board profile (M91).

Finds a unique core/boards/<platform>/<board>/board.json by board directory name
and emits the MEMORY block (plus the RP2350
IMAGE_DEF sections when the layout needs it), so a new port's linker script derives
from the same single source of truth the validator checks.

Usage: python tools/gen_memory_x.py rp2350-pico2w [--out path]  (default: stdout)
"""
import argparse
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))

RP2350_SECTIONS = """
SECTIONS {
    /* RP2350 boot: the bootrom scans for the IMAGE_DEF block right after the vectors */
    .start_block : ALIGN(4)
    {
        __start_block_addr = .;
        KEEP(*(.start_block));
    } > FLASH
} INSERT AFTER .vector_table;

_stext = ADDR(.start_block) + SIZEOF(.start_block);

SECTIONS {
    .end_block : ALIGN(4)
    {
        __end_block_addr = .;
        KEEP(*(.end_block));
    } > FLASH
} INSERT AFTER .uninit;
"""


def kib(n):
    return f"{n // 1024}K" if n % 1024 == 0 else str(n)


def generate(profile):
    d = json.load(open(profile))
    boot = d["boot"]
    start = boot["app_flash_start"]
    flash_len = int(boot["app_flash_len_bytes"])
    ram_start = boot["ram_start"]
    ram_len = int(boot["ram_len_bytes"])
    out = [
        f"/* GENERATED from {os.path.basename(os.path.dirname(profile))}/board.json",
        f"   ({d['board_id']}, layout {boot['layout']}) - do not edit by hand. */",
        "MEMORY {",
        f"    FLASH : ORIGIN = {start}, LENGTH = {kib(flash_len)}",
        f"    RAM   : ORIGIN = {ram_start}, LENGTH = {kib(ram_len)}",
        "}",
    ]
    text = "\n".join(out) + "\n"
    if boot["layout"] == "Rp2350ImageDef":
        text += RP2350_SECTIONS
    return text


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("board", help="board directory name under core/boards/<platform>/")
    ap.add_argument("--out")
    args = ap.parse_args()
    boards = os.path.join(HERE, "..", "core", "boards")
    matches = []
    for platform in os.listdir(boards):
        candidate = os.path.join(boards, platform, args.board, "board.json")
        if os.path.isfile(candidate):
            matches.append(candidate)
    if len(matches) != 1:
        print(f"expected one profile named {args.board!r}, found {len(matches)}")
        return 1
    profile = matches[0]
    text = generate(profile)
    if args.out:
        with open(args.out, "w", newline="\n") as f:
            f.write(text)
        print(f"wrote {args.out}")
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
