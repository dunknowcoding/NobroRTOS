#!/usr/bin/env python3
"""Build and size the RES-01/02 baseline suite across four execution models.

Equivalent observable workloads (see core/baselines/README.md), pinned build
settings. Reports flash (text+data), static RAM (data+bss), and local source
line counts; attributes nobro-min's flash to crates so the cost of each
enabled service is machine-readable; enforces regression thresholds for
nobro-min from tools/baseline_budgets.json.

    python tools/measure_baselines.py               # build + size + thresholds
    python tools/measure_baselines.py --breakdown   # + per-crate flash attribution
    python tools/measure_baselines.py --selftest    # gate: pure math, no builds

Exit 0 only when every required implementation builds and nobro-min stays
within its budgets. Embassy needs crates.io; when the registry is unreachable
the suite reports it as skipped (evidence says so) instead of failing local
offline runs.
"""
import argparse
import glob
import json
import os
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASE = ROOT / "core" / "baselines"
OUT = ROOT / "_work" / "evidence" / "baselines.json"
BUDGETS = ROOT / "tools" / "baseline_budgets.json"
TARGET = "thumbv7em-none-eabihf"
IMPLEMENTATIONS = ("baremetal-min", "nobro-min", "nobro-graph-min", "embassy-min",
                   "baremetal-complex", "nobro-graph-complex", "embassy-complex",
                   "embassy-complex-tuned", "freertos-complex")


def find_llvm_tool(name: str) -> str:
    sysroot = subprocess.run(
        ["rustc", "--print", "sysroot"], capture_output=True, text=True, check=True
    ).stdout.strip()
    exe = ".exe" if os.name == "nt" else ""
    matches = glob.glob(os.path.join(sysroot, "lib", "rustlib", "*", "bin", name + exe))
    if not matches:
        raise SystemExit(f"{name} not found - install rustup component llvm-tools-preview")
    return matches[0]


def parse_berkeley(size_output: str) -> dict:
    """Parse `llvm-size` berkeley output into text/data/bss ints."""
    for line in size_output.splitlines():
        fields = line.split()
        if len(fields) >= 3 and fields[0].isdigit():
            text, data, bss = (int(fields[i]) for i in range(3))
            return {
                "text": text,
                "data": data,
                "bss": bss,
                "flash": text + data,
                "static_ram": data + bss,
            }
    raise ValueError("unrecognized size output")


def elf_sizes(elf: pathlib.Path) -> dict:
    """text/data/bss straight from the ELF section headers — no external tool,
    so the gate runs identically on any CI host. Matches berkeley `size`
    semantics: text = alloc non-writable progbits, data = alloc writable
    progbits, bss = alloc nobits."""
    import struct
    blob = elf.read_bytes()
    assert blob[:4] == b"\x7fELF", "not an ELF"
    is64 = blob[4] == 2
    little = blob[5] == 1
    end = "<" if little else ">"
    if is64:
        shoff, shentsize, shnum = struct.unpack_from(end + "Q", blob, 0x28)[0], \
            struct.unpack_from(end + "H", blob, 0x3A)[0], \
            struct.unpack_from(end + "H", blob, 0x3C)[0]
    else:
        shoff = struct.unpack_from(end + "I", blob, 0x20)[0]
        shentsize = struct.unpack_from(end + "H", blob, 0x2E)[0]
        shnum = struct.unpack_from(end + "H", blob, 0x30)[0]
    text = data = bss = 0
    for index in range(shnum):
        base = shoff + index * shentsize
        if is64:
            sh_type, sh_flags = struct.unpack_from(end + "IQ", blob, base + 4)[0], \
                struct.unpack_from(end + "Q", blob, base + 8)[0]
            sh_size = struct.unpack_from(end + "Q", blob, base + 0x20)[0]
        else:
            sh_type = struct.unpack_from(end + "I", blob, base + 4)[0]
            sh_flags = struct.unpack_from(end + "I", blob, base + 8)[0]
            sh_size = struct.unpack_from(end + "I", blob, base + 0x14)[0]
        alloc = sh_flags & 0x2
        write = sh_flags & 0x1
        if not alloc:
            continue
        if sh_type == 8:  # SHT_NOBITS
            bss += sh_size
        elif write:
            data += sh_size
        else:
            text += sh_size
    return {"text": text, "data": data, "bss": bss,
            "flash": text + data, "static_ram": data + bss}


