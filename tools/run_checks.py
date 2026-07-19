#!/usr/bin/env python3
"""Run the reproducible source and package checks for NobroRTOS.

Runs every host-testable gate - cargo tests for the portable crates, the Python binding
suite, the software-surface contract, package checks (web flasher, block editor, SDK),
board profiles, tutorials, and integration catalogs, then prints a single summary.
It contains no machine-specific configuration. Exit 0 means every selected check passed.

    python tools/run_checks.py            # all default checks
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
HOST_CRATES = [
    "nobro-admission", "nobro-net", "nobro-crypto", "nobro-ml", "nobro-imu", "nobro-sensor", "nobro-power",
    "nobro-sal", "nobro-kernel", "nobro-classic", "nobro-control",
    "nobro-database", "nobro-secure", "nobro-storage",
    "nobro-device", "nobro-wireless", "nobro-camera", "nobro-nn", "nobro-ai", "nobro-host",
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
    output = subprocess.check_output(
        ["rustc", "-vV"], text=True, encoding="utf-8", errors="replace"
    )
    return next(line.split(":", 1)[1].strip()
                for line in output.splitlines() if line.startswith("host:"))


def run(name, cmd, cwd=ROOT, env=None, quiet=False):
    if not quiet:
        print(f"--- {name} ---", flush=True)
    r = subprocess.run(
        cmd, cwd=cwd, env=env, capture_output=True, text=True,
        encoding="utf-8", errors="replace",
    )
    ok = r.returncode == 0
    tail = ((r.stdout or "") + (r.stderr or "")).strip().splitlines()[-3:]
    if not quiet:
        for line in tail:
            print("   ", line)
        print(f"   => {'PASS' if ok else 'FAIL'}", flush=True)
    return {"name": name, "ok": ok, "detail": tail}


def gate_specs(quick, rust_only=False, extended=False):
    """Return the canonical local gate list as (name, cmd, cwd) tuples.

    Hosted jobs reuse individual entry points where their toolchains differ; workflow
    receipt bindings keep those selected commands explicit and fail closed on drift.
    """
    py = sys.executable
    bindings = os.path.join(ROOT, "bindings", "python")
    specs = []
    if not quick:
        specs.append(("dependency source/license policy",
                      [py, "tools/check_dependency_policy.py"], ROOT))
        cargo = ["cargo", "test", "--locked", "--target", host_target()]
        for c in HOST_CRATES:
            cargo += ["-p", c]
        specs.append(("cargo host tests", cargo, CORE))
        specs.append((
            "vendored nRF USBD regression tests",
            ["cargo", "test", "--locked", "--target", host_target(), "-p", "nrf-usbd"],
            CORE,
        ))
        specs.append((
            "kernel capacity-report feature tests",
            ["cargo", "test", "--locked", "--target", host_target(),
             "-p", "nobro-kernel", "--features", "capacity-report"],
            CORE,
        ))
        specs.append((
            "kernel preemption feature tests",
            ["cargo", "test", "--locked", "--target", host_target(),
             "-p", "nobro-kernel", "--features", "preemptive"],
            CORE,
        ))
        lint = ["cargo", "clippy", "--locked", "--no-deps", "--target", host_target()]
        for c in HOST_CRATES:
            lint += ["-p", c]
        lint += ["--", "-D", "warnings"]
        specs.append(("cargo clippy", lint, CORE))
        specs.append((
            "kernel capacity-report feature clippy",
            ["cargo", "clippy", "--locked", "--no-deps", "--all-targets",
             "--target", host_target(), "-p", "nobro-kernel", "--features",
             "capacity-report", "--", "-D", "warnings"],
            CORE,
        ))
        specs.append((
            "kernel preemption feature clippy",
            ["cargo", "clippy", "--locked", "--no-deps", "--all-targets",
             "--target", host_target(), "-p", "nobro-kernel", "--features",
             "preemptive", "--", "-D", "warnings"],
            CORE,
        ))
        specs.append(("cargo fmt", ["cargo", "fmt", "--all", "--", "--check"], CORE))
        specs.append(("nano kernel build/admission/symbol budgets",
                      [py, "tools/check_nano_kernel.py"], ROOT))
        specs += [
            ("USB RA4M1 backend host tests", ["cargo", "test", "--locked", "--target", host_target(),
             "-p", "nobro-usb", "--no-default-features", "--features", "backend-ra-usbfs"], CORE),
            ("USB Serial/JTAG ESP32-C3 backend host tests", ["cargo", "test", "--locked", "--target", host_target(),
              "-p", "nobro-usb", "--no-default-features", "--features",
             "backend-usb-serial-jtag-esp32c3"], CORE),
            ("USB Serial/JTAG ESP32-S3 backend host tests", ["cargo", "test", "--locked", "--target", host_target(),
             "-p", "nobro-usb", "--no-default-features", "--features",
             "backend-usb-serial-jtag-esp32s3"], CORE),
            ("RA4M1 provider conformance", ["cargo", "test", "--locked", "--lib", "--target",
             host_target()], os.path.join(CORE, "ports", "ra4m1")),
        ]
        if extended:
            specs += [
                ("cargo advisory audit", ["cargo", "audit", "--file", "Cargo.lock"], CORE),
                ("Rust coverage", ["cargo", "llvm-cov", "--locked", "--target", host_target(),
                 "-p", "nobro-kernel", "-p", "nobro-net", "-p", "nobro-secure",
                 "-p", "nobro-storage", "-p", "nobro-database", "-p", "nobro-power",
                 "-p", "nobro-sal", "--lcov", "--output-path",
                 os.path.join(ROOT, "_work", "coverage.lcov")], CORE),
                ("Miri portable safety", ["cargo", "+nightly", "miri", "test", "--locked",
                 "--target", host_target(),
                 "-p", "nobro-database", "-p", "nobro-storage", "-p", "nobro-net",
                 "-p", "nobro-secure", "-p", "nobro-hal"], CORE),
                ("Miri bounded async", [py, "tools/check_async_miri.py"], ROOT),
            ]
    if rust_only:
        return specs
    specs += [
        ("release boundary", [py, "tools/check_release_boundary.py"], ROOT),
        ("accounting semantics", [py, "tools/check_accounting_semantics.py"], ROOT),
        ("deadline masking", [py, "tools/check_timebase_masking.py"], ROOT),
        ("python bindings", [py, "-m", "unittest", "discover", "-s", "tests"], bindings),
        ("software surface", [py, "tools/nobro_contract_tool.py", "check-software-surface"], ROOT),
        ("public docs", [py, "tools/check_public_docs.py"], ROOT),
        ("static budget analyzer", [py, "tools/static_budget.py", "--selftest"], ROOT),
        ("flash tool fail-closed parser", [py, "tools/flash.py", "--selftest"], ROOT),
        ("board profiles", [py, "tools/check_board_profiles.py"], ROOT),
        ("core layout", [py, "tools/check_core_layout.py"], ROOT),
        ("sdk manifest", [py, "tools/check_sdk_manifest.py"], ROOT),
        ("arduino package", [py, "tools/package_arduino.py", "--check"], ROOT),
        ("distribution artifacts", [py, "tools/check_distribution_artifacts.py"], ROOT),
        ("PlatformIO release archive", [py, "tools/package_platformio.py", "--check"], ROOT),
        ("arduino representative compile", [py, "tools/check_arduino_compile.py"], ROOT),
        ("arduino facade contracts", [py, "tools/check_arduino_facade.py"], ROOT),
        ("audio facade contracts", [py, "tools/check_audio_facade.py"], ROOT),
        ("NiusIMU adapter contracts", [py, "tools/check_niusimu_adapter.py", "--selftest"], ROOT),
        ("web flasher", [py, "tools/check_web_flasher.py"], ROOT),
        ("block editor", [py, "tools/check_block_editor.py"], ROOT),
        ("tutorials", [py, "tools/tutorial_runner.py"], ROOT),
        ("app catalog", [py, "tools/nobro_app.py", "tutorials/hello-device/app.json"], ROOT),
        ("app authoring parity", [py, "tools/check_app_authoring.py"], ROOT),
        ("ros msg codegen", [py, "tools/ros_msg_gen.py", "--selftest"], ROOT),
        ("dts import", [py, "tools/import_dts.py", "--selftest"], ROOT),
        ("prebuilt uf2 loop", [py, "tools/package_prebuilt_uf2.py", "--check"], ROOT),
        ("tier-c link", [py, "tools/build_libnobro.py", "--check"], ROOT),
        ("admission analysis", [py, "tools/nobro_admission.py", "--selftest"], ROOT),
        ("capacity right-sizing", [py, "tools/nobro_shrink.py", "--selftest"], ROOT),
        ("platform tiers", [py, "tools/check_platform_tiers.py", "--selftest"], ROOT),
        ("board-feature registry", [py, "tools/check_board_features.py", "--selftest"], ROOT),
        ("adapter catalog", [py, "tools/check_adapter_catalog.py"], ROOT),
        ("adapter scaffold", [py, "tools/nobro_adapter.py", "--selftest"], ROOT),
        ("firmware project", [py, "tools/nobro_firmware_project.py", "--selftest"], ROOT),
        ("project experience", [py, "sdk/cli/nobro.py", "project", "--selftest"], ROOT),
        ("release versions", [py, "tools/check_release_versions.py", "--release"], ROOT),
        ("ros bridge contract", [py, "tools/check_ros_bridge.py", "--selftest"], ROOT),
        ("udi surface", [py, "tools/check_udi.py", "--selftest"], ROOT),
    ]
    if extended:
        specs.append(("cross-MCU matrix", ["bash", "tools/ci_matrix.sh"], ROOT))
    return specs


def run_gates(quick=False, quiet=False, rust_only=False, extended=False):
    """Run every gate; return (results, all_ok). Results are dicts (name/ok/detail)."""
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = os.path.join(ROOT, "_work", "ct2")
    results = [run(name, cmd, cwd=cwd, env=env, quiet=quiet)
               for name, cmd, cwd in gate_specs(quick, rust_only=rust_only, extended=extended)]
    all_ok = all(r["ok"] for r in results)
    return results, all_ok


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--quick", action="store_true", help="skip the slow cargo test gate")
    ap.add_argument("--rust-only", action="store_true",
                    help="run the comprehensive portable Rust test/lint/format gates only")
    ap.add_argument("--extended", action="store_true",
                    help="also require audit, coverage, Miri, and cross-MCU gates")
    args = ap.parse_args()

    results, all_ok = run_gates(
        quick=args.quick, rust_only=args.rust_only, extended=args.extended
    )

    print("\n=== SUMMARY ===")
    for r in results:
        print(f"  {'PASS' if r['ok'] else 'FAIL'}  {r['name']}")
    print(f"RESULT: {'ALL PASS' if all_ok else 'FAIL'}")

    return 0 if all_ok else 1


if __name__ == "__main__":
    sys.exit(main())
