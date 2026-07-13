#!/usr/bin/env python3
"""RAM-only, state-restoring runtime comparison for the Wave-59 workload.

No application flash is written. Each ELF is linked wholly into nRF52840 RAM,
loaded through J-Link, run for the same wall interval, and inspected for its
common report plus instrumented DWT task-work cycles. The target is then forced back
to its pre-existing UF2 DFU bootloader state even after a failure.

Direct current/joules are deliberately not claimed. ``energy_index`` is a
software estimate in arbitrary units with explicit coefficients.
"""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASE = ROOT / "core" / "baselines"
WORK = ROOT / "_work" / "runtime-comparison"
TARGET = "thumbv7em-none-eabihf"
RAM_LO = 0x2000_0000
RAM_HI = 0x2004_0000
VTOR = 0xE000_ED08
DEMCR = 0xE000_EDFC
DWT_CTRL = 0xE000_1000
DWT_CYCCNT = 0xE000_1004
CPU_HZ = 64_000_000

IMPLEMENTATIONS = (
    "baremetal-complex",
    "nobro-graph-complex",
    "embassy-complex",
    "embassy-complex-tuned",
    "freertos-complex",
)


def llvm_tool(name: str) -> str:
    from measure_baselines import find_llvm_tool
    return find_llvm_tool(name)


def build(name: str) -> pathlib.Path:
    directory_name = "embassy-complex" if name == "embassy-complex-tuned" else name
    binary_name = directory_name
    directory = BASE / directory_name
    target_dir = WORK / "build" / name
    env = dict(
        os.environ,
        NOBRO_RAM_RUN="1",
        CARGO_TARGET_DIR=str(target_dir),
        CARGO_PROFILE_RELEASE_DEBUG="2",
    )
    command = ["cargo", "build", "--release"]
    if name.startswith("embassy-complex"):
        command += ["--features", "runtime-trace"]
    if name == "embassy-complex-tuned":
        command += ["--features", "arena-1024"]
    completed = subprocess.run(
        command, cwd=directory, env=env,
        text=True, capture_output=True,
    )
    if completed.returncode:
        raise RuntimeError(f"{name} RAM build failed\n{completed.stderr[-4000:]}")
    built = target_dir / TARGET / "release" / binary_name
    loadable = target_dir / TARGET / "release" / f"{name}.elf"
    shutil.copy2(built, loadable)
    return loadable


def ihex_memory(elf: pathlib.Path) -> dict[int, int]:
    with tempfile.NamedTemporaryFile(suffix=".hex", delete=False) as output:
        hex_path = pathlib.Path(output.name)
    try:
        subprocess.run(
            [llvm_tool("llvm-objcopy"), "-O", "ihex", str(elf), str(hex_path)],
            check=True, capture_output=True,
        )
        memory: dict[int, int] = {}
        base = 0
        for line in hex_path.read_text(encoding="ascii").splitlines():
            raw = bytes.fromhex(line[1:])
            count = raw[0]
            offset = (raw[1] << 8) | raw[2]
            kind = raw[3]
            payload = raw[4:4 + count]
            if kind == 0:
                memory.update((base + offset + i, byte) for i, byte in enumerate(payload))
            elif kind == 2:
                base = int.from_bytes(payload, "big") << 4
            elif kind == 4:
                base = int.from_bytes(payload, "big") << 16
        return memory
    finally:
        hex_path.unlink(missing_ok=True)


def word(memory: dict[int, int], address: int) -> int:
    return struct.unpack("<I", bytes(memory[address + i] for i in range(4)))[0]


def symbol_address(elf: pathlib.Path, symbol: str) -> int:
    output = subprocess.run(
        [llvm_tool("llvm-nm"), str(elf)], check=True, capture_output=True, text=True
    ).stdout
    for line in output.splitlines():
        fields = line.split()
        if len(fields) >= 3 and fields[-1] == symbol:
            return int(fields[0], 16)
    raise RuntimeError(f"{symbol} missing from {elf.name}")


def optional_symbol_address(elf: pathlib.Path, symbol: str) -> int | None:
    try:
        return symbol_address(elf, symbol)
    except RuntimeError:
        return None


