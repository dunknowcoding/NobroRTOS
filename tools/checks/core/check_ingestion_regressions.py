#!/usr/bin/env python3
"""Stable minimized regressions from private bounded ingestion campaigns.

Mutation generation is deterministic and in-memory. Large corpora, fuzz engines,
raw crashes, and machine-specific campaign results remain private/ignored.
"""

from __future__ import annotations

import copy
import importlib.util
from pathlib import Path
import random
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "bindings" / "python"))
sys.path.insert(0, str(ROOT / "tools" / "cli" / "project"))

from nobro_rtos.app import AppDeclarationError, NobroApp  # noqa: E402
from nobro_rtos.reports import (  # noqa: E402
    FixedReport,
    ReportKind,
    ReportStatus,
    finalize_diagnostic_report,
)


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise ValueError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


firmware = load_module(
    "nobro_firmware_ingestion",
    ROOT / "tools" / "cli" / "project" / "nobro_firmware_project.py",
)
project = load_module(
    "nobro_project_ingestion",
    ROOT / "tools" / "cli" / "project" / "nobro_project.py",
)

SEED = 0x4E425231
MAX_CASES = 512


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def valid_app() -> dict:
    return {
        "schema": "nobro-app-v1",
        "app": "fuzz_app",
        "board": "nrf52840-nosd",
        "tasks": [{
            "name": "sense", "role": "periodic", "period_us": 10_000,
            "phase_us": 0, "deadline_us": 10_000, "budget_us": 500,
            "blocking_us": 0, "flash_bytes": 1024, "ram_bytes": 256,
        }],
        "wires": [],
    }


def app_regressions(rng: random.Random) -> int:
    base = valid_app()
    values = [None, True, False, -1, 0, 2**80, "", "x" * 256, [], {}, [1], {"x": 1}]
    paths = [
        ("schema",), ("app",), ("board",), ("tasks",), ("wires",),
        *(("tasks", 0, key) for key in base["tasks"][0]),
    ]
    cases = 0
    for path in paths:
        for value in values:
            document = copy.deepcopy(base)
            target = document
            for component in path[:-1]:
                target = target[component]
            target[path[-1]] = value
            try:
                app = NobroApp.from_dict(document)
                require(app.to_dict() == document, f"accepted app did not round-trip at {path}")
            except AppDeclarationError as error:
                require(str(error).startswith("NOBRO-E"), f"unstable app diagnostic at {path}")
            cases += 1
    rng.shuffle(values)
    return cases


def text_regressions(rng: random.Random) -> int:
    alphabet = "abcXYZ019_- #\n\t[]{}:/"
    stable = [
        "", "app", "app x\nboard missing\nperiodic a every 1ms",
        "app x\nboard nrf52840-nosd\nperiodic a every 0us",
        "app x\nboard nrf52840-nosd\ncontrol a every 1ms budget 2ms",
        "app x\nboard nrf52840-nosd\nperiodic a every 1ms -> missing",
    ]
    stable.extend(
        "".join(rng.choice(alphabet) for _ in range(rng.randrange(0, 256)))
        for _ in range(96)
    )
    for text in stable:
        try:
            parsed = firmware.parse(text)
            require(isinstance(parsed, dict), "compact parser returned a non-object")
            require(len(parsed.get("workload", {}).get("tasks", [])) <= 8, "task bound escaped")
        except (ValueError, KeyError, TypeError):
            pass
    return len(stable)


def graph_regressions() -> int:
    base = NobroApp.from_dict(valid_app()).firmware_spec()["workload"]
    values = [None, True, -1, 2**80, "bad", [], {}]
    cases = 0
    for key in ("profile", "tasks", "channels", "wire_capacities"):
        for value in values:
            workload = copy.deepcopy(base)
            workload[key] = value
            try:
                order = project.startup_order(workload, allow_empty_features_without_catalog=True)
                require(isinstance(order, list), "startup order returned non-list")
                require(len(order) <= 64, "startup order escaped its admitted bound")
            except (ValueError, KeyError, TypeError):
                pass
            cases += 1
    return cases


def report_regressions() -> int:
    cases = 0
    for kind in ReportKind:
        passing_flag = {
            ReportKind.BOARD_PACKAGE: "valid",
            ReportKind.MANIFEST: "valid",
            ReportKind.ADAPTER_COMPAT: "compatible",
            ReportKind.ADMISSION: "admitted",
        }.get(kind)
        payload = {} if passing_flag is None else {passing_flag: 1}
        sealed = finalize_diagnostic_report(kind, payload)
        report = FixedReport.from_dict(kind, sealed)
        require(report.status == ReportStatus.PASS, f"{kind.value}: sealed report rejected")
        for name in tuple(sealed):
            if name == "completed":
                continue
            corrupt = dict(sealed)
            corrupt[name] ^= 1
            decoded = FixedReport.from_dict(kind, corrupt)
            require(decoded.status != ReportStatus.PASS, f"{kind.value}: mutation escaped at {name}")
            cases += 1
        in_progress = dict(sealed)
        in_progress["completed"] = 0
        require(FixedReport.from_dict(kind, in_progress).status == ReportStatus.IN_PROGRESS,
                f"{kind.value}: in-progress state lost")
        cases += 1
    return cases


def main() -> int:
    rng = random.Random(SEED)
    counts = {
        "app": app_regressions(rng),
        "compact": text_regressions(rng),
        "graph": graph_regressions(),
        "report": report_regressions(),
    }
    require(sum(counts.values()) <= MAX_CASES, "public regression campaign lost its bound")
    print(f"ingestion regressions: PASS (seed={SEED}, cases={sum(counts.values())}, {counts})")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, OSError, ValueError) as error:
        print(f"ingestion regressions: FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