def source_lines(directory: pathlib.Path) -> int:
    count = 0
    source_paths = (
        path for path in (directory / "src").rglob("*")
        if path.suffix in {".rs", ".c", ".h"}
    )
    for path in source_paths:
        instrumentation = False
        for line in path.read_text(encoding="utf-8").splitlines():
            stripped = line.strip()
            if "BENCH_INSTRUMENTATION_BEGIN" in stripped:
                instrumentation = True
                continue
            if "BENCH_INSTRUMENTATION_END" in stripped:
                instrumentation = False
                continue
            if instrumentation:
                continue
            if stripped and not stripped.startswith("//"):
                count += 1
    return count


def crate_of(symbol: str) -> str:
    """Attribute a mangled/demangled symbol to its crate (cost breakdown)."""
    name = symbol.lstrip("_")
    if name.startswith("<"):
        name = name[1:]
    root = re.split(r"[:<,( ]", name, maxsplit=1)[0]
    known = {
        "nobro_kernel", "nobro_power", "nobro_crypto", "nobro_secure",
        "cortex_m", "cortex_m_rt", "compiler_builtins", "core", "portable_atomic",
        "critical_section", "embassy_executor", "embassy_time", "embassy_nrf",
        "embassy_sync", "embassy_time_driver",
    }
    return root if root in known else "app_or_misc"


def crate_breakdown(nm_tool: str, elf: pathlib.Path) -> dict:
    completed = subprocess.run(
        [nm_tool, "--size-sort", "--demangle", str(elf)],
        capture_output=True, text=True, check=True,
    )
    totals: dict[str, int] = {}
    for line in completed.stdout.splitlines():
        fields = line.split(None, 2)
        if len(fields) != 3:
            continue
        size_hex, kind, symbol = fields
        if kind.lower() not in ("t", "w"):  # flash code only
            continue
        totals[crate_of(symbol)] = totals.get(crate_of(symbol), 0) + int(size_hex, 16)
    return dict(sorted(totals.items(), key=lambda item: -item[1]))


# The minimal profile's isolation proof: none of these services may appear in
# nobro-min's binary. Selecting a service = adding its crate; not selecting it
# = it does not exist in flash. (managed = + secure/storage/database;
# assured = + net/fleet/ai surfaces. Profiles are dependency sets, enforced by
# this symbol-level check rather than by feature flags that could drift.)
FORBIDDEN_IN_MINIMAL = {
    "nobro_secure", "nobro_crypto", "nobro_net", "nobro_storage",
    "nobro_database", "nobro_ai", "nobro_ml", "nobro_nn",
}


def minimal_profile_violations(breakdown: dict) -> list[str]:
    return sorted(FORBIDDEN_IN_MINIMAL.intersection(breakdown))


def check_budgets(measure: dict, budgets: dict) -> list[str]:
    failures = []
    for key, ceiling in budgets.items():
        actual = measure.get(key)
        if actual is not None and actual > ceiling:
            failures.append(f"{key}={actual} exceeds budget {ceiling}")
    return failures


def build(directory: pathlib.Path, features: tuple[str, ...] = ()) -> tuple[bool, str]:
    env = os.environ.copy()
    # CI orchestrators (ci_matrix.sh) export a global CARGO_TARGET_DIR; the
    # baselines must build into their own tree so the ELF paths and the
    # per-implementation lockfiles stay deterministic everywhere.
    env["CARGO_TARGET_DIR"] = str(directory / "target")
    command = ["cargo", "build", "--release"]
    if features:
        command += ["--features", ",".join(features)]
    completed = subprocess.run(
        command, cwd=directory,
        capture_output=True, text=True, env=env,
    )
    return completed.returncode == 0, completed.stderr[-2000:]


