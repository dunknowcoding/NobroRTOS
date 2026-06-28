#!/usr/bin/env python3
"""One-command verification orchestrator for NobroRTOS (M78).

Runs every host-testable check - the cargo unit/integration tests for the host-portable
crates plus the standalone host validators - collects results, and prints a unified
PASS/FAIL. Fully autonomous (no hardware); the board J-Link demos are separate HW gates.
Exit 0 = all green.
"""
import os
import subprocess
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), ".."))
CORE = os.path.join(ROOT, "core")
HOST_TARGET = "x86_64-pc-windows-msvc"
HOST_CRATES = [
    "nobro-net", "nobro-crypto", "nobro-ml", "nobro-sensor", "nobro-power",
    "nobro-sal", "nobro-kernel", "nobro-adapter-ina3221",
]


def run(name, cmd, cwd=ROOT, env=None):
    print(f"--- {name} ---", flush=True)
    r = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True)
    ok = r.returncode == 0
    for line in (r.stdout + r.stderr).strip().splitlines()[-3:]:
        print("   ", line)
    print(f"   => {'PASS' if ok else 'FAIL'}", flush=True)
    return ok


def main():
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = os.path.join(ROOT, "_work", "ct2")
    results = {}

    cargo = ["cargo", "test", "--target", HOST_TARGET]
    for c in HOST_CRATES:
        cargo += ["-p", c]
    results["cargo host tests"] = run("cargo host tests", cargo, cwd=CORE, env=env)
    results["board profiles"] = run("board profiles", [sys.executable, "tools/check_board_profiles.py"])
    results["sdk manifest"] = run("sdk manifest", [sys.executable, "tools/check_sdk_manifest.py"])
    results["mesh chaos"] = run("mesh chaos", [sys.executable, "tools/chaos_test.py"])

    print("\n=== SUMMARY ===")
    all_ok = True
    for k, v in results.items():
        print(f"  {'PASS' if v else 'FAIL'}  {k}")
        all_ok = all_ok and v
    print(f"RESULT: {'ALL PASS' if all_ok else 'FAIL'}")
    return 0 if all_ok else 1


if __name__ == "__main__":
    sys.exit(main())