def parse_words(output: str) -> dict[int, int]:
    values: dict[int, int] = {}
    for line in output.splitlines():
        match = re.match(r"\s*([0-9A-Fa-f]{8})\s*=\s*(.*)", line)
        if not match:
            continue
        address = int(match.group(1), 16)
        for token in match.group(2).split():
            if re.fullmatch(r"[0-9A-Fa-f]{8}", token):
                values[address] = int(token, 16)
                address += 4
    return values


def validate_ram_image(memory: dict[int, int]) -> tuple[int, int]:
    if not memory:
        raise RuntimeError("RAM ELF contains no loadable bytes")
    low, high = min(memory), max(memory) + 1
    if low != RAM_LO or high > RAM_HI:
        raise RuntimeError(
            f"refusing non-RAM image 0x{low:08X}-0x{high:08X}; flash must never be touched"
        )
    stack, reset = word(memory, RAM_LO), word(memory, RAM_LO + 4)
    if not (RAM_LO < stack <= RAM_HI and RAM_LO <= (reset & ~1) < RAM_HI):
        raise RuntimeError(f"invalid RAM vectors SP=0x{stack:08X} PC=0x{reset:08X}")
    return stack, reset


def run_one(jlink: str, elf: pathlib.Path, run_ms: int) -> dict:
    import nobro_hw_eval as hw
    from measure_baselines import elf_sizes

    memory = ihex_memory(elf)
    stack, reset = validate_ram_image(memory)
    report = symbol_address(elf, "BASELINE_REPORT")
    busy_address = symbol_address(elf, "RUNTIME_BUSY_CYCLES")
    task_stack_address = optional_symbol_address(elf, "RUNTIME_PEAK_TASK_STACK_BYTES")
    heap_start = symbol_address(elf, "__sheap")
    if not RAM_LO <= report < RAM_HI - 16:
        raise RuntimeError("report is outside RAM")
    if not RAM_LO <= heap_start < stack:
        raise RuntimeError("invalid stack-canary range")
    pattern_path = WORK / "raw" / f"{elf.name}.stack-pattern.bin"
    snapshot_path = WORK / "raw" / f"{elf.name}.stack-snapshot.bin"
    pattern_path.parent.mkdir(parents=True, exist_ok=True)
    pattern = bytes([0xA5]) * (stack - heap_start)
    pattern_path.write_bytes(pattern)
    command_lines = [
        "h",
        f"loadfile {elf}",
        f"loadfile {pattern_path}, 0x{heap_start:08X}",
        f"w4 0x{VTOR:08X} 0x{RAM_LO:08X}",
        f"w4 0x{DEMCR:08X} 0x01000000",
        f"w4 0x{DWT_CYCCNT:08X} 0",
        f"w4 0x{DWT_CTRL:08X} 1",
        f"wreg MSP, 0x{stack:08X}",
        f"wreg PSP, 0x{stack:08X}",
        f"setpc 0x{reset:08X}",
        "g",
        f"sleep {run_ms}",
        "h",
        f"mem32 0x{report:08X},4",
        f"mem32 0x{busy_address:08X},1",
    ]
    if task_stack_address is not None:
        command_lines.append(f"mem32 0x{task_stack_address:08X},1")
    command_lines += [
        f"mem32 0x{DWT_CYCCNT:08X},1",
        f"savebin {snapshot_path}, 0x{heap_start:08X}, 0x{stack - heap_start:X}",
    ]
    commands = "\n".join(command_lines)
    output = hw.jlink_script(jlink, commands)
    raw_dir = WORK / "raw"
    raw_dir.mkdir(parents=True, exist_ok=True)
    (raw_dir / f"{elf.name}.log").write_text(output, encoding="utf-8")
    values = parse_words(output)
    report_words = [values.get(report + 4 * i) for i in range(4)]
    elapsed_cycles = values.get(DWT_CYCCNT)
    busy_cycles = values.get(busy_address)
    if elapsed_cycles is None or busy_cycles is None or any(value is None for value in report_words):
        raise RuntimeError(f"incomplete J-Link evidence for {elf.name}")
    snapshot = snapshot_path.read_bytes()
    changed = [index for index, value in enumerate(snapshot) if value != 0xA5]
    main_stack_peak = 0 if not changed else stack - (heap_start + min(changed))
    expected = {
        "control": run_ms / 20,
        "fusion": run_ms / 10,
        "radio": run_ms / 50,
    }
    observed = {
        "control": report_words[0],
        "fusion": report_words[1],
        "radio": report_words[2],
    }
    shortfalls = {
        key: max(0, round(expected[key]) - int(observed[key])) for key in expected
    }
    active_ratio = min(1.0, busy_cycles / max(1, elapsed_cycles))
    # Explicit estimate only: active cycle = 1 unit, idle/sleep cycle = 0.1 unit.
    energy_index = active_ratio + 0.1 * (1.0 - active_ratio)
    result = {
        "report": report_words,
        "static_ram_bytes": elf_sizes(elf)["static_ram"],
        "elapsed_cycles": elapsed_cycles,
        "busy_cycles": busy_cycles,
        "active_ratio": round(active_ratio, 6),
        "energy_index_estimate": round(energy_index, 6),
        "estimate_coefficients": {"active": 1.0, "idle_or_sleep": 0.1},
        "release_shortfalls": shortfalls,
        "main_stack_peak_bytes": main_stack_peak,
        "ram_image_range": [f"0x{min(memory):08X}", f"0x{max(memory)+1:08X}"],
    }
    if task_stack_address is not None:
        result["task_stack_peak_bytes"] = values.get(task_stack_address)
        result["task_stack_reserved_bytes"] = 5 * 128 * 4 + 96 * 4
    return result


