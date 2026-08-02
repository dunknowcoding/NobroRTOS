#!/usr/bin/env python3
"""Run the reproducible source and package checks for NobroRTOS.

Runs every host-testable gate - cargo tests for the portable crates, the Python binding
suite, the software-surface contract, package checks (web flasher, block editor, SDK),
board profiles, tutorials, and integration catalogs, then prints a single summary.
It contains no machine-specific configuration. Exit 0 means every selected check passed.

    python tools/checks/run_checks.py            # all default checks
    python tools/checks/run_checks.py --quick    # skip the slow cargo test gate

This wraps (never replaces) the narrower entry points, so `ci_matrix.sh` and
`python sdk/cli/nobro.py contract check-software-surface` remain standalone.
"""
import argparse
import os
import subprocess
import sys

ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", ".."))
CORE = os.path.join(ROOT, "core")


def console_text(value):
    """Keep tool diagnostics printable on non-UTF-8 Windows consoles."""
    encoding = getattr(sys.stdout, "encoding", None) or "utf-8"
    return value.encode(encoding, errors="backslashreplace").decode(encoding)


def bash_executable():
    """Prefer an explicit PATH-provided POSIX shell over the WSL launcher."""
    if os.name != "nt":
        return "bash"
    for directory in os.environ.get("PATH", "").split(os.pathsep):
        normalized = os.path.normcase(os.path.normpath(directory))
        candidate = os.path.join(directory, "bash.exe")
        if os.path.isfile(candidate) and (
            "msys" in normalized or normalized.endswith(os.path.normcase(r"\git\bin"))
        ):
            return candidate
    return "bash"


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
            print("   ", console_text(line))
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
                      [py, "tools/checks/core/check_dependency_policy.py"], ROOT))
        specs.append((
            "fail-closed host workspace",
            [py, "tools/checks/core/check_host_workspace.py"],
            ROOT,
        ))
        specs.append((
            "vendored nRF USBD regression tests",
            ["cargo", "test", "--locked", "--target", host_target(), "-p", "nrf-usbd"],
            CORE,
        ))
        specs.append((
            "wireless optional feature tests",
            ["cargo", "test", "--locked", "--target", host_target(),
             "-p", "nobro-wireless", "--all-features"],
            CORE,
        ))
        specs.append((
            "optional application service tests",
            ["cargo", "test", "--locked", "--target", host_target(),
             "-p", "nobro-services", "--all-features"],
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
        specs.append((
            "secure symmetric-only build",
            ["cargo", "check", "--locked", "--target", host_target(),
             "-p", "nobro-secure", "--no-default-features"],
            CORE,
        ))
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
        specs.append((
            "optional application service clippy",
            ["cargo", "clippy", "--locked", "--no-deps", "--all-targets",
             "--target", host_target(), "-p", "nobro-services", "--all-features",
             "--", "-D", "warnings"],
            CORE,
        ))
        specs.append(("cargo fmt", ["cargo", "fmt", "--all", "--", "--check"], CORE))
        specs.append(("nano kernel build/admission/symbol budgets",
                      [py, "tools/checks/core/check_nano_kernel.py"], ROOT))
        specs += [
            ("USB RA4M1 backend host tests", ["cargo", "test", "--locked", "--target", host_target(),
             "-p", "nobro-usb", "--no-default-features", "--features", "backend-ra-usbfs"], CORE),
            ("USB Serial/JTAG ESP32-C3 backend host tests", ["cargo", "test", "--locked", "--target", host_target(),
              "-p", "nobro-usb", "--no-default-features", "--features",
             "backend-usb-serial-jtag-esp32c3"], CORE),
            ("USB Serial/JTAG ESP32-P4 backend host tests", ["cargo", "test", "--locked", "--target", host_target(),
             "-p", "nobro-usb", "--no-default-features", "--features",
             "backend-usb-serial-jtag-esp32p4"], CORE),
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
                ("Miri bounded async", [py, "tools/checks/core/check_async_miri.py"], ROOT),
            ]
    if rust_only:
        return specs
    specs += [
        ("release boundary", [py, "tools/checks/release/check_release_boundary.py"], ROOT),
        ("CI evidence hermeticity", [py, "tools/checks/release/check_ci_hermeticity.py", "--selftest"], ROOT),
        ("accounting semantics", [py, "tools/checks/core/check_accounting_semantics.py"], ROOT),
        ("deadline masking", [py, "tools/checks/core/check_timebase_masking.py"], ROOT),
        ("python bindings", [py, "-m", "unittest", "discover", "-s", "tests"], bindings),
        ("software surface", [py, "sdk/cli/nobro.py", "contract", "check-software-surface"], ROOT),
        ("public docs", [py, "tools/checks/release/check_public_docs.py"], ROOT),
        (
            "report trust boundary",
            [py, "tools/checks/core/check_report_trust_boundary.py"],
            ROOT,
        ),
        ("static budget analyzer", [py, "sdk/cli/nobro.py", "budget", "--selftest"], ROOT),
        ("flash tool fail-closed parser", [py, "sdk/cli/nobro.py", "flash", "--selftest"], ROOT),
        ("board profiles", [py, "tools/checks/platforms/check_board_profiles.py"], ROOT),
        ("kernel-lite candidates", [py, "tools/checks/platforms/check_kernel_lite_candidates.py"], ROOT),
        ("core layout", [py, "tools/checks/core/check_core_layout.py"], ROOT),
        ("api contract", [py, "tools/build/gen_api_contract.py", "--check"], ROOT),
        ("claim contract", [py, "tools/checks/core/check_claim_contract.py"], ROOT),
        ("sdk manifest", [py, "tools/checks/release/check_sdk_manifest.py"], ROOT),
        ("arduino package", [py, "tools/release/package_arduino.py", "--check"], ROOT),
        ("distribution artifacts", [py, "tools/checks/release/check_distribution_artifacts.py"], ROOT),
        ("PlatformIO release archive", [py, "tools/release/package_platformio.py", "--check"], ROOT),
        ("arduino representative compile", [py, "tools/checks/integrations/check_arduino_compile.py"], ROOT),
        ("arduino facade contracts", [py, "tools/checks/integrations/check_arduino_facade.py"], ROOT),
        (
            "provider lifecycle contracts",
            [
                py,
                "tools/checks/platforms/check_provider_lifecycle.py",
                "--target",
                host_target(),
            ],
            ROOT,
        ),
        ("ArduinoBLE UNO R4 integrations",
         [py, "tools/checks/integrations/check_arduino_ble_integrations.py"], ROOT),
        ("Arduino-ESP32 BLE integrations",
         [py, "tools/checks/integrations/check_arduino_esp_ble.py"], ROOT),
        ("audio facade contracts", [py, "tools/checks/integrations/check_audio_facade.py"], ROOT),
        ("camera facade contracts", [py, "tools/checks/integrations/check_camera_facade.py"], ROOT),
        ("crypto facade contracts", [py, "tools/checks/integrations/check_crypto_facade.py"], ROOT),
        ("display facade contracts", [py, "tools/checks/integrations/check_display_facade.py"], ROOT),
        ("servo facade contracts", [py, "tools/checks/integrations/check_servo_facade.py"], ROOT),
        ("Thread facade contracts", [py, "tools/checks/integrations/check_thread_facade.py"], ROOT),
        ("Zigbee facade contracts", [py, "tools/checks/integrations/check_zigbee_facade.py"], ROOT),
        ("ESP32 peripheral facade contracts",
         [py, "tools/checks/integrations/check_esp32_peripheral_facade.py"], ROOT),
        ("NiusIMU adapter contracts", [py, "tools/checks/integrations/check_niusimu_adapter.py", "--selftest"], ROOT),
        ("sensor member facades", [py, "tools/checks/integrations/check_sensor_member_facades.py"], ROOT),
        ("web flasher", [py, "tools/checks/product/check_web_flasher.py"], ROOT),
        ("block editor", [py, "tools/checks/product/check_block_editor.py"], ROOT),
        ("tutorials", [py, "sdk/cli/nobro.py", "tutorials"], ROOT),
        ("app catalog", [py, "sdk/cli/nobro.py", "app", "tutorials/hello-device/app.json"], ROOT),
        ("app authoring parity", [py, "tools/checks/product/check_app_authoring.py"], ROOT),
        ("ros msg codegen", [py, "sdk/cli/nobro.py", "ros-msg", "--selftest"], ROOT),
        ("dts import", [py, "sdk/cli/nobro.py", "import-dts", "--selftest"], ROOT),
        ("prebuilt uf2 loop", [py, "tools/release/package_prebuilt_uf2.py", "--check"], ROOT),
        ("tier-c link", [py, "tools/build/build_libnobro.py", "--check"], ROOT),
        ("admission analysis", [py, "sdk/cli/nobro.py", "admit", "--selftest"], ROOT),
        ("capacity right-sizing", [py, "sdk/cli/nobro.py", "shrink", "--selftest"], ROOT),
        ("HAL/SAL contract", [py, "tools/checks/platforms/check_hal_contract.py", "--selftest"], ROOT),
        ("platform tiers", [py, "tools/checks/platforms/check_platform_tiers.py", "--selftest"], ROOT),
        ("board-feature registry", [py, "tools/checks/platforms/check_board_features.py", "--selftest"], ROOT),
        ("adapter catalog", [py, "tools/checks/integrations/check_adapter_catalog.py"], ROOT),
        (
            "portable adapter contract",
            [py, "tools/checks/integrations/check_portable_adapter_contract.py"],
            ROOT,
        ),
        ("adapter scaffold", [py, "sdk/cli/nobro.py", "adapter", "--selftest"], ROOT),
        ("firmware project", [py, "sdk/cli/nobro.py", "firmware", "--selftest"], ROOT),
        ("project experience", [py, "sdk/cli/nobro.py", "project", "--selftest"], ROOT),
        ("release versions", [py, "tools/checks/release/check_release_versions.py", "--release"], ROOT),
        ("ros bridge contract", [py, "tools/checks/integrations/check_ros_bridge.py", "--selftest"], ROOT),
        ("udi surface", [py, "tools/checks/product/check_udi.py", "--selftest"], ROOT),
    ]
    if extended:
        specs.append(
            (
                "cross-MCU matrix",
                [bash_executable(), "tools/checks/ci_matrix.sh"],
                ROOT,
            )
        )
    return specs


def run_gates(quick=False, quiet=False, rust_only=False, extended=False):
    """Run every gate; return (results, all_ok). Results are dicts (name/ok/detail)."""
    env = dict(os.environ)
    # Nested shell gates must use the same Python environment as this runner.
    python_dir = os.path.dirname(sys.executable)
    env["PATH"] = python_dir + os.pathsep + env.get("PATH", "")
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
