#!/usr/bin/env python3
"""Build and verify the exact tracked PlatformIO release archive."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "packages" / "platformio"
FORBIDDEN = {"_maintainer", "_work", ".git", "__pycache__", "build", "target"}


def fail(message: str) -> None:
    raise ValueError(message)


def package_files() -> tuple[Path, ...]:
    result = subprocess.run(
        ["git", "ls-files", "-z", "--", PACKAGE.relative_to(ROOT).as_posix()],
        cwd=ROOT,
        check=True,
        capture_output=True,
    )
    files = tuple(
        ROOT / raw.decode("utf-8")
        for raw in result.stdout.split(b"\0")
        if raw
    )
    if not files:
        fail("no tracked PlatformIO package files")
    for path in files:
        if not path.is_file() or path.is_symlink():
            fail(f"not a regular tracked package file: {path.relative_to(ROOT)}")
        relative = PurePosixPath(path.relative_to(PACKAGE).as_posix())
        if relative.is_absolute() or ".." in relative.parts or set(relative.parts) & FORBIDDEN:
            fail(f"unsafe package path: {relative}")
    return files


def archive_bytes() -> bytes:
    output = io.BytesIO()
    with gzip.GzipFile(fileobj=output, mode="wb", mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w") as archive:
            for path in package_files():
                relative = PurePosixPath(path.relative_to(PACKAGE).as_posix())
                data = path.read_bytes()
                info = tarfile.TarInfo((PurePosixPath("NobroRTOS") / relative).as_posix())
                info.size = len(data)
                info.mtime = 0
                info.uid = 0
                info.gid = 0
                info.uname = ""
                info.gname = ""
                info.mode = 0o644
                archive.addfile(info, io.BytesIO(data))
    return output.getvalue()


def verify(archive: Path) -> None:
    expected = {
        (PurePosixPath("NobroRTOS") / path.relative_to(PACKAGE).as_posix()).as_posix():
        hashlib.sha256(path.read_bytes()).hexdigest()
        for path in package_files()
    }
    with tarfile.open(archive, "r:gz") as handle:
        members = handle.getmembers()
        actual_names = {member.name for member in members}
        if actual_names != set(expected):
            fail(
                "archive membership drift: "
                f"missing={sorted(set(expected)-actual_names)}, "
                f"extra={sorted(actual_names-set(expected))}"
            )
        for member in members:
            if not member.isfile():
                fail(f"non-file archive member: {member.name}")
            stream = handle.extractfile(member)
            if stream is None:
                fail(f"unreadable archive member: {member.name}")
            if hashlib.sha256(stream.read()).hexdigest() != expected[member.name]:
                fail(f"archive content drift: {member.name}")


def version() -> str:
    return str(json.loads((PACKAGE / "library.json").read_text(encoding="utf-8"))["version"])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--archive", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.output is not None and not args.archive:
        parser.error("--output requires --archive")
    try:
        if args.archive:
            destination = (
                args.output
                if args.output is not None
                else ROOT / "_work" / f"NobroRTOS-PlatformIO-{version()}.tar.gz"
            )
            if not destination.is_absolute():
                destination = ROOT / destination
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(archive_bytes())
            verify(destination)
            print(
                f"wrote {destination} ({destination.stat().st_size} bytes, "
                "byte-matched to tracked source)"
            )
            return 0
        with tempfile.TemporaryDirectory(prefix="nobro-platformio-package-") as temporary:
            archive = Path(temporary) / "NobroRTOS.tar.gz"
            archive.write_bytes(archive_bytes())
            verify(archive)
        print("PLATFORMIO PACKAGE: PASS (deterministic, tracked, byte-matched)")
        return 0
    except (OSError, subprocess.CalledProcessError, tarfile.TarError, ValueError) as error:
        print(f"PLATFORMIO PACKAGE: FAIL ({error})")
        return 1


if __name__ == "__main__":
    sys.exit(main())
