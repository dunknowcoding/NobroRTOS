#!/usr/bin/env python3
"""Validate tracked public-document links, tool examples, and privacy boundaries."""

import pathlib
import re
import subprocess
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
TEXT_SUFFIXES = {
    ".c", ".cc", ".cpp", ".h", ".hpp", ".ino", ".json", ".md", ".ps1",
    ".py", ".rs", ".sh", ".toml", ".txt", ".yaml", ".yml",
}


def tracked_text_files() -> list[pathlib.Path]:
    result = subprocess.run(
        ["git", "ls-files", "-z"], cwd=ROOT, capture_output=True, check=True
    )
    return [
        ROOT / raw.decode("utf-8")
        for raw in result.stdout.split(b"\0")
        if raw and pathlib.Path(raw.decode("utf-8")).suffix.lower() in TEXT_SUFFIXES
    ]


def main() -> int:
    errors: list[str] = []
    local_needles_path = ROOT / "tools" / "leak_needles.local.txt"
    local_needles = []
    if local_needles_path.is_file():
        local_needles = [
            line.strip()
            for line in local_needles_path.read_text(encoding="utf-8").splitlines()
            if line.strip() and not line.lstrip().startswith("#")
        ]

    for path in tracked_text_files():
        text = path.read_text(encoding="utf-8", errors="replace")
        relative = path.relative_to(ROOT)
        patterns = {} if path == pathlib.Path(__file__).resolve() else PRIVATE
        for label, pattern in patterns.items():
            for match in pattern.finditer(text):
                line = text.count("\n", 0, match.start()) + 1
                errors.append(f"{relative}:{line}: {label}: {match.group(0)!r}")
        folded = text.casefold()
        for needle in local_needles:
            if needle.casefold() in folded:
                errors.append(f"{relative}: local privacy needle present")

    for path in DOCS:
        if not path.is_file():
            errors.append(f"missing public document: {path.relative_to(ROOT)}")
            continue
        text = path.read_text(encoding="utf-8")
        relative = path.relative_to(ROOT)
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
    print(
        f"PUBLIC TREE: {'PASS' if not errors else 'FAIL'} "
        f"({len(tracked_text_files())} tracked text files; {len(DOCS)} public docs)"
    )
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
