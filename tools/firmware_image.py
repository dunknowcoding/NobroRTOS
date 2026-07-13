#!/usr/bin/env python3
"""Safe, platform-neutral helpers for building nRF52840 application images.

The helpers retain only endpoint-neutral ELF-to-flash and UF2 packaging behavior.
Device selection, flashing policy, and report collection are outside this utility.
"""

import glob
import os
import struct
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CORE = os.path.join(ROOT, "core")
TARGET_DIR = os.path.join(ROOT, "_work", "cargo-target")
RELEASE = os.path.join(TARGET_DIR, "thumbv7em-none-eabihf", "release")

FLASH_END = 0x00100000
MAX_APP_BYTES = 0x60000
UF2_FAMILY = 0xADA52840
UF2_MAGIC0 = 0x0A324655
UF2_MAGIC1 = 0x9E5D5157
UF2_MAGICEND = 0x0AB16F30
UF2_FLAG_FAMILY = 0x00002000


def llvm_bin():
    """Return the Rust LLVM tools directory without assuming a host OS/triple."""
    sysroot = subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip()
    hits = glob.glob(os.path.join(sysroot, "lib", "rustlib", "*", "bin"))
    if not hits:
        raise RuntimeError("llvm-tools not found; install llvm-tools-preview")
    return hits[0]


def _tool(directory, name):
    suffix = ".exe" if os.name == "nt" else ""
    return os.path.join(directory, name + suffix)


def elf_flash_bytes(elf, llvm_directory):
    """Read only loadable flash bytes from an ELF through Intel HEX records."""
    with tempfile.NamedTemporaryFile("w", suffix=".hex", delete=False) as output:
        hex_path = output.name
    try:
        subprocess.run(
            [_tool(llvm_directory, "llvm-objcopy"), "-O", "ihex", elf, hex_path],
            check=True,
        )
        memory = {}
        base = 0
        with open(hex_path, encoding="ascii") as records:
            for raw in records:
                line = raw.strip()
                if not line.startswith(":"):
                    continue
                record = bytes.fromhex(line[1:])
                offset = (record[1] << 8) | record[2]
                kind = record[3]
                payload = record[4 : 4 + record[0]]
                if kind == 0x00:
                    for index, byte in enumerate(payload):
                        address = base + offset + index
                        if address < FLASH_END:
                            memory[address] = byte
                elif kind == 0x02:
                    base = ((payload[0] << 8) | payload[1]) << 4
                elif kind == 0x04:
                    base = ((payload[0] << 8) | payload[1]) << 16
                elif kind == 0x01:
                    break
        return memory
    finally:
        os.unlink(hex_path)


def flash_image(memory, app_base):
    """Create a guarded contiguous application image without boot regions."""
    if not memory:
        raise ValueError("no flash content parsed from ELF")
    lowest, highest = min(memory), max(memory)
    if lowest < app_base:
        raise ValueError(
            f"image byte at 0x{lowest:X} is below app base 0x{app_base:X}"
        )
    size = highest - app_base + 1
    if size > MAX_APP_BYTES:
        raise ValueError(f"application image is too large: {size} bytes")
    output = bytearray(size)
    for address, byte in memory.items():
        output[address - app_base] = byte
    return app_base, bytes(output)


def make_uf2(memory):
    """Pack flash bytes in nRF52840-family UF2 blocks."""
    if not memory:
        raise ValueError("cannot package an empty image")
    lowest, highest = min(memory), max(memory)
    start = (lowest // 256) * 256
    end = ((highest // 256) + 1) * 256
    pages = [
        (address, bytes(memory.get(address + index, 0) for index in range(256)))
        for address in range(start, end, 256)
    ]
    output = bytearray()
    for index, (address, payload) in enumerate(pages):
        header = struct.pack(
            "<IIIIIIII",
            UF2_MAGIC0,
            UF2_MAGIC1,
            UF2_FLAG_FAMILY,
            address,
            256,
            index,
            len(pages),
            UF2_FAMILY,
        )
        output += header + payload + b"\x00" * 220 + struct.pack("<I", UF2_MAGICEND)
    return bytes(output)
