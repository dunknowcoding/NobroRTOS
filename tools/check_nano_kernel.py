#!/usr/bin/env python3
"""Build and inspect the public L0 kernel specimens.

This is a product regression gate, not a comparison harness: it verifies that
generated firmware contains executable/vector sections, retains its admitted
read-only table, stays within the documented L0 ceilings, and does not link any
subsystem reported as absent.
"""
import pathlib
import re
import os
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
import nobro_firmware_project as firmware  # noqa: E402

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


def build_case(root: pathlib.Path, source_text: str, ceiling: int) -> dict:
    source = root / "app.nobro"
    source.parent.mkdir(parents=True, exist_ok=True)
    source.write_text(source_text, encoding="utf-8")
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
    size = command([llvm_tool("size"), elf]).strip().splitlines()[-1].split()
    flash = int(size[0]) + int(size[1])
    static_ram = int(size[1]) + int(size[2])
    if flash > ceiling:
        raise AssertionError(f"{app}: flash {flash} exceeds {ceiling}")
    sections = command([llvm_tool("objdump"), "-h", elf])
    for required in (".vector_table", ".text", ".rodata"):
        if required not in sections:
            raise AssertionError(f"{app}: missing executable section {required}")
    symbols = command([llvm_tool("nm"), "-S", "--defined-only", "--demangle", elf])
    match = re.search(r"^\S+\s+(\S+)\s+\S\s+NOBRO_ADMITTED_WORKLOAD$",
                      symbols, re.MULTILINE)
    if not match or int(match.group(1), 16) == 0:
        raise AssertionError(f"{app}: admitted .rodata table was discarded")
    violations = [f"{feature}:{token}" for feature, tokens in FORBIDDEN.items()
                  for token in tokens if token in symbols]
    if violations:
        raise AssertionError(f"{app}: forbidden linked symbols: {', '.join(violations)}")
    return {"app": app, "flash": flash, "static_ram": static_ram,
            "table": int(match.group(1), 16)}


def main() -> int:
    try:
        with tempfile.TemporaryDirectory(prefix="nobro-nano-") as tmp:
            root = pathlib.Path(tmp)
            simple = build_case(root / "simple", SIMPLE, 3_000)
            complex_case = build_case(root / "complex", COMPLEX, 3_400)

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
          f"(min flash={simple['flash']} table={simple['table']}; "
          f"complex flash={complex_case['flash']} table={complex_case['table']}; "
          "build rejection attributed; feature symbols absent)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
