#!/usr/bin/env python3
"""Convert a raw firmware .bin to UF2 for drag-and-drop / scripted bootloader flashing.

Families: rp2350-arm-s (0xE48BFF59), rp2040 (0xE48BFF56), nrf52840 (0xADA52840).
Usage: python tools/bin2uf2.py app.bin app.uf2 --base 0x10000000 --family rp2350-arm-s
"""
import argparse
import struct
import sys

MAGIC0, MAGIC1, MAGIC_END = 0x0A324655, 0x9E5D5157, 0x0AB16F30
FLAG_FAMILY_ID = 0x00002000
FAMILIES = {
    "rp2350-arm-s": 0xE48BFF59,
    "rp2040": 0xE48BFF56,
    "nrf52840": 0xADA52840,
}


def convert(data, base, family_id):
    chunks = [data[i:i + 256] for i in range(0, len(data), 256)]
    total = len(chunks)
    out = bytearray()
    for i, chunk in enumerate(chunks):
        payload = chunk + b"\x00" * (256 - len(chunk))
        block = struct.pack(
            "<IIIIIIII", MAGIC0, MAGIC1, FLAG_FAMILY_ID, base + i * 256,
            256, i, total, family_id)
        block += payload + b"\x00" * (476 - 256)
        block += struct.pack("<I", MAGIC_END)
        assert len(block) == 512
        out += block
    return bytes(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("src")
    ap.add_argument("dst")
    ap.add_argument("--base", type=lambda x: int(x, 0), required=True)
    ap.add_argument("--family", choices=sorted(FAMILIES), required=True)
    args = ap.parse_args()
    data = open(args.src, "rb").read()
    uf2 = convert(data, args.base, FAMILIES[args.family])
    open(args.dst, "wb").write(uf2)
    print(f"{args.dst}: {len(data)} bytes -> {len(uf2)//512} UF2 blocks "
          f"(base 0x{args.base:08X}, family {args.family})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
