#!/usr/bin/env python3
"""Generate host-readable contracts and production firmware from one small app file.

The input is intentionally a declaration, not generated Rust boilerplate::

    app rover
    board nrf52840-s140
    control motor every 5ms
    sensor imu every 10ms -> motor
    service camera every 40ms

Safe budgets and memory estimates are inferred by role.  Advanced users can still
edit the emitted workload.json before admission; the original declaration remains
the auditable source used to regenerate firmware.
"""
import argparse
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_OUT = ROOT / "_work" / "projects"
NAME = re.compile(r"^[a-z][a-z0-9_-]{0,47}$")
LINE = re.compile(
    r"^(control|sensor|service)\s+([a-z][a-z0-9_-]{0,47})\s+"
    r"every\s+([1-9][0-9]*)(us|ms|s)(?:\s*->\s*([a-z][a-z0-9_-]{0,47}))?"
    r"(?:\s+budget\s+([1-9][0-9]*)(us|ms|s))?"
    r"(?:\s+blocking\s+([1-9][0-9]*)(us|ms|s))?"
    r"(?:\s+memory\s+([1-9][0-9]*)/([1-9][0-9]*))?$"
)
BOARDS = {
    "nrf52840-s140": ("s140", 128 * 1024, 32 * 1024),
    "nrf52840-nosd": ("nosd", 128 * 1024, 32 * 1024),
}
ROLE = {
    "control": ("hard_realtime", 2048, 512, 5),
    "sensor": ("driver", 1024, 256, 10),
    "service": ("best_effort", 1024, 256, 20),
}


def parse_duration(value: str, unit: str) -> int:
    scale = {"us": 1, "ms": 1000, "s": 1_000_000}[unit]
    result = int(value) * scale
    if result > 0xFFFF_FFFF:
        raise ValueError("period exceeds the firmware's u32 microsecond range")
    return result


