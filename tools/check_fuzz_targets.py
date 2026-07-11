#!/usr/bin/env python3
"""Validate persistent fuzz targets/corpora and compile their harnesses."""

import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FUZZ = os.path.join(ROOT, "core", "fuzz")
TARGETS = ("wire_inputs", "database_images", "control_state", "abi_lengths")


def main():
    failures = []
    for target in TARGETS:
        source = os.path.join(FUZZ, "fuzz_targets", f"{target}.rs")
        corpus = os.path.join(FUZZ, "corpus", target)
        if not os.path.isfile(source):
            failures.append(f"missing target {target}")
        if not os.path.isdir(corpus) or not os.listdir(corpus):
            failures.append(f"missing persistent corpus {target}")
    if not failures:
        result = subprocess.run(
            ["cargo", "check", "--manifest-path", os.path.join(FUZZ, "Cargo.toml"), "--bins"],
            cwd=ROOT,
        )
        if result.returncode:
            failures.append("fuzz harness compilation failed")
    if failures:
        print("FUZZ TARGETS: FAIL")
        print("\n".join(f"  {failure}" for failure in failures))
        return 1
    print(f"FUZZ TARGETS: PASS ({len(TARGETS)} targets with persistent corpora)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
