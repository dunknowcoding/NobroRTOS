#!/usr/bin/env python3
"""Validate the standalone SDK manifest (M77).

Loads sdk/sdk-manifest.json and confirms every path it references actually resolves in
the repo (canonical contract, core workspace, include roots, host tools, python package,
and the Arduino/PlatformIO package surfaces). Pure stdlib; exit 0 = manifest is coherent.
"""
import json
import os
import sys

ROOT = os.path.join(os.path.dirname(__file__), "..")


def rel(p):
    return os.path.normpath(os.path.join(ROOT, p))


def main():
    man = json.load(open(os.path.join(ROOT, "sdk", "sdk-manifest.json")))
    checks = []
    checks.append(("canonical_contract", man["canonical_contract"]))
    checks.append(("core_workspace", man["core_workspace"]))
    for r in man.get("include_roots", []):
        checks.append(("include_root", r))
    for t in man.get("host_tools", []):
        checks.append(("host_tool", t))
    checks.append(("python_package", man["python_package"]))
    for name, path in man.get("package_surfaces", {}).items():
        checks.append((f"package_surface:{name}", path))

    missing = 0
    for kind, path in checks:
        ok = os.path.exists(rel(path))
        print(f"[{'OK ' if ok else 'MISS'}] {kind:22} {path}")
        if not ok:
            missing += 1
    print(f"RESULT: {'PASS' if missing == 0 else 'FAIL'} ({len(checks)-missing}/{len(checks)} resolve)")
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
