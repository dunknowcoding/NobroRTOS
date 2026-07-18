#!/usr/bin/env python3
"""Build and inspect the public L0 kernel specimens.

This is a product regression gate, not a comparison harness: it verifies that
generated firmware contains executable/vector sections, retains its admitted
read-only table, stays within the documented L0 ceilings, and does not link any
subsystem reported as absent.
"""
import pathlib
import re
import copy
import json
import os
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
import nobro_firmware_project as firmware  # noqa: E402
import static_budget  # noqa: E402

TARGET = "thumbv7em-none-eabihf"
FORBIDDEN = {
    "admission-runtime": ("nobro_kernel::admission", "nobro_admission::admit"),
    "recovery": ("nobro_kernel::recovery",),
    "report": ("nobro_kernel::report",),
    "trace": ("CapabilityTrace", "nobro_kernel::event_log"),
    "quota": ("nobro_kernel::quota",),
    "health": ("nobro_kernel::health",),
    "stack-guard": ("nobro_kernel::stack_guard",),
    "mpu": ("nobro_hal::mpu", "nobro_kernel::mpu"),
    "async-rt": ("nobro_kernel::async_",),
    "classic-compat": ("nobro_classic",),
    "formatting": ("core::fmt::",),
}

SIMPLE = """app nano_min
board nrf52840-nosd
control control every 5ms budget 400us blocking 50us
sensor sensor every 10ms -> control budget 700us
service telemetry every 40ms
"""

COMPLEX = """app nano_complex
board nrf52840-nosd
sensor acquire every 5ms -> fusion budget 200us
control fusion every 10ms -> control budget 400us
control control every 20ms -> radio budget 500us
sensor radio every 50ms -> logger budget 300us
service logger every 100ms
"""

SCALE_10 = """app nano_scale_10
board nrf52840-nosd
control t0 every 5ms phase 0us budget 120us
sensor t1 every 10ms phase 250us -> t0 budget 180us
sensor t2 every 20ms phase 500us -> t0 budget 220us
service t3 every 40ms phase 750us -> t4 budget 260us
control t4 every 5ms phase 1000us budget 120us
sensor t5 every 10ms phase 1250us -> t0 budget 180us
sensor t6 every 20ms phase 1500us -> t0 budget 220us
service t7 every 40ms phase 1750us -> t4 budget 260us
control t8 every 5ms phase 2000us budget 120us
sensor t9 every 10ms phase 2250us -> t0 budget 180us
"""

SCALE_16 = """app nano_scale_16
board nrf52840-nosd
control t0 every 5ms phase 0us budget 120us
sensor t1 every 10ms phase 250us -> t0 budget 180us
sensor t2 every 20ms phase 500us -> t0 budget 220us
service t3 every 40ms phase 750us -> t4 budget 260us
control t4 every 5ms phase 1000us budget 120us
sensor t5 every 10ms phase 1250us -> t0 budget 180us
sensor t6 every 20ms phase 1500us -> t0 budget 220us
service t7 every 40ms phase 1750us -> t4 budget 260us
control t8 every 5ms phase 2000us budget 120us
sensor t9 every 10ms phase 2250us -> t0 budget 180us
sensor t10 every 20ms phase 2500us -> t0 budget 220us
service t11 every 40ms phase 2750us -> t4 budget 260us
control t12 every 5ms phase 3000us budget 120us
sensor t13 every 10ms phase 3250us -> t0 budget 180us
sensor t14 every 20ms phase 3500us -> t0 budget 220us
service t15 every 40ms phase 3750us -> t4 budget 260us
"""

UNSCHEDULABLE = """app nano_reject
board nrf52840-nosd
control fast every 2ms budget 800us
control slow every 3ms budget 1500us
"""


def llvm_tool(name: str) -> pathlib.Path:
    sysroot = pathlib.Path(subprocess.check_output(
        ["rustc", "--print", "sysroot"], text=True).strip())
    host = next(line.split(":", 1)[1].strip()
                for line in subprocess.check_output(
                    ["rustc", "-vV"], text=True).splitlines()
                if line.startswith("host:"))
    suffix = ".exe" if sys.platform == "win32" else ""
    path = sysroot / "lib" / "rustlib" / host / "bin" / f"llvm-{name}{suffix}"
    if not path.is_file():
        raise RuntimeError(f"llvm-{name} unavailable; install llvm-tools-preview")
    return path


