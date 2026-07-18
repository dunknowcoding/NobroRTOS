#!/usr/bin/env python3
"""Validate one canonical NobroRTOS task/wire app document.

`nobro app` and the block editor use the same strict `nobro-app-v1`
document as `nobro firmware`. This command is a fast validation/inspection
step; native firmware generation remains `nobro firmware`.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bindings" / "python"))

from nobro_rtos.app import AppDeclarationError, NobroApp  # noqa: E402


def validate(document: object) -> list[str]:
    """Compatibility helper used by package gates: return zero or one error."""

    try:
        NobroApp.from_dict(document)
    except (AppDeclarationError, KeyError, TypeError) as error:
        return [str(error)]
    return []


def plan(app: NobroApp) -> str:
    lines = [
        f"=== NobroRTOS app: {app.name} ===",
        f"board: {app.board}",
    ]
    for task in app.tasks:
        lines.append(
            f"  task {task.name}: {task.role} every {task.period_us} us "
            f"(budget {task.budget_us} us)"
        )
    for wire in app.wires:
        lines.append(
            f"  wire {wire.source} -> {wire.destination} "
            f"(capacity {wire.capacity})"
        )
    return "\n".join(lines)


def generate_rust(app: NobroApp) -> str:
    """Render the same small Rust graph vocabulary for compatibility users."""

    constructors = {
        "periodic": "periodic",
        "control": "control",
        "service": "service",
    }
    lines = [
        "// GENERATED from a canonical NobroRTOS task/wire app.",
        "use nobro_kernel::{AppGraph, TaskDecl};",
        "",
        f"pub fn app() -> AppGraph<{len(app.tasks)}> {{",
        f"    AppGraph::<{len(app.tasks)}>::new()",
    ]
    for task in app.tasks:
        constructor = constructors[task.role]
        expression = (
            f'TaskDecl::{constructor}("{task.name}", {task.period_us})'
            f".phase_us({task.phase_us})"
            f".deadline_us({task.deadline_us})"
            f".budget_us({task.budget_us})"
            f".blocking_us({task.blocking_us})"
        )
        lines.append(f"        .task({expression}).unwrap()")
    for wire in app.wires:
        lines.append(
            f'        .wire("{wire.source}", "{wire.destination}").unwrap()'
        )
    lines.extend(["}", ""])
    return "\n".join(lines)


def selftest() -> int:
    document = {
        "schema": "nobro-app-v1",
        "app": "hello_device",
        "board": "nrf52840-nosd",
        "tasks": [
            {
                "name": "imu",
                "role": "periodic",
                "period_us": 10000,
                "phase_us": 0,
                "deadline_us": 10000,
                "budget_us": 1000,
                "blocking_us": 0,
                "flash_bytes": 1024,
                "ram_bytes": 256,
            },
            {
                "name": "control",
                "role": "control",
                "period_us": 20000,
                "phase_us": 0,
                "deadline_us": 20000,
                "budget_us": 2000,
                "blocking_us": 0,
                "flash_bytes": 1024,
                "ram_bytes": 256,
            },
        ],
        "wires": [{"from": "imu", "to": "control", "capacity": 8}],
    }
    app = NobroApp.from_dict(document)
    assert not validate(document)
    assert ".wire(\"imu\", \"control\")" in generate_rust(app)
    bad = json.loads(json.dumps(document))
    bad["wires"][0]["to"] = "missing"
    errors = validate(bad)
    assert len(errors) == 1 and errors[0].startswith("NOBRO-E055:")
    print(plan(app))
    print("RESULT: PASS")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("app", nargs="?")
    parser.add_argument("--gen", help="write a compatibility Rust AppGraph source")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if not args.app:
        parser.print_help()
        return 0
    try:
        app = NobroApp.read_json(args.app)
    except (AppDeclarationError, OSError) as error:
        print(f"ERROR: {error}")
        print("RESULT: FAIL")
        return 1
    print(plan(app))
    if args.gen:
        Path(args.gen).write_text(
            generate_rust(app), encoding="utf-8", newline="\n"
        )
        print(f"\nwrote {args.gen}")
    print("RESULT: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