def parse(text: str) -> dict:
    records = []
    for number, raw in enumerate(text.splitlines(), 1):
        line = raw.split("#", 1)[0].strip()
        if line:
            records.append((number, line))
    if len(records) < 3 or not records[0][1].startswith("app "):
        raise ValueError("line 1 must be: app <name>")
    app = records[0][1][4:].strip()
    if not NAME.fullmatch(app):
        raise ValueError("app name must match [a-z][a-z0-9_-]{0,47}")
    if not records[1][1].startswith("board "):
        raise ValueError("line 2 must be: board <profile>")
    board = records[1][1][6:].strip()
    if board not in BOARDS:
        raise ValueError(f"unsupported board profile {board!r}; choose {', '.join(BOARDS)}")
    tasks = []
    channels = []
    for number, line in records[2:]:
        match = LINE.fullmatch(line)
        if not match:
            raise ValueError(f"line {number}: expected '<role> <name> every <duration> [-> <task>]' ")
        (role, name, value, unit, destination, budget_value, budget_unit,
         blocking_value, blocking_unit, flash_override, ram_override) = match.groups()
        criticality, flash, ram, divisor = ROLE[role]
        period = parse_duration(value, unit)
        budget = (parse_duration(budget_value, budget_unit)
                  if budget_value else max(1, period // divisor))
        blocking = (parse_duration(blocking_value, blocking_unit)
                    if blocking_value else 0)
        if budget + blocking > period:
            raise ValueError(f"line {number}: budget + blocking exceeds period")
        tasks.append({"name": name, "role": role, "criticality": criticality,
                      "flash": int(flash_override or flash),
                      "ram": int(ram_override or ram), "period_us": period,
                      "budget_us": budget, "blocking_us": blocking})
        if destination:
            channels.append([name, destination])
    names = [task["name"] for task in tasks]
    if len(set(names)) != len(names):
        raise ValueError("task names must be unique")
    for source, destination in channels:
        if destination not in names:
            raise ValueError(f"{source}: channel destination {destination!r} is not a task")
        if source == destination:
            raise ValueError(f"{source}: a task cannot send to itself")
    _, flash_limit, ram_limit = BOARDS[board]
    workload = {
        "profile": {"flash": flash_limit, "ram": ram_limit, "pool": max(8, len(tasks) + 1)},
        "tasks": [{"name": "kernel", "criticality": "hard_realtime",
                   "flash": 12 * 1024, "ram": 3 * 1024, "pool": 2,
                   "period_us": 20_000, "budget_us": 0}] + tasks,
        "channels": channels,
    }
    return {"app": app, "board": board, "workload": workload,
            "user_lines": len(records)}


def rust_main(spec: dict) -> str:
    tasks = spec["workload"]["tasks"][1:]
    chain = f"AppGraph::<{len(tasks)}>::new()\n"
    constructor = {"control": "control", "sensor": "periodic", "service": "service"}
    for task in tasks:
        chain += (f'        .task(TaskDecl::{constructor[task["role"]]}('
                  f'"{task["name"]}", {task["period_us"]})'
                  f'.budget_us({task["budget_us"]})'
                  f'.blocking_us({task["blocking_us"]})).unwrap()\n')
    for source, destination in spec["workload"]["channels"]:
        chain += f'        .channel("{source}", "{destination}").unwrap()\n'
    chain += f"        .build_for::<{len(tasks) + 1}>(SystemProfile::NRF52840_CORE).unwrap()"
    return f'''//! Generated from app.nobro. Regenerate instead of editing this file.
#![no_std]
#![no_main]
use cortex_m::asm;
use cortex_m_rt::entry;
use panic_halt as _;
use nobro_kernel::{{AppGraph, SystemProfile, TaskDecl}};

#[no_mangle]
#[used]
static mut NOBRO_APP_REPORT: [u32; 4] = [0; 4];

#[entry]
fn main() -> ! {{
    let built = {chain};
    unsafe {{
        core::ptr::write_volatile(core::ptr::addr_of_mut!(NOBRO_APP_REPORT),
            [0x4e42_4150, built.task_len as u32, built.startup_len as u32, 1]);
    }}
    loop {{ asm::wfi(); }}
}}
'''


def generate(source: pathlib.Path, out_dir: pathlib.Path) -> dict:
    text = source.read_text(encoding="utf-8")
    spec = parse(text)
    project = (out_dir / spec["app"]).resolve()
    (project / "src").mkdir(parents=True, exist_ok=True)
    (project / "app.nobro").write_text(text, encoding="utf-8", newline="\n")
    (project / "workload.json").write_text(
        json.dumps(spec["workload"], indent=2) + "\n", encoding="utf-8", newline="\n")
    kernel = ROOT / "core" / "crates" / "nobro_kernel"
    try:
        kernel_path = os.path.relpath(kernel, project).replace("\\", "/")
    except ValueError:
        kernel_path = str(kernel).replace("\\", "/")
    cargo = f'''[package]
name = "nobro-app-{spec['app'].replace('_', '-')}"
version = "0.1.0"
edition = "2021"
publish = false
build = "build.rs"

[workspace]

[dependencies]
nobro-kernel = {{ path = {json.dumps(kernel_path)} }}
cortex-m = {{ version = "0.7", features = ["critical-section-single-core"] }}
cortex-m-rt = "0.7"
panic-halt = "0.2"

[profile.release]
opt-level = "z"
lto = "fat"
codegen-units = 1
'''
    (project / "Cargo.toml").write_text(cargo, encoding="utf-8", newline="\n")
    profile = BOARDS[spec["board"]][0]
    shutil.copyfile(ROOT / "core" / f"memory-{profile}.x", project / "memory.x")
    (project / "build.rs").write_text('''use std::{env, fs, path::PathBuf};
fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).expect("copy memory.x");
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rustc-link-search={}", out.display());
}
''', encoding="utf-8", newline="\n")
    (project / "src" / "main.rs").write_text(rust_main(spec), encoding="utf-8", newline="\n")
    metadata = {"schema": "nobro-firmware-project-v1", "app": spec["app"],
                "board": spec["board"], "memory_profile": profile,
                "user_lines": spec["user_lines"], "generated_rust_lines": len(rust_main(spec).splitlines()),
                "task_count": len(spec["workload"]["tasks"]) - 1}
    (project / "generation.json").write_text(
        json.dumps(metadata, indent=2) + "\n", encoding="utf-8", newline="\n")
    return {"project": project, **metadata}


def build(project: pathlib.Path) -> subprocess.CompletedProcess:
    return subprocess.run(["cargo", "build", "--release", "--target",
                           "thumbv7em-none-eabihf", "--manifest-path",
                           str(project / "Cargo.toml")], cwd=ROOT, text=True,
                          capture_output=True)


def selftest() -> int:
    import tempfile
    sample = """app rover
board nrf52840-s140
control motor every 5ms
sensor imu every 10ms -> motor
service camera every 40ms
"""
    spec = parse(sample)
    assert spec["user_lines"] == 5 and len(spec["workload"]["tasks"]) == 4
    assert spec["workload"]["channels"] == [["imu", "motor"]]
    assert spec["workload"]["tasks"][1]["budget_us"] == 1000
    overridden = parse(sample.replace(
        "control motor every 5ms",
        "control motor every 5ms budget 400us blocking 100us memory 3072/640"))
    assert overridden["workload"]["tasks"][1]["budget_us"] == 400
    assert overridden["workload"]["tasks"][1]["blocking_us"] == 100
    assert overridden["workload"]["tasks"][1]["ram"] == 640
    with tempfile.TemporaryDirectory() as tmp:
        source = pathlib.Path(tmp) / "app.nobro"
        source.write_text(sample, encoding="utf-8")
        result = generate(source, pathlib.Path(tmp) / "out")
        assert result["memory_profile"] == "s140" and result["user_lines"] == 5
        assert (result["project"] / "src" / "main.rs").is_file()
        generated = (result["project"] / "src" / "main.rs").read_text()
        assert "AppGraph::<3>" in generated and ".budget_us(1000).blocking_us(0)" in generated
    for invalid in (sample.replace("motor every", "motor motor every"),
                    sample.replace("-> motor", "-> missing"),
                    sample.replace("nrf52840-s140", "unknown")):
        try:
            parse(invalid)
            raise AssertionError("invalid declaration accepted")
        except ValueError:
            pass
    print("NOBRO FIRMWARE PROJECT SELFTEST: PASS (parse/generate/profiles/validation)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("source", nargs="?", type=pathlib.Path)
    parser.add_argument("--out", type=pathlib.Path, default=DEFAULT_OUT)
    parser.add_argument("--build", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if not args.source:
        parser.error("source is required")
    try:
        result = generate(args.source, args.out)
    except (OSError, ValueError) as error:
        print(f"FIRMWARE PROJECT: FAIL ({error})")
        return 1
    print(f"FIRMWARE PROJECT: generated {result['project']} from {result['user_lines']} user lines")
    if args.build:
        completed = build(result["project"])
        print(f"FIRMWARE BUILD: {'PASS' if completed.returncode == 0 else 'FAIL'}")
        if completed.returncode:
            print("\n".join((completed.stdout + completed.stderr).splitlines()[-12:]))
        return completed.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
