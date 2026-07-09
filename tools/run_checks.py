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


def run(name, cmd, cwd=ROOT, env=None, quiet=False):
    if not quiet:
        print(f"--- {name} ---", flush=True)
    r = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True)
    ok = r.returncode == 0
    tail = (r.stdout + r.stderr).strip().splitlines()[-3:]
    if not quiet:
        for line in tail:
            print("   ", line)
        print(f"   => {'PASS' if ok else 'FAIL'}", flush=True)
    return {"name": name, "ok": ok, "detail": tail}


def gate_specs(quick):
    """Return the ordered gate list as (name, cmd, cwd) tuples. Single source of truth
    shared by the CLI summary here and the `nobro verify` Evidence Pack."""
    py = sys.executable
    bindings = os.path.join(ROOT, "bindings", "python")
    specs = []
    if not quick:
        cargo = ["cargo", "test", "--target", HOST_TARGET]
        for c in HOST_CRATES:
            cargo += ["-p", c]
        specs.append(("cargo host tests", cargo, CORE))
    specs += [
        ("python bindings", [py, "-m", "unittest", "discover", "-s", "tests"], bindings),
        ("software surface", [py, "tools/nobro_contract_tool.py", "check-software-surface"], ROOT),
        ("board profiles", [py, "tools/check_board_profiles.py"], ROOT),
        ("sdk manifest", [py, "tools/check_sdk_manifest.py"], ROOT),
        ("arduino package", [py, "tools/package_arduino.py", "--check"], ROOT),
        ("web flasher", [py, "tools/check_web_flasher.py"], ROOT),
        ("block editor", [py, "tools/check_block_editor.py"], ROOT),
        ("tutorials", [py, "tools/tutorial_runner.py"], ROOT),
        ("app catalog", [py, "tools/nobro_app.py", "tutorials/hello-device/app.json"], ROOT),
        ("mesh chaos", [py, "tools/chaos_test.py"], ROOT),
    ]
    return specs


def run_gates(quick=False, quiet=False):
    """Run every gate; return (results, all_ok). Results are dicts (name/ok/detail)."""
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = os.path.join(ROOT, "_work", "ct2")
    results = [run(name, cmd, cwd=cwd, env=env, quiet=quiet)
               for name, cmd, cwd in gate_specs(quick)]
    all_ok = all(r["ok"] for r in results)
    return results, all_ok


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--quick", action="store_true", help="skip the slow cargo test gate")
    args = ap.parse_args()

    results, all_ok = run_gates(quick=args.quick)

    print("\n=== SUMMARY ===")
    for r in results:
        print(f"  {'PASS' if r['ok'] else 'FAIL'}  {r['name']}")
    print(f"RESULT: {'ALL PASS' if all_ok else 'FAIL'}")
    return 0 if all_ok else 1


if __name__ == "__main__":
    sys.exit(main())
