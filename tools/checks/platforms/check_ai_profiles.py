#!/usr/bin/env python3
"""Validate public AI admission profiles and portable Q4/Q2 C99 execution."""

from __future__ import annotations

import argparse
import json
import pathlib
import shutil
import subprocess
import tempfile


ROOT = pathlib.Path(__file__).resolve().parents[3]
PROFILES = ROOT / "core" / "boards" / "ai_profiles.json"
HEADER = ROOT / "bindings" / "c" / "include" / "nobro_nn_packed.h"
EXPECTED = {
    "mcs51-kernel-lite", "atmega328p", "samd21", "ra4m1", "nrf52840",
    "rp2040", "rp2350", "esp8266", "esp32", "esp32c3", "esp32s3", "esp32p4",
}
RAM_CEILINGS = {
    "mcs51-kernel-lite": 256, "atmega328p": 1024, "samd21": 12288,
    "ra4m1": 16384, "nrf52840": 32768, "rp2040": 32768,
    "rp2350": 65536, "esp8266": 32768, "esp32": 49152,
    "esp32c3": 65536, "esp32s3": 65536, "esp32p4": 131072,
}
FLASH_CEILINGS = {
    "mcs51-kernel-lite": 8192, "atmega328p": 16384, "samd21": 49152,
    "ra4m1": 98304, "nrf52840": 81920, "rp2040": 65536,
    "rp2350": 131072, "esp8266": 524288, "esp32": 131072,
    "esp32c3": 131072, "esp32s3": 131072, "esp32p4": 262144,
}

C_WITNESS = r'''#include "nobro_nn_packed.h"
int main(void) {
    static const int8_t input[3] = {3, -2, 5};
    static const uint8_t q4[4] = {0xf9, 0x00, 0x71, 0x01};
    static const uint8_t q2[2] = {0x13, 0x07};
    static const uint8_t one_q4[1] = {0x07};
    static const int32_t bias[2] = {4, -3};
    int8_t out[2];
    nobro_nn_packed_receipt_t receipt;
    nobro_nn_packed_quantization_t q = {0, 0, 1073741824L, 1, -128, 127};
    if (nobro_nn_packed_dense(NOBRO_NN_Q4, input, 3,
            nobro_nn_contiguous_blob(q4, sizeof(q4)), bias, 2, q, out,
            &receipt) != NOBRO_NN_OK || out[0] != -15 || out[1] != -9 ||
            receipt.logical_weights != 6) return 1;
    if (nobro_nn_packed_dense(NOBRO_NN_Q2, input, 3,
            nobro_nn_contiguous_blob(q2, sizeof(q2)), bias, 2, q, out,
            &receipt) != NOBRO_NN_OK || out[0] != 6 || out[1] != -8) return 2;
    if (nobro_nn_packed_dense(NOBRO_NN_Q2, input, 3,
            nobro_nn_contiguous_blob(q2, 1), bias, 2, q, out,
            &receipt) != NOBRO_NN_INVALID_PACKED_LENGTH) return 3;
    {
        static const int8_t max_input[1] = {127};
        static const int32_t max_bias[1] = {INT32_MAX};
        if (nobro_nn_packed_dense(NOBRO_NN_Q4, max_input, 1,
                nobro_nn_contiguous_blob(one_q4, 1), max_bias, 1, q, out,
                &receipt) != NOBRO_NN_ACCUMULATOR_OVERFLOW) return 4;
    }
    return 0;
}
'''


