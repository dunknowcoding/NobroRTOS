#!/usr/bin/env python3
"""THE verification orchestrator for NobroRTOS: one command, one verdict.

Runs every host-testable gate - cargo tests for the portable crates, the Python binding
suite, the software-surface contract, package checks (web flasher, block editor, SDK),
board profiles, tutorials, and mesh chaos - then prints a single summary. Fully
autonomous (no hardware); the board gates live in `nobro_hw_eval.py`. Exit 0 = all green.

When all gates pass, also emits an Evidence Pack (JSON + HTML) under `_work/evidence/`
via `nobro_verify.build_pack_from_results` - the "folded" display that ties gates to
the audit artifact without re-running them.

    python tools/run_checks.py            # everything + evidence pack on success
    python tools/run_checks.py --quick    # skip the slow cargo test gate

This wraps (never replaces) the narrower entry points, so `ci_matrix.sh` and
`nobro_contract_tool.py check-software-surface` keep working standalone.
"""
import argparse
import json
import os
import subprocess
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), ".."))
CORE = os.path.join(ROOT, "core")
HOST_CRATES = [
    "nobro-net", "nobro-crypto", "nobro-ml", "nobro-sensor", "nobro-power",
    "nobro-sal", "nobro-kernel", "nobro-classic", "nobro-control",
    "nobro-conformance", "nobro-database", "nobro-secure", "nobro-storage",
    "nobro-device", "nobro-iot", "nobro-nn", "nobro-ai", "nobro-host",
    "nobro-usb", "nobro-hal", "nobro-adapter-bmp280",
    "nobro-adapter-icm45686", "nobro-adapter-ina3221", "nobro-adapter-motion-ai",
    "nobro-adapter-mpu9250-imu", "nobro-adapter-nn-motion-ai",
    "nobro-adapter-radio-comms", "nobro-adapter-robo-servo",
    "nobro-adapter-ros-imu-bridge", "nobro-adapter-sensor-stub",
]


def host_target():
    """Return rustc's native host triple instead of assuming one developer OS."""
    override = os.environ.get("HOST_TARGET")
    if override:
        return override
    output = subprocess.check_output(["rustc", "-vV"], text=True)
    return next(line.split(":", 1)[1].strip()
                for line in output.splitlines() if line.startswith("host:"))


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


def gate_specs(quick, rust_only=False):
    """Return the ordered gate list as (name, cmd, cwd) tuples. Single source of truth
    shared by the CLI summary here and the `nobro verify` Evidence Pack."""
    py = sys.executable
    bindings = os.path.join(ROOT, "bindings", "python")
    specs = []
    if not quick:
        cargo = ["cargo", "test", "--target", host_target()]
        for c in HOST_CRATES:
            cargo += ["-p", c]
        specs.append(("cargo host tests", cargo, CORE))
        lint = ["cargo", "clippy", "--no-deps", "--target", host_target()]
        for c in HOST_CRATES:
            lint += ["-p", c]
        lint += ["--", "-D", "warnings"]
        specs.append(("cargo clippy", lint, CORE))
        specs.append(("cargo fmt", ["cargo", "fmt", "--all", "--", "--check"], CORE))
    if rust_only:
        return specs
    specs += [
        ("python bindings", [py, "-m", "unittest", "discover", "-s", "tests"], bindings),
        ("software surface", [py, "tools/nobro_contract_tool.py", "check-software-surface"], ROOT),
        ("public docs", [py, "tools/check_public_docs.py"], ROOT),
        ("board profiles", [py, "tools/check_board_profiles.py"], ROOT),
        ("sdk manifest", [py, "tools/check_sdk_manifest.py"], ROOT),
        ("arduino package", [py, "tools/package_arduino.py", "--check"], ROOT),
        ("web flasher", [py, "tools/check_web_flasher.py"], ROOT),
        ("block editor", [py, "tools/check_block_editor.py"], ROOT),
        ("tutorials", [py, "tools/tutorial_runner.py"], ROOT),
        ("app catalog", [py, "tools/nobro_app.py", "tutorials/hello-device/app.json"], ROOT),
        ("ros msg codegen", [py, "tools/ros_msg_gen.py", "--selftest"], ROOT),
        ("dts import", [py, "tools/import_dts.py", "--selftest"], ROOT),
        ("wasm slot spike", [py, "tools/wasm_slot_spike.py", "--selftest"], ROOT),
        ("evidence pack smoke", [py, "tools/nobro_verify.py", "--selftest"], ROOT),
        ("prebuilt uf2 loop", [py, "tools/package_prebuilt_uf2.py", "--check"], ROOT),
        ("tier-c link", [py, "tools/build_libnobro.py", "--check"], ROOT),
        ("fleet evidence", [py, "tools/fleet_evidence.py", "--selftest"], ROOT),
        ("release versions", [py, "tools/check_release_versions.py"], ROOT),
        ("ota preflight", [py, "tools/ota_preflight_demo.py"], ROOT),
        ("ros bridge contract", [py, "tools/check_ros_bridge.py", "--selftest"], ROOT),
        ("udi surface", [py, "tools/check_udi.py", "--selftest"], ROOT),
        ("mesh chaos", [py, "tools/chaos_test.py"], ROOT),
    ]
    return specs


def run_gates(quick=False, quiet=False, rust_only=False):
    """Run every gate; return (results, all_ok). Results are dicts (name/ok/detail)."""
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = os.path.join(ROOT, "_work", "ct2")
    results = [run(name, cmd, cwd=cwd, env=env, quiet=quiet)
               for name, cmd, cwd in gate_specs(quick, rust_only=rust_only)]
    all_ok = all(r["ok"] for r in results)
    return results, all_ok


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--quick", action="store_true", help="skip the slow cargo test gate")
    ap.add_argument("--no-evidence", action="store_true",
                    help="skip Evidence Pack emission even when all gates pass")
    ap.add_argument("--rust-only", action="store_true",
                    help="run the comprehensive portable Rust test/lint/format gates only")
    args = ap.parse_args()

    results, all_ok = run_gates(quick=args.quick, rust_only=args.rust_only)

    print("\n=== SUMMARY ===")
    for r in results:
        print(f"  {'PASS' if r['ok'] else 'FAIL'}  {r['name']}")
    print(f"RESULT: {'ALL PASS' if all_ok else 'FAIL'}")

    if all_ok and not args.no_evidence and not args.rust_only:
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        import nobro_verify
        pack, _ = nobro_verify.build_pack_from_results(results, quick=args.quick)
        out_dir = os.path.join(ROOT, "_work", "evidence")
        os.makedirs(out_dir, exist_ok=True)
        json_path = os.path.join(out_dir, "evidence_pack.json")
        html_path = os.path.join(out_dir, "evidence_pack.html")
        with open(json_path, "w", encoding="utf-8") as f:
            json.dump(pack, f, indent=2)
        with open(html_path, "w", encoding="utf-8") as f:
            f.write(nobro_verify.render_html(pack))
        s = pack["summary"]
        b = pack["budgets"]
        print(f"\n=== EVIDENCE PACK ===")
        print(f"  {s['result']} ({s['passed']}/{s['total']} gates)")
        if b.get("available"):
            print(f"  budgets: {len(b['targets'])} app(s) priced")
        else:
            print(f"  budgets: {b.get('note')}")
        print(f"  JSON: {os.path.relpath(json_path, ROOT)}")
        print(f"  HTML: {os.path.relpath(html_path, ROOT)}")

    return 0 if all_ok else 1


if __name__ == "__main__":
    sys.exit(main())
