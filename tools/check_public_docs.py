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
        "ARCHITECTURE.md", "PORTING.md", "LIMITATIONS.md", "CAMERA_SUPPORT.md",
        "WIRELESS_SUPPORT.md", "ERROR_CODES.md", "api-index.md",
    )),
]
LINK = re.compile(r"\[[^]]*\]\(([^)]+)\)")
TOOL = re.compile(
    r"(?:python(?:3(?:\.\d+)?)?|py)(?:\.exe)?"
    r"(?:\s+-[A-Za-z0-9_.=-]+)*\s+"
    r"(tools[\\/][A-Za-z0-9_.\\/-]+\.py)\b",
    re.IGNORECASE,
)
DOC_REFERENCE = re.compile(r"\bdocs[\\/][A-Za-z0-9_.-]+\.md\b")
DOC_REFERENCE_EXEMPT = {
    pathlib.PurePosixPath("tools/check_public_docs.py"),
    pathlib.PurePosixPath("tools/check_release_boundary.py"),
}
PRIVATE = {
    "non-public document reference": re.compile(
        r"(?<![A-Za-z0-9_.-])[A-Za-z0-9_.-]*_"
        r"(?:INTERNAL|PRIVATE)(?:\.[A-Za-z0-9_.-]+)?",
        re.IGNORECASE,
    ),
    "lab board label": re.compile(
        r"(?<![A-Za-z0-9])board[1-9][0-9]*(?![A-Za-z0-9])",
        re.IGNORECASE,
    ),
    "local serial port": re.compile(
        r"(?<![A-Za-z0-9])COM[0-9]+(?![A-Za-z0-9])", re.IGNORECASE
    ),
    "Windows machine path": re.compile(r"\b[A-Za-z]:\\"),
    "local environment": re.compile(r"\b(?:conda|venv)\s+(?:activate|env)\b", re.IGNORECASE),
}
TEXT_SUFFIXES = {
    ".c", ".cc", ".cpp", ".h", ".hpp", ".ino", ".j2", ".jinja", ".jinja2",
    ".json", ".md", ".ps1", ".py", ".rs", ".sh", ".template", ".tmpl", ".toml",
    ".txt", ".yaml", ".yml",
}


def tracked_text_files() -> list[pathlib.Path]:
    result = subprocess.run(
        ["git", "ls-files", "-z"], cwd=ROOT, capture_output=True, check=True
    )
    paths = [
        ROOT / raw.decode("utf-8")
        for raw in result.stdout.split(b"\0")
        if raw and pathlib.Path(raw.decode("utf-8")).suffix.lower() in TEXT_SUFFIXES
    ]
    # A pre-commit run may intentionally delete a tracked file. Validate the working
    # tree that will be committed instead of crashing while the deletion is unstaged.
    return [path for path in paths if path.is_file()]


