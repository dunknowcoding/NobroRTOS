#!/usr/bin/env python3
"""Gate bounded source responsibilities and compatibility-preserving modules."""

from __future__ import annotations

import pathlib
import re
import sys


ROOT = pathlib.Path(__file__).resolve().parents[3]
CORE = ROOT / "core"
KERNEL = CORE / "crates" / "nobro_kernel" / "src"
MAX_PRODUCTION_LINES = 2_250
TEST_MODULE = re.compile(r"(?m)^#\[cfg\(test\)\]\s*\nmod (?:tests|property_tests) \{")

REQUIRED_CONCERNS = {
    "scheduler": "scheduler.rs",
    "task table": "executor.rs",
    "runtime/executor orchestration": "kernel_executor.rs",
    "application graph": "graph.rs",
    "manifest": "manifest.rs",
    "reactor": "async_rt.rs",
    "async synchronization": "async_sync.rs",
    "managed runtime": "runtime.rs",
    "dynamic dependencies": "runtime_dependency.rs",
    "capacity presets": "presets.rs",
}


def source_counts(path: pathlib.Path) -> tuple[int, int]:
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines()
    match = TEST_MODULE.search(text)
    if match is None:
        return len(lines), 0
    production = text[: match.start()].count("\n")
    return production, len(lines) - production


def maintained_sources() -> list[pathlib.Path]:
    roots = [CORE / name for name in ("crates", "adapters", "apps", "ports")]
    return sorted(
        path
        for root in roots
        for path in root.rglob("*.rs")
        if "vendor" not in path.parts and "target" not in path.parts
    )


def validate() -> tuple[list[str], list[tuple[int, int, str]]]:
    errors: list[str] = []
    lib = (KERNEL / "lib.rs").read_text(encoding="utf-8")
    startup = (KERNEL / "startup.rs").read_text(encoding="utf-8")
    async_rt = (KERNEL / "async_rt.rs").read_text(encoding="utf-8")

    for concern, filename in REQUIRED_CONCERNS.items():
        path = KERNEL / filename
        module = path.stem
        if not path.is_file():
            errors.append(f"{concern}: missing {path.relative_to(ROOT).as_posix()}")
        if f"pub mod {module};" not in lib:
            errors.append(f"{concern}: {module} is not a discoverable public module")

    compatibility = {
        "startup dynamic-dependency re-export": (
            startup,
            "pub use crate::runtime_dependency::{",
        ),
        "async compatibility re-export": (async_rt, "pub use crate::async_sync::{"),
        "root capacity-preset compatibility": (lib, "pub type SmallRuntime ="),
        "explicit application identity capacity": (lib, "APP_MODULE_ID_CAPACITY"),
        "explicit compact startup capacity": (lib, "STARTUP_GRAPH_MAX_MODULES"),
    }
    for label, (text, token) in compatibility.items():
        if token not in text:
            errors.append(f"{label}: missing `{token}`")

    counts: list[tuple[int, int, str]] = []
    for path in maintained_sources():
        production, tests = source_counts(path)
        relative = path.relative_to(ROOT).as_posix()
        counts.append((production, tests, relative))
        if production > MAX_PRODUCTION_LINES:
            errors.append(
                f"{relative}: {production} production lines exceed bounded audit ceiling "
                f"{MAX_PRODUCTION_LINES}; split by responsibility or explicitly revise policy"
            )
    return errors, sorted(counts, reverse=True)


def main() -> int:
    errors, counts = validate()
    for error in errors:
        print(f"MAINTAINABILITY: {error}")
    leaders = ", ".join(
        f"{path}={production}+{tests}test"
        for production, tests, path in counts[:5]
    )
    print(f"MAINTAINABILITY: production/test recount ({leaders})")
    print(
        "MAINTAINABILITY: "
        + ("PASS" if not errors else "FAIL")
        + f" ({len(counts)} maintained Rust files, production ceiling {MAX_PRODUCTION_LINES})"
    )
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