def command(args: list[str | pathlib.Path]) -> str:
    return subprocess.check_output([str(arg) for arg in args], text=True,
                                   encoding="utf-8", errors="replace")


def build_case(root: pathlib.Path, source_text: str | dict, ceiling: int,
               ram_ceiling: int, stack_ceiling: int, total_ram_ceiling: int,
               table_ceiling: int | None = None,
               allowed_forbidden: frozenset[str] = frozenset(),
               required_symbols: tuple[str, ...] = ()) -> dict:
    source = root / ("workload.json" if isinstance(source_text, dict) else "app.nobro")
    source.parent.mkdir(parents=True, exist_ok=True)
    source.write_text(
        json.dumps(source_text, indent=2) + "\n"
        if isinstance(source_text, dict) else source_text,
        encoding="utf-8",
    )
    generated = firmware.generate(source, root / "projects")
    built = firmware.build(pathlib.Path(generated["project"]))
    if built.returncode:
        raise AssertionError((built.stdout + built.stderr)[-4000:])
    app = generated["app"].replace("_", "-")
    target_root = pathlib.Path(os.environ.get(
        "CARGO_TARGET_DIR", pathlib.Path(generated["project"]) / "target"))
    if not target_root.is_absolute():
        target_root = pathlib.Path(generated["project"]) / target_root
    elf = target_root / TARGET / "release" / f"nobro-app-{app}"
    objdump = str(llvm_tool("objdump"))
    size = command([llvm_tool("size"), elf]).strip().splitlines()[-1].split()
    flash = int(size[0]) + int(size[1])
    static_ram = int(size[1]) + int(size[2])
    if flash > ceiling:
        raise AssertionError(f"{app}: flash {flash} exceeds {ceiling}")
    (
        _frames,
        _cycles,
        stack_worst,
        stack_path,
        _cycle_worst,
        _cycle_path,
        indirect,
        _loops,
        recursive,
        _unknown_cycles,
    ) = static_budget.analyze(str(elf), objdump)
    total_ram = static_ram + stack_worst
    if static_ram > ram_ceiling:
        raise AssertionError(f"{app}: static RAM {static_ram} exceeds {ram_ceiling}")
    if stack_worst > stack_ceiling:
        raise AssertionError(
            f"{app}: computed stack {stack_worst} exceeds {stack_ceiling}; "
            f"path {static_budget.format_path(stack_path)}")
    if total_ram > total_ram_ceiling:
        raise AssertionError(f"{app}: total RAM {total_ram} exceeds {total_ram_ceiling}")
    if indirect:
        raise AssertionError(f"{app}: static budget has unpriced indirect calls: {indirect[:4]}")
    if recursive:
        raise AssertionError(f"{app}: static budget found recursion: {recursive[:4]}")
    sections = command([llvm_tool("objdump"), "-h", elf])
    for required in (".vector_table", ".text", ".rodata"):
        if required not in sections:
            raise AssertionError(f"{app}: missing executable section {required}")
    symbols = command([llvm_tool("nm"), "-S", "--defined-only", "--demangle", elf])
    match = re.search(r"^\S+\s+(\S+)\s+\S\s+NOBRO_ADMITTED_WORKLOAD$",
                      symbols, re.MULTILINE)
    if not match or int(match.group(1), 16) == 0:
        raise AssertionError(f"{app}: admitted .rodata table was discarded")
    table = int(match.group(1), 16)
    if table_ceiling is not None and table > table_ceiling:
        raise AssertionError(f"{app}: admitted table {table} exceeds {table_ceiling}")
    violations = [f"{feature}:{token}" for feature, tokens in FORBIDDEN.items()
                  if feature not in allowed_forbidden
                  for token in tokens if token in symbols]
    if violations:
        raise AssertionError(f"{app}: forbidden linked symbols: {', '.join(violations)}")
    missing = [symbol for symbol in required_symbols if symbol not in symbols]
    if missing:
        raise AssertionError(f"{app}: required linked symbols missing: {', '.join(missing)}")
    return {"app": app, "flash": flash, "static_ram": static_ram,
            "stack": stack_worst, "total_ram": total_ram,
            "table": table, "symbols": symbols}


