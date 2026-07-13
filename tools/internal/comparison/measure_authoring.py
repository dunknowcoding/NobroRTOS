#!/usr/bin/env python3
"""Reproduce the narrow Wave-52 authoring comparison without marketing inference."""
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
SOURCES = {
    "nobro": ROOT / "tutorials" / "rover-one-file" / "app.nobro",
    "embassy": ROOT / "core" / "baselines" / "authoring" / "embassy.rs",
    "freertos": ROOT / "core" / "baselines" / "authoring" / "freertos.c",
}
CONCEPTS = {
    "nobro": ["board profile", "task role", "period", "channel"],
    "embassy": ["task attribute", "async function", "timer/duration", "spawner", "entry attribute"],
    "freertos": ["task function", "delay/tick conversion", "stack", "priority", "task creation", "scheduler start"],
}


def semantic_lines(path: pathlib.Path) -> int:
    lines = []
    in_block = False
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if line.startswith("/*"):
            in_block = True
        if line and not in_block and not line.startswith(("# ", "//")):
            lines.append(line)
        if line.endswith("*/"):
            in_block = False
    return len(lines)


def measure() -> dict:
    return {name: {"semantic_lines": semantic_lines(path),
                   "declared_concepts": len(CONCEPTS[name]),
                   "concepts": CONCEPTS[name]}
            for name, path in SOURCES.items()}


def main() -> int:
    result = measure()
    assert result["nobro"]["semantic_lines"] == 5
    assert all(row["semantic_lines"] >= 5 for row in result.values())
    print(json.dumps({"schema": "nobro-authoring-comparison-v1",
                      "scope": "three periodic task declarations only",
                      "results": result}, indent=2))
    print("AUTHORING COMPARISON: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
