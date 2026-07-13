#!/usr/bin/env python3
"""Keep private automation, machine details, and planning metadata untracked."""

import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "sdk" / "sdk-manifest.json"
FORBIDDEN_REFERENCES = (
    "tools/internal/",
    "tools/dev/",
    "core/baselines/",
    "core/fuzz/",
    "core/internal/",
    "core/ecosystem/",
    "measure_baselines.py",
    "measure_complex_runtime.py",
    "measure_embassy_variants.py",
    "measure_authoring.py",
    "nobro_hw_eval.py",
    "hil_matrix.py",
    "wasm_slot_spike.py",
    "kernel_wcet_demo",
    "kernel_selftest",
    "hil_fault_demo",
    "nobro_verify.py",
    "fleet_evidence.py",
    "chaos_test.py",
    "ota_preflight_demo.py",
    "multicore_pipeline.rs",
    "nn_latency.rs",
    "resource_sched_demo",
    "sal_adapter_demo",
    "nobro_kernel/src/eval.rs",
    "nobro_kernel/src/fault_inject.rs",
    "board_fixtures.rs",
)
FORBIDDEN_PATTERNS = {
    "local serial endpoint": re.compile(r"\bCOM[0-9]+\b", re.IGNORECASE),
    "local board alias": re.compile(r"\bboard[1-9][0-9]*\b", re.IGNORECASE),
    "local environment name": re.compile(r"\bIronEngine(?:World)?\b", re.IGNORECASE),
    "internal wave tag": re.compile(r"\bWave\s+[0-9]+\b", re.IGNORECASE),
    "internal milestone tag": re.compile(r"\(M[0-9]+(?:[/,-][^)]*)?\)"),
    "Windows machine path": re.compile(r"\b[A-Za-z]:\\"),
    "internal document": re.compile(r"(?:_INTERNAL\.md|REMODELING_PLAN_" r"INTERNAL)"),
    "private hardware report": re.compile(
        r"(?:nobro-hil-fleet-v1|state-restor|physical_hil|pre-test flash)",
        re.IGNORECASE,
    ),
    "private host-contract section": re.compile(
        r'"(?:lab|phase1_eval|phase2_eval|build_budgets|ironengine)"\s*:',
        re.IGNORECASE,
    ),
}


def overlaps(left: pathlib.PurePosixPath, right: pathlib.PurePosixPath) -> bool:
    return left == right or left in right.parents or right in left.parents


def validate() -> list[str]:
    errors: list[str] = []
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    public_tools = set(manifest.get("host_tools", []))
    excludes = {pathlib.PurePosixPath(item) for item in manifest.get("release_excludes", [])}
    includes = {pathlib.PurePosixPath(item) for item in manifest.get("core_distribution_roots", [])}
    required_excludes = {
        pathlib.PurePosixPath("_work"),
        pathlib.PurePosixPath("_maintainer"),
        pathlib.PurePosixPath("core/baselines"),
        pathlib.PurePosixPath("core/fuzz"),
        pathlib.PurePosixPath("core/internal"),
        pathlib.PurePosixPath("tools/internal"),
        pathlib.PurePosixPath("tools/dev"),
    }
    if not required_excludes <= excludes:
        errors.append(f"missing release excludes: {sorted(map(str, required_excludes-excludes))}")
    forbidden_tracked = (
        "_maintainer/", "core/baselines/", "core/fuzz/", "core/internal/",
        "core/ecosystem/", "core/ecosystems/",
        "tools/internal/", "tools/dev/", "docs/ENGINEERING.md",
        "tools/nobro_hw_eval.py", "tools/hil_matrix.py", "tools/wasm_slot_spike.py",
        "core/apps/kernel/kernel_wcet_demo/", "core/apps/kernel/kernel_selftest/",
        "core/apps/kernel/hil_fault_demo/",
        "core/apps/kernel/resource_sched_demo/",
        "core/apps/interop/sal_adapter_demo/",
        "core/ports/esp32s3/src/bin/multicore_pipeline.rs",
        "core/ports/esp32s3/src/bin/nn_latency.rs",
        "core/crates/nobro_kernel/src/eval.rs",
        "core/crates/nobro_kernel/src/fault_inject.rs",
        "tools/nobro_verify.py", "tools/fleet_evidence.py", "tools/chaos_test.py",
        "tools/ota_preflight_demo.py", "tools/check_ecosystem_matrix.py",
        "tools/check_camera_ecosystem.py", "tools/check_wireless_ecosystem.py",
    )
    tracked = subprocess.run(
        ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout.splitlines()
    leaked = [
        path for path in tracked
        if path.startswith(forbidden_tracked) and (ROOT / path).exists()
    ]
    if leaked:
        errors.append(f"maintainer-only files are tracked: {leaked[:5]}")
    for public in map(pathlib.PurePosixPath, public_tools):
        if any(overlaps(public, excluded) for excluded in excludes):
            errors.append(f"public tool overlaps excluded path: {public}")
        if not (ROOT / public).is_file():
            errors.append(f"public tool is missing: {public}")
    for included in includes:
        if any(overlaps(included, excluded) for excluded in excludes):
            errors.append(f"core distribution root overlaps excluded path: {included}")
        if not (ROOT / included).exists():
            errors.append(f"core distribution root is missing: {included}")
    if list((ROOT / "tools").glob("measure_*.py")):
        errors.append("comparison tools must not live on the public tools root")
    workflow = (ROOT / ".github" / "workflows" / "gates.yml").read_text(encoding="utf-8")
    for name in ("measure_authoring.py", "measure_baselines.py", "measure_complex_runtime.py", "measure_embassy_variants.py"):
        if f"tools/{name}" in workflow:
            errors.append(f"hosted workflow uses stale public comparison path: tools/{name}")
    hosted_jobs = re.findall(
        r"(?ms)^  [A-Za-z0-9_-]+:\s*\n.*?(?=^  [A-Za-z0-9_-]+:\s*$|\Z)", workflow
    )
    linux_jobs = [job for job in hosted_jobs if re.search(r"(?m)^    runs-on: ubuntu", job)]
    if any("arduino-cli core install arduinonrf:nrf52" in job for job in linux_jobs):
        errors.append("hosted Linux workflow cannot install the Windows-only ArduinoNRF toolchain")
    for relative in tracked:
        path = ROOT / relative
        if relative in {
            ".gitignore", "sdk/sdk-manifest.json", "tools/check_release_boundary.py"
        } or not path.is_file():
            continue
        if path.suffix.lower() in {".png", ".jpg", ".uf2", ".zip"}:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for token in FORBIDDEN_REFERENCES:
            if token in text:
                errors.append(f"{relative} exposes maintainer-only token {token}")
        for label, pattern in FORBIDDEN_PATTERNS.items():
            match = pattern.search(text)
            if match:
                line = text.count("\n", 0, match.start()) + 1
                errors.append(f"{relative}:{line} exposes {label}: {match.group(0)!r}")
    return errors


def main() -> int:
    errors = validate()
    for error in errors:
        print(f"RELEASE BOUNDARY: {error}")
    print(f"RELEASE BOUNDARY: {'PASS' if not errors else 'FAIL'}")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