def main() -> int:
    try:
        with tempfile.TemporaryDirectory(prefix="nobro-nano-") as tmp:
            root = pathlib.Path(tmp)
            simple = build_case(root / "simple", SIMPLE, 3_000, 64, 128, 192)
            complex_case = build_case(root / "complex", COMPLEX, 3_400, 96, 128, 224)
            scale_10 = build_case(
                root / "scale10", SCALE_10, 3_300, 96, 128, 224, table_ceiling=360)
            scale_16 = build_case(
                root / "scale16", SCALE_16, 3_600, 96, 128, 224, table_ceiling=560)

            feature_off_workload = firmware.parse(SIMPLE)["workload"]
            feature_off_workload["app"] = "feature_off"
            feature_on_workload = copy.deepcopy(feature_off_workload)
            feature_on_workload["app"] = "feature_on"
            feature_on_workload["features"] = {"capacity-report": True}
            feature_off = build_case(
                root / "feature-off", feature_off_workload, 3_000, 64, 128, 192
            )
            feature_on = build_case(
                root / "feature-on",
                feature_on_workload,
                5_000,
                256,
                256,
                512,
                allowed_forbidden=frozenset({"report"}),
                required_symbols=("NOBRO_FEATURE_CAPACITY_REPORT",),
            )
            if "NOBRO_FEATURE_CAPACITY_REPORT" in feature_off["symbols"]:
                raise AssertionError("feature-off image retained the capacity marker")
            deltas = {
                "flash_delta_bytes_max": feature_on["flash"] - feature_off["flash"],
                "static_ram_delta_bytes_max": (
                    feature_on["static_ram"] - feature_off["static_ram"]
                ),
                "total_ram_delta_bytes_max": (
                    feature_on["total_ram"] - feature_off["total_ram"]
                ),
            }
            if deltas["flash_delta_bytes_max"] <= 0:
                raise AssertionError("capacity-report did not change linked flash")
            catalog = firmware.project_model.feature_catalog()
            entry = catalog["targets"]["nrf52840-nosd"]["capacity-report"]
            price = entry["price"]
            for field, delta in deltas.items():
                if delta < 0 or delta > int(price[field]):
                    raise AssertionError(
                        f"capacity-report {field} delta {delta} exceeds catalog {price[field]}"
                    )
            measured = entry["evidence"]["measured"]
            actual = {
                "off": {
                    "flash_bytes": feature_off["flash"],
                    "static_ram_bytes": feature_off["static_ram"],
                    "total_ram_bytes": feature_off["total_ram"],
                },
                "on": {
                    "flash_bytes": feature_on["flash"],
                    "static_ram_bytes": feature_on["static_ram"],
                    "total_ram_bytes": feature_on["total_ram"],
                },
                "delta": {
                    "flash_bytes": deltas["flash_delta_bytes_max"],
                    "static_ram_bytes": deltas["static_ram_delta_bytes_max"],
                    "total_ram_bytes": deltas["total_ram_delta_bytes_max"],
                },
            }
            if measured != actual:
                raise AssertionError(
                    f"capacity-report linked evidence drift: catalog={measured} actual={actual}"
                )

            reject_source = root / "reject" / "app.nobro"
            reject_source.parent.mkdir(parents=True)
            reject_source.write_text(UNSCHEDULABLE, encoding="utf-8")
            rejected = firmware.generate(reject_source, root / "reject" / "projects")
            result = firmware.build(pathlib.Path(rejected["project"]))
            detail = result.stdout + result.stderr
            if result.returncode == 0 or "NOBRO-E009" not in detail or "task `slow`" not in detail:
                raise AssertionError("unschedulable build did not fail with its task label")
    except (OSError, RuntimeError, subprocess.SubprocessError, AssertionError) as error:
        print(f"NANO KERNEL: FAIL ({error})")
        return 1
    print("NANO KERNEL: PASS "
          f"(min flash={simple['flash']} ram={simple['static_ram']} "
          f"stack={simple['stack']} table={simple['table']}; "
          f"complex flash={complex_case['flash']} ram={complex_case['static_ram']} "
          f"stack={complex_case['stack']} table={complex_case['table']}; "
          f"scale10 flash={scale_10['flash']} ram={scale_10['static_ram']} "
          f"stack={scale_10['stack']} table={scale_10['table']}; "
          f"scale16 flash={scale_16['flash']} ram={scale_16['static_ram']} "
          f"stack={scale_16['stack']} table={scale_16['table']}; "
          f"capacity-report off/on flash={feature_off['flash']}/{feature_on['flash']} "
          f"static={feature_off['static_ram']}/{feature_on['static_ram']} "
          f"total={feature_off['total_ram']}/{feature_on['total_ram']}; "
          "feature marker off/on verified; build rejection attributed)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