def validate_profiles() -> None:
    data = json.loads(PROFILES.read_text(encoding="utf-8"))
    if data.get("schema") != "nobro-ai-admission-profiles-v1":
        raise RuntimeError("AI profile schema drift")
    if data.get("policy", {}).get("raw_measurements") != "private":
        raise RuntimeError("raw measurement boundary drift")
    entries = data.get("profiles", [])
    by_id = {entry.get("mcu"): entry for entry in entries}
    if len(by_id) != len(entries) or set(by_id) != EXPECTED:
        raise RuntimeError(f"AI profile inventory drift: {sorted(by_id)}")
    for mcu, entry in by_id.items():
        if entry.get("formats") != ["q4", "q2"]:
            raise RuntimeError(f"{mcu}: Q4/Q2 format contract drift")
        limits = entry.get("limits", {})
        required = {
            "reserved_flash_bytes", "reserved_static_ram_bytes", "stack_bytes",
            "scratch_bytes", "model_bytes", "input_values", "output_values",
            "macs", "weight_alignment",
        }
        if set(limits) != required or any(
            not isinstance(value, int) or value < 0 for value in limits.values()
        ):
            raise RuntimeError(f"{mcu}: incomplete or invalid budgets")
        if not all(limits[name] > 0 for name in (
            "reserved_flash_bytes", "reserved_static_ram_bytes", "stack_bytes",
            "model_bytes", "input_values", "output_values", "macs",
            "weight_alignment",
        )):
            raise RuntimeError(f"{mcu}: zero required budget")
        if limits["reserved_flash_bytes"] + limits["model_bytes"] > FLASH_CEILINGS[mcu]:
            raise RuntimeError(f"{mcu}: flash reservation exceeds board budget")
        ram = limits["reserved_static_ram_bytes"] + limits["stack_bytes"] + limits["scratch_bytes"]
        if ram > RAM_CEILINGS[mcu]:
            raise RuntimeError(f"{mcu}: RAM reservation exceeds board budget")
        if limits["input_values"] * limits["output_values"] < limits["macs"]:
            raise RuntimeError(f"{mcu}: MAC budget exceeds declared maximum dense shape")
        if not entry.get("unsupported"):
            raise RuntimeError(f"{mcu}: missing explicit unsupported-operation limits")
    mcs51 = by_id["mcs51-kernel-lite"]
    if mcs51.get("support") != "feasibility-only-c99" or "Rust runtime" not in mcs51["unsupported"]:
        raise RuntimeError("8051 profile must remain C99 feasibility-only")


def compile_host() -> None:
    compiler = shutil.which("cc") or shutil.which("gcc") or shutil.which("clang")
    if compiler is None:
        raise RuntimeError("no C99 compiler found for packed-kernel witness")
    with tempfile.TemporaryDirectory(prefix="nobro-ai-") as directory:
        source = pathlib.Path(directory) / "packed_witness.c"
        binary = pathlib.Path(directory) / ("packed_witness.exe" if shutil.which("cmd") else "packed_witness")
        source.write_text(C_WITNESS, encoding="utf-8")
        subprocess.run([
            compiler, "-std=c99", "-Wall", "-Wextra", "-Werror",
            f"-I{HEADER.parent}", str(source), "-o", str(binary),
        ], check=True)
        subprocess.run([str(binary)], check=True)


def compile_sdcc(required: bool) -> None:
    compiler = shutil.which("sdcc")
    if compiler is None:
        if required:
            raise RuntimeError("exact SDCC toolchain required but unavailable")
        return
    with tempfile.TemporaryDirectory(prefix="nobro-ai-8051-") as directory:
        source = pathlib.Path(directory) / "packed_8051.c"
        source.write_text(C_WITNESS, encoding="utf-8")
        subprocess.run([
            compiler, "-mmcs51", "--std-c99", f"-I{HEADER.parent}",
            "-c", str(source), "-o", str(pathlib.Path(directory) / "packed_8051.rel"),
        ], check=True)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--require-sdcc", action="store_true")
    args = parser.parse_args()
    try:
        validate_profiles()
        compile_host()
        compile_sdcc(args.require_sdcc)
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"AI ADMISSION PROFILES: FAIL ({error})")
        return 1
    print("AI ADMISSION PROFILES: PASS (Q4/Q2 C99 + bounded MCU admissions)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
