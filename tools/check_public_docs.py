#!/usr/bin/env python3
"""Validate tracked public-document links, tool examples, and privacy boundaries."""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
DOCS = [
    ROOT / "README.md",
    *(ROOT / "docs" / name for name in (
        "README.md", "GETTING_STARTED.md", "USER_GUIDE.md", "API.md",
        "ARCHITECTURE.md", "PORTING.md", "ENGINEERING.md", "api-index.md",
    )),
]
LINK = re.compile(r"\[[^]]*\]\(([^)]+)\)")
TOOL = re.compile(r"(?:python3?|py)\s+(tools/[A-Za-z0-9_./-]+\.py)\b")
PRIVATE = {
    "internal plan reference": re.compile(r"(?:_INTERNAL\.md|REMODELING_PLAN_INTERNAL)"),
    "lab board label": re.compile(r"\bboard[1-9][0-9]*\b", re.IGNORECASE),
    "local serial port": re.compile(r"\bCOM[0-9]+\b", re.IGNORECASE),
    "Windows machine path": re.compile(r"\b[A-Za-z]:\\"),
    "local environment": re.compile(r"\b(?:conda|venv)\s+(?:activate|env)\b", re.IGNORECASE),
}


def main() -> int:
    errors: list[str] = []
    for path in DOCS:
        if not path.is_file():
            errors.append(f"missing public document: {path.relative_to(ROOT)}")
            continue
        text = path.read_text(encoding="utf-8")
        relative = path.relative_to(ROOT)
        for label, pattern in PRIVATE.items():
            for match in pattern.finditer(text):
                line = text.count("\n", 0, match.start()) + 1
                errors.append(f"{relative}:{line}: {label}: {match.group(0)!r}")
        for match in LINK.finditer(text):
            target = match.group(1).split("#", 1)[0].strip()
            if not target or "://" in target or target.startswith(("mailto:", "#")):
                continue
            resolved = (path.parent / target).resolve()
            if not resolved.exists():
                line = text.count("\n", 0, match.start()) + 1
                errors.append(f"{relative}:{line}: broken link: {target}")
        for match in TOOL.finditer(text):
            target = ROOT / match.group(1)
            if not target.is_file():
                line = text.count("\n", 0, match.start()) + 1
                errors.append(f"{relative}:{line}: missing CLI tool: {match.group(1)}")

    for error in errors:
        print(f"FAIL: {error}")
    print(f"PUBLIC DOCS: {'PASS' if not errors else 'FAIL'} ({len(DOCS)} files)")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
