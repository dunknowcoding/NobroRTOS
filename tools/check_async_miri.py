#!/usr/bin/env python3
"""Run Miri over bounded async; AsyncCore intentionally has static lifetime."""

import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]


def host_target() -> str:
    output = subprocess.check_output(["rustc", "-vV"], text=True)
    return next(line.split(":", 1)[1].strip()
                for line in output.splitlines() if line.startswith("host:"))


def main() -> int:
    env = dict(os.environ)
    # Tests model embedded static-cell placement with Box::leak. Those allocations
    # are intentionally permanent; all other Miri checks remain enabled.
    flags = env.get("MIRIFLAGS", "").strip()
    env["MIRIFLAGS"] = f"{flags} -Zmiri-ignore-leaks".strip()
    command = ["cargo", "+nightly", "miri", "test", "--target", host_target(),
               "-p", "nobro-kernel", "async_rt::tests"]
    return subprocess.run(command, cwd=ROOT / "core", env=env).returncode


if __name__ == "__main__":
    sys.exit(main())