def selftest() -> int:
    sample = "20020000 = 00000001 00000002\nE0001004 = 00000003\n"
    assert parse_words(sample) == {
        0x20020000: 1, 0x20020004: 2, DWT_CYCCNT: 3
    }
    memory = {RAM_LO + i: byte for i, byte in enumerate(
        struct.pack("<II", RAM_HI, RAM_LO + 0x101)
    )}
    assert validate_ram_image(memory) == (RAM_HI, RAM_LO + 0x101)
    print("COMPLEX RUNTIME SELFTEST: PASS")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--build-only", action="store_true")
    parser.add_argument("--run-ms", type=int, default=5000)
    parser.add_argument("--jlink")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    built = {name: build(name) for name in IMPLEMENTATIONS}
    for elf in built.values():
        validate_ram_image(ihex_memory(elf))
    if args.build_only:
        print(f"COMPLEX RUNTIME BUILD: PASS ({len(IMPLEMENTATIONS)} RAM-only ELFs)")
        return 0

    import nobro_hw_eval as hw
    jlink = hw.find_jlink(args.jlink)
    initial_dfu = bool(hw.dfu_drives())
    if not initial_dfu:
        raise SystemExit("protected target is not initially in DFU; refusing runtime campaign")
    results = {}
    restored = False
    try:
        for name, elf in built.items():
            results[name] = run_one(jlink, elf, args.run_ms)
    finally:
        hw.enter_dfu(jlink)
        for _ in range(30):
            if hw.dfu_drives():
                restored = True
                break
            time.sleep(1)
    spreads = {
        field: max(result["report"][index] for result in results.values())
        - min(result["report"][index] for result in results.values())
        for index, field in enumerate(("control", "fusion", "radio", "drops"))
    }
    equivalent = (
        spreads["control"] <= 4
        and spreads["fusion"] <= 6
        and spreads["radio"] <= 4
        and spreads["drops"] <= 8
        and all(not any(result["release_shortfalls"].values()) for result in results.values())
        and all(result["busy_cycles"] <= result["elapsed_cycles"] for result in results.values())
    )
    evidence = {
        "schema": "nobro-complex-runtime-v1",
        "method": "RAM-only J-Link run; instrumented DWT task-work cycles; no flash writes",
        "run_ms": args.run_ms,
        "cpu_hz": CPU_HZ,
        "electrical_energy_measured": False,
        "restored_dfu": restored,
        "equivalent_observables": equivalent,
        "observable_spreads": spreads,
        "results": results,
    }
    output = WORK / "runtime.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(evidence, indent=2), encoding="utf-8")
    print(json.dumps(evidence, indent=2))
    print(f"evidence: {output}")
    return 0 if restored and equivalent and len(results) == len(IMPLEMENTATIONS) else 1


if __name__ == "__main__":
    sys.exit(main())
