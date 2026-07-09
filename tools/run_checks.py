#!/usr/bin/env python3
"""THE verification orchestrator for NobroRTOS: one command, one verdict.

Runs every host-testable gate - cargo tests for the portable crates, the Python binding
suite, the software-surface contract, package checks (web flasher, block editor, SDK),
board profiles, tutorials, and mesh chaos - then prints a single summary. Fully
autonomous (no hardware); the board gates live in `nobro_hw_eval.py`. Exit 0 = all green.

    python tools/run_checks.py            # everything
    python tools/run_checks.py --quick    # skip the slow cargo test gate

This wraps (never replaces) the narrower entry points, so `ci_matrix.sh` and
`nobro_contract_tool.py check-software-surface` keep working standalone.
"""
import argparse
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
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--quick", action="store_true", help="skip the slow cargo test gate")
    args = ap.parse_args()

    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = os.path.join(ROOT, "_work", "ct2")
    results = {}

    if not args.quick:
        cargo = ["cargo", "test", "--target", HOST_TARGET]
        for c in HOST_CRATES:
            cargo += ["-p", c]
        results["cargo host tests"] = run("cargo host tests", cargo, cwd=CORE, env=env)
    results["python bindings"] = run(
        "python bindings",
        [sys.executable, "-m", "unittest", "discover", "-s", "tests"],
        cwd=os.path.join(ROOT, "bindings", "python"),
    )
    results["software surface"] = run(
        "software surface",
        [sys.executable, "tools/nobro_contract_tool.py", "check-software-surface"],
    )
    results["board profiles"] = run("board profiles", [sys.executable, "tools/check_board_profiles.py"])
    results["sdk manifest"] = run("sdk manifest", [sys.executable, "tools/check_sdk_manifest.py"])
    results["arduino package"] = run("arduino package", [sys.executable, "tools/package_arduino.py", "--check"])
    results["web flasher"] = run("web flasher", [sys.executable, "tools/check_web_flasher.py"])
    results["block editor"] = run("block editor", [sys.executable, "tools/check_block_editor.py"])
    results["tutorials"] = run("tutorials", [sys.executable, "tools/tutorial_runner.py"])
    results["app catalog"] = run(
        "app catalog",
        [sys.executable, "tools/nobro_app.py", "tutorials/hello-device/app.json"],
    )
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