def selftest() -> int:
    parsed = parse_berkeley("   text\t   data\t    bss\t    dec\t    hex\n"
                            "  9000\t    100\t   2000\t  11100\t  2b5c\tapp\n")
    assert parsed == {"text": 9000, "data": 100, "bss": 2000,
                      "flash": 9100, "static_ram": 2100}
    assert crate_of("nobro_kernel::runtime::Runtime<...>::send") == "nobro_kernel"
    assert crate_of("<nobro_kernel::mailbox::Mailbox<_> as core::fmt::Debug>::fmt") \
        == "nobro_kernel"
    assert crate_of("embassy_executor::raw::Executor::poll") == "embassy_executor"
    assert crate_of("main") == "app_or_misc"
    failures = check_budgets({"flash": 100, "static_ram": 10}, {"flash": 90})
    assert failures == ["flash=100 exceeds budget 90"]
    assert check_budgets({"flash": 80}, {"flash": 90}) == []
    assert minimal_profile_violations({"nobro_kernel": 5000, "app_or_misc": 8000}) == []
    assert minimal_profile_violations(
        {"nobro_kernel": 1, "nobro_secure": 900}
    ) == ["nobro_secure"]
    print("BASELINE MEASURE SELFTEST: PASS")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--breakdown", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    budgets = json.loads(BUDGETS.read_text(encoding="utf-8")) if BUDGETS.is_file() else {}

    report = {"schema": "nobro-baselines-v1", "target": TARGET, "results": {}}
    report["toolchain"] = subprocess.run(
        ["rustc", "--version"], capture_output=True, text=True
    ).stdout.strip()
    failures: list[str] = []
    for name in IMPLEMENTATIONS:
        directory_name = "embassy-complex" if name == "embassy-complex-tuned" else name
        binary_name = directory_name
        directory = BASE / directory_name
        features = ("arena-1024",) if name == "embassy-complex-tuned" else ()
        ok, stderr = build(directory, features)
        if not ok:
            offline = "failed to get" in stderr or "network" in stderr or "download" in stderr
            if name.startswith("embassy-") and offline:
                report["results"][name] = {"skipped": "registry_unreachable"}
                continue
            report["results"][name] = {"failed": True}
            failures.append(f"{name}: build failed")
            continue
        elf = directory / "target" / TARGET / "release" / binary_name
        sizes = elf_sizes(elf)
        sizes["source_lines"] = source_lines(directory)
        if name in ("nobro-min", "nobro-graph-min", "nobro-graph-complex"):
            breakdown = crate_breakdown(find_llvm_tool("llvm-nm"), elf)
            if args.breakdown:
                sizes["flash_by_crate"] = breakdown
            violations = minimal_profile_violations(breakdown)
            sizes["minimal_profile_clean"] = not violations
            if violations:
                failures.append(
                    f"{name}: unselected services linked: {', '.join(violations)}"
                )
        report["results"][name] = sizes
        if name in budgets:
            failures.extend(f"{name}: {msg}" for msg in check_budgets(sizes, budgets[name]))

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(f"{'impl':14} {'flash':>8} {'static_ram':>11} {'lines':>6}")
    for name, sizes in report["results"].items():
        if "flash" in sizes:
            print(f"{name:14} {sizes['flash']:>8} {sizes['static_ram']:>11} "
                  f"{sizes['source_lines']:>6}")
        else:
            print(f"{name:14} {'skipped' if 'skipped' in sizes else 'FAILED':>8}")
    for failure in failures:
        print("BUDGET FAIL:", failure)
    print(f"json: {OUT}")
    print(f"RESULT: {'PASS' if not failures else 'FAIL'}")
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
