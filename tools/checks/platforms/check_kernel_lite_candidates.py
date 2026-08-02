#!/usr/bin/env python3
"""Fail closed if candidate-only architecture witnesses imply support."""

from __future__ import annotations

import json
import pathlib
import sys


ROOT = pathlib.Path(__file__).resolve().parents[3]
REGISTRY = ROOT / "core" / "boards" / "candidate_families.json"
SOURCE = (ROOT / "core" / "ports" / "kernel_lite_candidates" /
          "nobro_kernel_lite.c")

EXPECTED = {
    "avr-dx", "attiny-modern", "ch32v003", "stm32c0", "mcs51",
    "stm8s103f3", "pic16f18855", "pic24fj64ga002",
    "pic32mx270f256b", "msp430g2553", "c2000-f280049c",
    "fpga-bounded-dispatch",
}

EXPECTED_TOOLCHAINS = {
    "avr-dx": "not-run",
    "attiny-modern": "not-run",
    "ch32v003": "xPack GNU RISC-V Embedded GCC 15.2.0",
    "stm32c0": "arm-none-eabi-gcc 13.4.0",
    "mcs51": "SDCC 4.5.0 #15242; IAR 8051 V10.20.1.5333 secondary",
    "stm8s103f3": "SDCC 4.5.0 #15242",
    "pic16f18855": "MPLAB XC8 V3.00",
    "pic24fj64ga002": "XC16 Microchip v2.10",
    "pic32mx270f256b": "Microchip XC32 Compiler v6.00; PIC32MX_DFP 1.7.380",
    "msp430g2553": "msp430-gcc 9.3.1.11",
    "c2000-f280049c": "TI C2000 compiler 25.11.1",
    "fpga-bounded-dispatch": "Icarus Verilog 13.0",
}


def fail(message: str) -> None:
    raise RuntimeError(message)


def validate() -> None:
    registry = json.loads(REGISTRY.read_text(encoding="utf-8"))
    if registry.get("schema") != "nobro-candidate-families-v2":
        fail("candidate schema drift")
    entries = registry.get("families", [])
    families = {entry.get("id"): entry for entry in entries}
    if len(entries) != len(families):
        fail("candidate inventory contains duplicate ids")
    if set(families) != EXPECTED:
        fail(f"candidate inventory drift: {sorted(families)}")
    for name, entry in families.items():
        if entry.get("state") not in {"candidate-not-supported", "feasibility-only"}:
            fail(f"{name} has promoted state in the candidate registry")
        if entry.get("claims") != []:
            fail(f"{name} improperly exposes capability claims")
        contract = entry.get("compile_contract", {})
        if (contract.get("exact_device_required") is not True or
                contract.get("exact_core_version_required") is not True):
            fail(f"{name} lacks exact device/toolchain intake")
        if contract.get("toolchain_revision") != EXPECTED_TOOLCHAINS[name]:
            fail(f"{name} exact toolchain revision drift")
        upstream = entry.get("upstream", {})
        if not str(upstream.get("url", "")).startswith("https://"):
            fail(f"{name} lacks primary HTTPS provenance")
    if families["mcs51"]["compile_contract"].get("status") != (
            "no-maintained-rust-target-admitted"):
        fail("8051 must not imply a maintained Rust target")
    if families["c2000-f280049c"]["compile_contract"].get(
            "storage_unit_bits") != 16:
        fail("C28x 16-bit addressable storage-unit boundary drift")
    source = SOURCE.read_text(encoding="utf-8")
    for required in ("NOBRO_TARGET_ID", "CHAR_BIT", "deadline_reached",
                     "push_drop_oldest", "crc8_07"):
        if required not in source:
            fail(f"candidate witness is missing {required}")


def main() -> int:
    try:
        validate()
    except (OSError, RuntimeError, ValueError) as error:
        print(f"KERNEL-LITE CANDIDATES: FAIL ({error})")
        return 1
    print("KERNEL-LITE CANDIDATES: PASS (candidate-only; no board/HAL claims)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
