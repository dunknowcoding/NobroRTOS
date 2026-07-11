#!/usr/bin/env python3
"""Offline dependency source/license policy for the authoritative gate."""

import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CORE = os.path.join(ROOT, "core")
ALLOWED_LICENSE_TOKENS = {
    "MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib",
    "Unicode-3.0", "MPL-2.0", "0BSD", "Unlicense", "BSD-1-Clause",
}


def main():
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        cwd=CORE, text=True,
    ))
    failures = []
    checked = 0
    for package in metadata["packages"]:
        source = package.get("source")
        if source is None:
            continue
        checked += 1
        if not source.startswith("registry+https://github.com/rust-lang/crates.io-index"):
            failures.append(f"{package['name']}: unapproved source {source}")
        license_expr = package.get("license") or ""
        tokens = (license_expr.replace("(", " ").replace(")", " ").replace("/", " ")
                  .replace("OR", " ").replace("AND", " ").split())
        if not tokens or any(token not in ALLOWED_LICENSE_TOKENS for token in tokens):
            failures.append(f"{package['name']}: unapproved/missing license {license_expr!r}")
    if failures:
        print("DEPENDENCY POLICY: FAIL")
        print("\n".join(f"  {failure}" for failure in failures))
        return 1
    print(f"DEPENDENCY POLICY: PASS ({checked} external packages; locked sources/licenses)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