def main() -> int:
    errors: list[str] = []
    tracked = {
        pathlib.PurePosixPath(raw.decode("utf-8").replace("\\", "/"))
        for raw in subprocess.run(
            ["git", "ls-files", "-z"], cwd=ROOT, capture_output=True, check=True
        ).stdout.split(b"\0")
        if raw
    }
    local_needles_path = ROOT / "tools" / "leak_needles.local.txt"
    local_needles = []
    if local_needles_path.is_file():
        local_needles = [
            line.strip()
            for line in local_needles_path.read_text(encoding="utf-8").splitlines()
            if line.strip() and not line.lstrip().startswith("#")
        ]

    text_files = tracked_text_files()
    for path in text_files:
        text = path.read_text(encoding="utf-8", errors="replace")
        relative = path.relative_to(ROOT)
        relative_folded = relative.as_posix().casefold()
        path_is_sensitive = any(
            needle.casefold() in relative_folded for needle in local_needles
        )
        display = "<redacted tracked path>" if path_is_sensitive else str(relative)
        if path_is_sensitive:
            errors.append("tracked path contains a local privacy marker")
        if "\ufffd" in text:
            lines = {
                text.count("\n", 0, index) + 1
                for index, value in enumerate(text)
                if value == "\ufffd"
            }
            errors.append(
                f"{display}:{','.join(map(str, sorted(lines)))}: "
                "Unicode replacement character (invalid or mojibake text)"
            )
        patterns = {} if path == pathlib.Path(__file__).resolve() else PRIVATE
        for label, pattern in patterns.items():
            for match in pattern.finditer(text):
                line = text.count("\n", 0, match.start()) + 1
                errors.append(f"{display}:{line}: {label}")
        folded = text.casefold()
        for needle in local_needles:
            if needle.casefold() in folded:
                errors.append(f"{display}: local privacy needle present")

        # Generator templates and source comments can publish documentation paths without
        # using Markdown-link syntax. Discover every such tracked literal automatically so
        # a newly added generator cannot escape a hardcoded allowlist. Product-contract
        # checkers are the exception: by design they name deliberately absent retired paths.
        if pathlib.PurePosixPath(relative.as_posix()) not in DOC_REFERENCE_EXEMPT:
            for match in DOC_REFERENCE.finditer(text):
                target = pathlib.PurePosixPath(match.group(0).replace("\\", "/"))
                target_file = ROOT / pathlib.Path(*target.parts)
                if not target_file.is_file():
                    line = text.count("\n", 0, match.start()) + 1
                    errors.append(
                        f"{display}:{line}: public documentation reference is missing"
                    )
                elif target not in tracked:
                    line = text.count("\n", 0, match.start()) + 1
                    errors.append(
                        f"{display}:{line}: public documentation reference is not tracked"
                    )

        # A command in any tracked Markdown file is part of the user experience, not
        # only commands in the primary documentation set. Require its tool to be a
        # safe, existing, tracked repository path on both POSIX and Windows examples.
        if path.suffix.lower() == ".md":
            for match in TOOL.finditer(text):
                target = pathlib.PurePosixPath(match.group(1).replace("\\", "/"))
                target_file = ROOT / pathlib.Path(*target.parts)
                safe = (
                    target.parts[:1] == ("tools",)
                    and all(part not in ("", ".", "..") for part in target.parts)
                )
                line = text.count("\n", 0, match.start()) + 1
                if not safe or not target_file.is_file():
                    errors.append(f"{display}:{line}: missing public CLI tool")
                elif target not in tracked:
                    errors.append(f"{display}:{line}: public CLI tool is not tracked")

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
            try:
                tracked_target = resolved.relative_to(ROOT.resolve())
            except ValueError:
                line = text.count("\n", 0, match.start()) + 1
                errors.append(f"{relative}:{line}: link leaves the repository")
                continue
            if not resolved.exists():
                line = text.count("\n", 0, match.start()) + 1
                errors.append(f"{relative}:{line}: broken local link")
                continue
            if not (
                pathlib.PurePosixPath(tracked_target.as_posix()) in tracked
                or (
                    resolved.is_dir()
                    and any(
                        p.is_relative_to(pathlib.PurePosixPath(tracked_target.as_posix()))
                        for p in tracked
                    )
                )
            ):
                line = text.count("\n", 0, match.start()) + 1
                errors.append(f"{relative}:{line}: local link target is not tracked")
    generated = subprocess.run(
        [sys.executable, "tools/gen_api_index.py", "--check"], cwd=ROOT
    )
    if generated.returncode:
        errors.append("generated API index is stale")
    error_codes = subprocess.run(
        [sys.executable, "tools/gen_error_codes.py", "--check"], cwd=ROOT
    )
    if error_codes.returncode:
        errors.append("generated error-code index is stale")
    for error in errors:
        print(f"FAIL: {error}")
    print(
        f"PUBLIC TREE: {'PASS' if not errors else 'FAIL'} "
        f"({len(text_files)} tracked text files; {len(DOCS)} public docs)"
    )
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
