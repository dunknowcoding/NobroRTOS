#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Generate a bounded byte-machine Core application contract."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys


SCHEMA = "nobro-rtos-core-byte-v1"
IDENTIFIER = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


class ContractError(ValueError):
    """A fail-closed Core application error."""


def _integer(value: object, name: str, low: int, high: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ContractError(f"NOBRO-E070: {name} must be an integer")
    if value < low or value > high:
        raise ContractError(
            f"NOBRO-E070: {name} must be in {low}..{high}")
    return value


def normalize(document: object) -> dict[str, object]:
    if not isinstance(document, dict) or document.get("schema") != SCHEMA:
        raise ContractError(f"NOBRO-E070: schema must be {SCHEMA}")
    allowed = {
        "schema", "tick_unit_us", "mailbox_capacity", "idle_hook",
        "watchdog_hook", "program_capacity_bytes", "application_reserve_bytes",
        "tasks",
    }
    unknown = sorted(set(document) - allowed)
    if unknown:
        raise ContractError(f"NOBRO-E070: unknown fields: {', '.join(unknown)}")
    tick_unit_us = _integer(document.get("tick_unit_us"), "tick_unit_us", 1, 1_000_000)
    mailbox = _integer(document.get("mailbox_capacity", 0), "mailbox_capacity", 0, 8)
    idle = document.get("idle_hook", False)
    watchdog = document.get("watchdog_hook", False)
    if not isinstance(idle, bool) or not isinstance(watchdog, bool):
        raise ContractError("NOBRO-E070: hook selections must be booleans")
    program_capacity = _integer(
        document.get("program_capacity_bytes", 2048),
        "program_capacity_bytes", 2048, 8192,
    )
    if program_capacity not in (2048, 4096, 8192):
        raise ContractError(
            "NOBRO-E070: program_capacity_bytes must be 2048, 4096, or 8192")
    application_reserve = _integer(
        document.get("application_reserve_bytes", program_capacity // 2),
        "application_reserve_bytes", program_capacity // 2, program_capacity - 1,
    )
    tasks = document.get("tasks")
    if not isinstance(tasks, list) or not 1 <= len(tasks) <= 8:
        raise ContractError("NOBRO-E070: tasks must contain 1..8 entries")
    names: set[str] = set()
    macro_names: set[str] = set()
    normalized_tasks: list[dict[str, object]] = []
    utilization_numerator = 0
    utilization_denominator = 1
    for index, raw in enumerate(tasks):
        if not isinstance(raw, dict):
            raise ContractError(f"NOBRO-E070: tasks[{index}] must be an object")
        extra = sorted(set(raw) - {
            "name", "period_ticks", "dispatch_deadline_ticks", "budget_ticks",
        })
        if extra:
            raise ContractError(
                f"NOBRO-E070: tasks[{index}] unknown fields: {', '.join(extra)}")
        name = raw.get("name")
        if not isinstance(name, str) or not IDENTIFIER.fullmatch(name):
            raise ContractError(
                f"NOBRO-E070: tasks[{index}].name must be a C identifier")
        if name.startswith("_") or name == "nobro_core":
            raise ContractError(
                f"NOBRO-E070: tasks[{index}].name collides with a reserved symbol")
        if name in names:
            raise ContractError(f"NOBRO-E070: duplicate task name {name}")
        macro_name = name.upper()
        if macro_name in macro_names:
            raise ContractError(
                f"NOBRO-E070: task names collide after macro normalization: {name}")
        names.add(name)
        macro_names.add(macro_name)
        period = _integer(raw.get("period_ticks"), f"{name}.period_ticks", 1, 127)
        deadline = _integer(
            raw.get("dispatch_deadline_ticks"),
            f"{name}.dispatch_deadline_ticks", 1, period,
        )
        budget = _integer(raw.get("budget_ticks"), f"{name}.budget_ticks", 1, deadline)
        # Exact rational addition; admission is fail-closed at utilization > 1.
        utilization_numerator = utilization_numerator * period + budget * utilization_denominator
        utilization_denominator *= period
        divisor = _gcd(utilization_numerator, utilization_denominator)
        utilization_numerator //= divisor
        utilization_denominator //= divisor
        normalized_tasks.append({
            "name": name,
            "period_ticks": period,
            "dispatch_deadline_ticks": deadline,
            "budget_ticks": budget,
        })
    if utilization_numerator > utilization_denominator:
        raise ContractError(
            "NOBRO-E071: admitted task utilization exceeds one byte-machine core")
    start_bounds = _dispatch_start_bounds(normalized_tasks)
    return {
        "schema": SCHEMA,
        "tick_unit_us": tick_unit_us,
        "mailbox_capacity": mailbox,
        "idle_hook": idle,
        "watchdog_hook": watchdog,
        "program_capacity_bytes": program_capacity,
        "application_reserve_bytes": application_reserve,
        "tasks": normalized_tasks,
        "utilization": {
            "numerator": utilization_numerator,
            "denominator": utilization_denominator,
        },
        "dispatch_start_bounds_ticks": start_bounds,
    }


def _gcd(left: int, right: int) -> int:
    while right:
        left, right = right, left % right
    return left


def _dispatch_start_bounds(tasks: list[dict[str, object]]) -> list[int]:
    """Conservatively admit fixed-order, non-preemptive dispatch starts."""

    bounds: list[int] = []
    for index, task in enumerate(tasks):
        deadline = int(task["dispatch_deadline_ticks"])
        blocking = max(
            (int(lower["budget_ticks"]) for lower in tasks[index + 1:]),
            default=0,
        )
        higher = tasks[:index]
        response = blocking + sum(int(item["budget_ticks"]) for item in higher)
        while True:
            interference = 0
            for item in higher:
                period = int(item["period_ticks"])
                budget = int(item["budget_ticks"])
                jobs = (response + period - 1) // period
                if jobs == 0:
                    jobs = 1
                interference += jobs * budget
            updated = blocking + interference
            if updated == response:
                break
            if updated >= deadline:
                response = updated
                break
            response = updated
        if response >= deadline:
            raise ContractError(
                "NOBRO-E071: declared non-preemptive dispatch-start bound "
                f"for {task['name']} is {response} ticks, not below its "
                f"{deadline}-tick deadline")
        bounds.append(response)
    return bounds


def digest(contract: dict[str, object]) -> str:
    encoded = json.dumps(
        contract, sort_keys=True, separators=(",", ":"), ensure_ascii=True
    ).encode("ascii")
    return hashlib.sha256(encoded).hexdigest()


def render_config_h(contract: dict[str, object], identity: str) -> str:
    tasks = contract["tasks"]
    assert isinstance(tasks, list)
    lines = [
        "#ifndef NOBRO_CORE_CONFIG_H",
        "#define NOBRO_CORE_CONFIG_H",
        "",
        "/* Generated from a validated bounded Core contract. */",
        f"#define NOBRO_CORE_TASK_COUNT {len(tasks)}",
        f"#define NOBRO_CORE_MAILBOX_CAPACITY {contract['mailbox_capacity']}",
        f"#define NOBRO_CORE_ENABLE_IDLE_HOOK {int(bool(contract['idle_hook']))}",
        f"#define NOBRO_CORE_ENABLE_WATCHDOG_HOOK {int(bool(contract['watchdog_hook']))}",
        f"#define NOBRO_CORE_TICK_UNIT_US {contract['tick_unit_us']}UL",
        f"#define NOBRO_CORE_PROGRAM_CAPACITY_BYTES {contract['program_capacity_bytes']}UL",
        f"#define NOBRO_CORE_APPLICATION_RESERVE_BYTES {contract['application_reserve_bytes']}UL",
        f'#define NOBRO_CORE_CONTRACT_SHA256 "{identity}"',
        "",
        '#include "nobro_core.h"',
        "",
    ]
    for index, task in enumerate(tasks):
        assert isinstance(task, dict)
        lines.append(f"#define NOBRO_CORE_TASK_{str(task['name']).upper()} {index}")
    lines.extend(["", "#endif", ""])
    return "\n".join(lines)


def render_config_c(contract: dict[str, object]) -> str:
    tasks = contract["tasks"]
    assert isinstance(tasks, list)
    lines = [
        '#include "nobro_core_config.h"',
        "",
        "NOBRO_CORE_CODE const struct nobro_core_task_spec",
        "nobro_core_tasks[NOBRO_CORE_TASK_COUNT] = {",
    ]
    for task in tasks:
        assert isinstance(task, dict)
        lines.append(
            f"    {{ {task['period_ticks']}u, {task['dispatch_deadline_ticks']}u }},")
    lines.extend(["};", ""])
    return "\n".join(lines)


def render_app_c(contract: dict[str, object]) -> str:
    tasks = contract["tasks"]
    assert isinstance(tasks, list)
    lines = ['#include "nobro_core_config.h"', ""]
    for task in tasks:
        assert isinstance(task, dict)
        lines.append(f"extern void {task['name']}_step(void);")
    if contract["idle_hook"]:
        lines.append("extern void nobro_app_idle(void);")
    if contract["watchdog_hook"]:
        lines.append("extern void nobro_app_watchdog(void);")
    lines.extend(["", "void nobro_core_step(void)", "{", "    switch (nobro_core_abi.active_task) {"])
    for index, task in enumerate(tasks):
        assert isinstance(task, dict)
        lines.append(f"    case {index}u: {task['name']}_step(); break;")
    lines.extend(["    default: break;", "    }", "}", ""])
    if contract["idle_hook"]:
        lines.extend([
            "void nobro_core_idle(void)", "{", "    nobro_app_idle();", "}", "",
        ])
    if contract["watchdog_hook"]:
        lines.extend([
            "void nobro_core_watchdog(void)", "{", "    nobro_app_watchdog();", "}", "",
        ])
    return "\n".join(lines)


def render_assembly(toolchain: str) -> str:
    if toolchain == "sdcc":
        prefix, comment, equ = "_", ";", ".equ"
    elif toolchain == "keil":
        # No-argument C51 functions and globals use their source names.
        prefix, comment, equ = "", ";", "EQU"
    elif toolchain == "iar":
        prefix, comment, equ = "", ";", "EQU"
    else:
        raise AssertionError(toolchain)
    names = ["reset", "dispatch", "post", "take", "recover", "step"]
    lines = [
        f"{comment} Generated logical-symbol and ABI-offset contract.",
        f"{comment} Entry points take no parameters and return no register value.",
    ]
    if toolchain == "sdcc":
        for name in names:
            lines.append(f".globl {prefix}nobro_core_{name}")
        lines.append(f".globl {prefix}nobro_core_abi")
    elif toolchain == "keil":
        for name in names:
            lines.append(f"EXTRN CODE (nobro_core_{name})")
        lines.append("EXTRN DATA (nobro_core_abi)")
    else:
        for name in names:
            lines.append(f"\tEXTERN nobro_core_{name}")
        lines.append("\tEXTERN nobro_core_abi")
    if toolchain == "sdcc":
        # ASxxxx .equ cannot alias a relocatable external symbol.  A delimited
        # .define performs the intended token substitution without inventing
        # a second linker symbol.
        for name in names:
            lines.append(
                f".define NOBRO_CORE_{name.upper()} /{prefix}nobro_core_{name}/")
        lines.append(f".define NOBRO_CORE_ABI /{prefix}nobro_core_abi/")
    elif toolchain == "iar":
        for name in names:
            lines.append(f"NOBRO_CORE_{name.upper()} {equ} {prefix}nobro_core_{name}")
        lines.append(f"NOBRO_CORE_ABI {equ} {prefix}nobro_core_abi")
    else:
        lines.append(
            f"{comment} Call imported nobro_core_* C51 symbols by their exact names.")
    for name, offset in (
        ("NOW", 0), ("SLEEP_TICKS", 1), ("ACTIVE_TASK", 2),
        ("MESSAGE", 3), ("RESULT", 4), ("FAULTS", 5),
    ):
        lines.append(f"NOBRO_CORE_ABI_{name} {equ} {offset}")
    lines.append("")
    return "\n".join(lines)


def parse_size_report(text: str, report_format: str) -> dict[str, int]:
    """Parse exact compiler/linker size output without guessing units."""

    if report_format == "mplab-xc8":
        program_match = re.search(
            r"Program space\s+used\s+[0-9A-Fa-f]+h\s+\(\s*([0-9,]+)\)"
            r"\s+of\s+[0-9A-Fa-f]+h\s+words",
            text, re.IGNORECASE,
        )
        data_match = re.search(
            r"Data space\s+used\s+[0-9A-Fa-f]+h\s+\(\s*([0-9,]+)\)"
            r"\s+of\s+[0-9A-Fa-f]+h\s+bytes",
            text, re.IGNORECASE,
        )
        if not program_match or not data_match:
            raise ContractError("NOBRO-E072: MPLAB XC8 report lacks exact totals")
        return {
            "program_words": int(program_match.group(1).replace(",", "")),
            "program_unit_bits": 14,
            "data_bytes": int(data_match.group(1).replace(",", "")),
        }

    if report_format == "sdcc-stm8-map":
        areas = {}
        for match in re.finditer(
            r"^([A-Za-z_.][A-Za-z0-9_.]*)\s+[0-9A-Fa-f]{8}\s+"
            r"[0-9A-Fa-f]{8}\s+=\s+(\d+)\.\s+bytes",
            text, re.MULTILINE,
        ):
            areas[match.group(1)] = int(match.group(2))
        code_names = ("HOME", "GSINIT", "GSFINAL", "CONST", "INITIALIZER", "CODE")
        data_names = ("DATA", "INITIALIZED")
        if "CODE" not in areas or "DATA" not in areas:
            raise ContractError("NOBRO-E072: SDCC STM8 map lacks CODE/DATA areas")
        return {
            "program_bytes": sum(areas.get(name, 0) for name in code_names),
            "data_bytes": sum(areas.get(name, 0) for name in data_names),
        }

    patterns: dict[str, tuple[str, str]] = {
        "sdcc-mem": (
            r"ROM/EPROM/FLASH\s+0x[0-9A-Fa-f]+\s+0x[0-9A-Fa-f]+\s+(\d+)",
            r"Kernel-owned data\s*:\s*(\d+)\s+bytes",
        ),
        "iar": (
            r"([0-9][0-9,]*)\s+bytes?\s+of\s+CODE\s+memory",
            r"([0-9][0-9,]*)\s+bytes?\s+of\s+(?:DATA|read/write)\s+memory",
        ),
        "keil-c51": (
            r"Program Size:\s*data=\d+(?:\.\d+)?\s+xdata=\d+(?:\.\d+)?\s+code=(\d+(?:\.\d+)?)",
            r"Program Size:\s*data=(\d+(?:\.\d+)?)\s+xdata=",
        ),
        "gnu-size": (
            r"^\s*(\d+)\s+\d+\s+\d+\s+\d+\s+[0-9A-Fa-f]+\s+\S+\s*$",
            r"^\s*\d+\s+(\d+)\s+(\d+)\s+\d+\s+[0-9A-Fa-f]+\s+\S+\s*$",
        ),
        "mplab": (
            r"Program space used\s*:?\s*([0-9][0-9,]*)\s+bytes",
            r"Data space used\s*:?\s*([0-9][0-9,]*)\s+bytes",
        ),
        "mplab-xc16": (
            r"Total\s+\"program\"\s+memory\s+used\s+\(bytes\):\s+"
            r"0x[0-9A-Fa-f]+\s+\(([0-9,]+)\)",
            r"Total\s+\"data\"\s+memory\s+used\s+\(bytes\):\s+"
            r"0x[0-9A-Fa-f]+\s+\(([0-9,]+)\)",
        ),
    }
    if report_format not in patterns:
        raise ContractError(f"NOBRO-E072: unsupported size format {report_format}")
    program_pattern, data_pattern = patterns[report_format]
    flags = re.IGNORECASE | re.MULTILINE
    program_match = re.search(program_pattern, text, flags)
    data_match = re.search(data_pattern, text, flags)
    if not program_match or not data_match:
        raise ContractError(
            f"NOBRO-E072: {report_format} report lacks exact program/data totals")

    def number(value: str) -> int:
        parsed = float(value.replace(",", ""))
        if not parsed.is_integer():
            raise ContractError("NOBRO-E072: fractional byte total is not exact")
        return int(parsed)

    if report_format == "gnu-size":
        initialized = number(data_match.group(1))
        program = number(program_match.group(1)) + initialized
        data = initialized + number(data_match.group(2))
    else:
        program = number(program_match.group(1))
        data = number(data_match.group(1))
    return {"program_bytes": program, "data_bytes": data}


def generate(source: Path, output: Path) -> dict[str, object]:
    document = json.loads(source.read_text(encoding="utf-8"))
    contract = normalize(document)
    identity = digest(contract)
    output.mkdir(parents=True, exist_ok=True)
    files = {
        "nobro_core_config.h": render_config_h(contract, identity),
        "nobro_core_config.c": render_config_c(contract),
        "nobro_core_app.c": render_app_c(contract),
        "nobro_core_sdcc.inc": render_assembly("sdcc"),
        "nobro_core_keil.inc": render_assembly("keil"),
        "nobro_core_iar.inc": render_assembly("iar"),
    }
    for name, content in files.items():
        (output / name).write_text(content, encoding="ascii", newline="\n")
    receipt = dict(contract)
    receipt["contract_sha256"] = identity
    receipt["kernel_data_bytes"] = (
        6 + len(contract["tasks"]) +
        (2 + int(contract["mailbox_capacity"])
         if int(contract["mailbox_capacity"]) else 0)
    )
    receipt["resource_ladder"] = {
        "profile": "core-byte",
        "program_capacity_bytes": contract["program_capacity_bytes"],
        "application_reserve_bytes": contract["application_reserve_bytes"],
        "runtime_baseline_budget_bytes": (
            int(contract["program_capacity_bytes"])
            - int(contract["application_reserve_bytes"])
        ),
        "maximum_linked_image_bytes": contract["program_capacity_bytes"],
        "optional_extensions": [],
    }
    (output / "nobro_core_contract.json").write_text(
        json.dumps(receipt, indent=2, sort_keys=True) + "\n",
        encoding="ascii", newline="\n",
    )
    return receipt


def selftest() -> int:
    sample = {
        "schema": SCHEMA,
        "tick_unit_us": 1000,
        "mailbox_capacity": 2,
        "idle_hook": True,
        "watchdog_hook": True,
        "program_capacity_bytes": 2048,
        "application_reserve_bytes": 1024,
        "tasks": [
            {"name": "sense", "period_ticks": 10,
             "dispatch_deadline_ticks": 3, "budget_ticks": 1},
            {"name": "control", "period_ticks": 20,
             "dispatch_deadline_ticks": 4, "budget_ticks": 2},
        ],
    }
    contract = normalize(sample)
    if contract["utilization"] != {"numerator": 1, "denominator": 5}:
        raise ContractError("selftest utilization drift")
    if contract["dispatch_start_bounds_ticks"] != [2, 1]:
        raise ContractError("selftest dispatch-start admission drift")
    for mutation in (
        {**sample, "tasks": []},
        {**sample, "mailbox_capacity": 9},
        {**sample, "program_capacity_bytes": 3072},
        {**sample, "application_reserve_bytes": 1000},
        {**sample, "tasks": [{"name": "bad-name", "period_ticks": 1,
                               "dispatch_deadline_ticks": 1, "budget_ticks": 1}]},
        {**sample, "tasks": [{"name": "nobro_core", "period_ticks": 1,
                               "dispatch_deadline_ticks": 1, "budget_ticks": 1}]},
        {**sample, "tasks": [{"name": "one", "period_ticks": 2,
                               "dispatch_deadline_ticks": 1, "budget_ticks": 1},
                              {"name": "ONE", "period_ticks": 2,
                               "dispatch_deadline_ticks": 1, "budget_ticks": 1}]},
        {**sample, "tasks": [{"name": "a", "period_ticks": 1,
                               "dispatch_deadline_ticks": 1, "budget_ticks": 1},
                              {"name": "b", "period_ticks": 1,
                               "dispatch_deadline_ticks": 1, "budget_ticks": 1}]},
        {**sample, "tasks": [{"name": "urgent", "period_ticks": 10,
                               "dispatch_deadline_ticks": 2, "budget_ticks": 1},
                              {"name": "blocking", "period_ticks": 10,
                               "dispatch_deadline_ticks": 10, "budget_ticks": 2}]},
    ):
        try:
            normalize(mutation)
        except ContractError:
            pass
        else:
            raise ContractError("selftest accepted an invalid contract")
    reports = {
        "sdcc-mem": (
            "ROM/EPROM/FLASH  0x0000 0x02bb 700 2048\n"
            "Kernel-owned data: 12 bytes\n", {"program_bytes": 700, "data_bytes": 12}),
        "iar": ("700 bytes of CODE memory\n12 bytes of DATA memory\n",
                {"program_bytes": 700, "data_bytes": 12}),
        "keil-c51": ("Program Size: data=12.0 xdata=0 code=700\n",
                     {"program_bytes": 700, "data_bytes": 12}),
        "gnu-size": (" text data bss dec hex filename\n 680 8 4 692 2b4 app.elf\n",
                     {"program_bytes": 688, "data_bytes": 12}),
        "mplab": ("Program space used: 700 bytes\nData space used: 12 bytes\n",
                  {"program_bytes": 700, "data_bytes": 12}),
        "mplab-xc16": (
            'Total "program" memory used (bytes): 0x2bc (700)\n'
            'Total "data" memory used (bytes): 0xc (12)\n',
            {"program_bytes": 700, "data_bytes": 12}),
    }
    for kind, (report, expected) in reports.items():
        if parse_size_report(report, kind) != expected:
            raise ContractError(f"selftest {kind} size parser drift")
    keil_assembly = render_assembly("keil")
    if ("EXTRN CODE (nobro_core_dispatch)" not in keil_assembly or
            "EXTRN DATA (nobro_core_abi)" not in keil_assembly or
            "NOBRO_CORE_DISPATCH EQU" in keil_assembly):
        raise ContractError("selftest Keil C51 assembly spelling drift")
    try:
        parse_size_report("ambiguous", "iar")
    except ContractError:
        pass
    else:
        raise ContractError("selftest accepted an incomplete size report")
    xc8 = parse_size_report(
        "Program space used 15Ch (348) of 2000h words\n"
        "Data space used 1Eh (30) of 400h bytes\n", "mplab-xc8")
    if xc8 != {"program_words": 348, "program_unit_bits": 14, "data_bytes": 30}:
        raise ContractError("selftest MPLAB XC8 size parser drift")
    stm8 = parse_size_report(
        "HOME 00008000 00000007 = 7. bytes (REL,CON)\n"
        "GSINIT 00008007 00000023 = 35. bytes (REL,CON)\n"
        "GSFINAL 0000802A 00000003 = 3. bytes (REL,CON)\n"
        "CONST 0000802D 00000004 = 4. bytes (REL,CON)\n"
        "CODE 00008031 000001DD = 477. bytes (REL,CON)\n"
        "DATA 00000001 0000000E = 14. bytes (REL,CON)\n",
        "sdcc-stm8-map",
    )
    if stm8 != {"program_bytes": 526, "data_bytes": 14}:
        raise ContractError("selftest SDCC STM8 size parser drift")
    print("NOBRO CORE GENERATOR: PASS")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("contract", nargs="?", type=Path)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--map", dest="size_report", type=Path)
    parser.add_argument(
        "--format", choices=(
            "sdcc-mem", "sdcc-stm8-map", "iar", "keil-c51", "gnu-size",
            "mplab", "mplab-xc8", "mplab-xc16",
        ))
    parser.add_argument("--program-limit", type=int)
    parser.add_argument("--program-word-limit", type=int)
    parser.add_argument("--data-limit", type=int)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if args.size_report is not None:
        if args.format is None:
            parser.error("--map requires --format; format guessing is not allowed")
        try:
            totals = parse_size_report(
                args.size_report.read_text(encoding="utf-8", errors="replace"),
                args.format,
            )
        except (ContractError, OSError) as error:
            print(f"Core SIZE: FAIL ({error})", file=sys.stderr)
            return 1
        if args.program_limit is not None and totals.get("program_bytes", -1) > args.program_limit:
            print("Core SIZE: FAIL (program limit exceeded)", file=sys.stderr)
            return 1
        if args.program_limit is not None and "program_bytes" not in totals:
            print("Core SIZE: FAIL (report uses non-byte program units)", file=sys.stderr)
            return 1
        if (args.program_word_limit is not None and
                totals.get("program_words", args.program_word_limit + 1) > args.program_word_limit):
            print("Core SIZE: FAIL (program word limit exceeded)", file=sys.stderr)
            return 1
        if args.data_limit is not None and totals["data_bytes"] > args.data_limit:
            print("Core SIZE: FAIL (data limit exceeded)", file=sys.stderr)
            return 1
        print(json.dumps(totals, sort_keys=True))
        print("Core SIZE: PASS")
        return 0
    if args.contract is None or args.out is None:
        parser.error("contract and --out are required")
    try:
        receipt = generate(args.contract, args.out)
    except (ContractError, OSError, json.JSONDecodeError) as error:
        print(f"NOBRO CORE GENERATOR: FAIL ({error})", file=sys.stderr)
        return 1
    print(
        "NOBRO CORE GENERATOR: PASS "
        f"({len(receipt['tasks'])} tasks, {receipt['kernel_data_bytes']} kernel data bytes)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
