#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Fail closed if the public repository exceeds the Core source boundary."""

from __future__ import annotations

from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
ALLOWED_ROOTS = {
    ".github",
    ".gitattributes",
    ".gitignore",
    "Cargo.lock",
    "Cargo.toml",
    "LICENSE",
    "NOTICE",
    "README.md",
    "README.ja.md",
    "README.zh-CN.md",
    "SECURITY.md",
    "ci",
    "crates",
    "docs",
    "examples",
    "library.json",
    "library.properties",
    "ports",
    "pyproject.toml",
    "rust-toolchain.toml",
    "src",
    "tools",
}
FORBIDDEN_SUFFIXES = {
    ".a", ".bin", ".d51", ".dll", ".elf", ".exe", ".hex", ".ihx",
    ".lib", ".log", ".map", ".o", ".obj", ".pyc", ".rel", ".uf2",
}
PRIVATE_PATTERNS = (
    re.compile(r"(?i)cybermcu|niussim|niusmultidebuggers|ironengineworld"),
    re.compile(r"(?i)(?:^|[/\\])_(?:maintainer|work)(?:[/\\]|$)"),
    re.compile(r"(?i)\bCOM\d+\b"),
    re.compile(r"(?i)\bW\d{2,}[A-Z0-9-]*\b"),
    re.compile(r"(?i)(?<![A-Z0-9])[A-Z]:[/\\]"),
    re.compile(r"(?i)pypi-[A-Za-z0-9_-]+"),
)
ADVANCED_SOURCE_PATTERNS = (
    re.compile(r"(?i)secure[_ -]?session|hardware[_ -]?root"),
    re.compile(r"(?i)multicore|cross[_ -]?core"),
    re.compile(r"(?i)L1Guarded|L2Managed|L3Assured"),
    re.compile(r"(?i)nobro_(?:secure|nn|ai|camera|audio|wireless|storage|hal)"),
)
SOURCE_SUFFIXES = {".c", ".h", ".py", ".rs", ".s", ".asm"}
PUBLIC_ASSETS = {
    "docs/images/discord_niusrobotlab.jpg",
    "docs/images/Nobro_full.png",
    "docs/images/Nobro_simple.png",
    "docs/images/real_tests.jpg",
    "docs/images/rtos_pure_radar_comparison.png",
    "docs/images/rtos_pure_technical_bar_ranking.png",
    "docs/images/rtos_pure_technical_ranking.png",
}


def repository_files() -> list[Path]:
    candidates = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        cwd=ROOT,
        check=True,
        capture_output=True,
    ).stdout
    names = [item.decode("utf-8") for item in candidates.split(b"\0") if item]
    return [ROOT / name for name in names]


def main() -> int:
    failures: list[str] = []
    files = repository_files()
    for path in files:
        if not path.exists():
            continue
        relative = path.relative_to(ROOT)
        if relative.parts[0] not in ALLOWED_ROOTS:
            failures.append(f"unapproved top-level path: {relative.as_posix()}")
        if path.is_symlink():
            failures.append(f"symlink is not permitted: {relative.as_posix()}")
        if path.suffix.lower() in FORBIDDEN_SUFFIXES:
            failures.append(f"generated/binary file is tracked: {relative.as_posix()}")
        if relative.as_posix() in PUBLIC_ASSETS:
            if path.suffix.lower() not in {".jpg", ".png"} or path.stat().st_size > 4_000_000:
                failures.append(f"invalid public image asset: {relative.as_posix()}")
            continue
        if path.stat().st_size > 1_000_000:
            failures.append(f"unexpected large file: {relative.as_posix()}")
        if path.name == "LICENSE":
            continue
        # This gate necessarily contains the expressions it enforces. Retain
        # all structural checks for the gate itself, but do not make its rule
        # literals self-incriminating.
        if relative.as_posix() == "tools/check_public_boundary.py":
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            failures.append(f"non-UTF-8 public file: {relative.as_posix()}")
            continue
        for pattern in PRIVATE_PATTERNS:
            if pattern.search(text):
                failures.append(
                    f"private identifier pattern in {relative.as_posix()}: {pattern.pattern}"
                )
        if path.suffix.lower() in SOURCE_SUFFIXES:
            for pattern in ADVANCED_SOURCE_PATTERNS:
                if pattern.search(text):
                    failures.append(
                        f"non-core source pattern in {relative.as_posix()}: {pattern.pattern}"
                    )

    cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    if 'members = ["crates/nobro-core"]' not in cargo:
        failures.append("workspace must contain only crates/nobro-core")
    if 'license = "GPL-3.0-only"' not in cargo:
        failures.append("workspace license is not GPL-3.0-only")
    if (ROOT / "core").exists() or (ROOT / "sdk").exists() or (ROOT / "packages").exists():
        failures.append("legacy full-tree source root is present")

    if failures:
        print("PUBLIC CORE BOUNDARY: FAIL")
        for failure in failures:
            print(f"- {failure}")
        return 1
    print(f"PUBLIC CORE BOUNDARY: PASS ({len(files)} files)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
