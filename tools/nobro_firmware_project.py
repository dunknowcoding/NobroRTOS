#!/usr/bin/env python3
"""Generate host-readable contracts and production firmware from one small app file.

The input is either the compact declaration below or strict JSON exported by
``nobro_rtos.NobroApp``. It is configuration, not generated Rust boilerplate::

    app rover
    board nrf52840-s140
    wake 25us
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
sys.path.insert(0, str(ROOT / "bindings" / "python"))

from nobro_rtos.app import NobroApp  # noqa: E402

DEFAULT_OUT = ROOT / "_work" / "projects"
NAME = re.compile(r"^[a-z][a-z0-9_-]{0,47}$")
LINE = re.compile(
    r"^(control|sensor|service)\s+([a-z][a-z0-9_-]{0,47})\s+"
    r"every\s+([1-9][0-9]*)(us|ms|s)"
    r"(?:\s+phase\s+([0-9]+)(us|ms|s))?"
    r"(?:\s+deadline\s+([1-9][0-9]*)(us|ms|s))?"
    r"(?:\s*->\s*([a-z][a-z0-9_-]{0,47}))?"
    r"(?:\s+budget\s+([1-9][0-9]*)(us|ms|s))?"
    r"(?:\s+blocking\s+([1-9][0-9]*)(us|ms|s))?"
    r"(?:\s+memory\s+([1-9][0-9]*)/([1-9][0-9]*))?$"
)
WAKE = re.compile(r"^wake\s+([1-9][0-9]*)(us|ms|s)$")
BOARDS = {
    "nrf52840-s140": ("s140", 128 * 1024, 32 * 1024),
    "nrf52840-nosd": ("nosd", 128 * 1024, 32 * 1024),
}
MAX_WRAP_SAFE_INTERVAL_US = 0x7FFF_FFFF
ROLE = {
    "control": ("hard_realtime", 2048, 512, 5),
    "sensor": ("driver", 1024, 256, 10),
    "service": ("best_effort", 1024, 256, 20),
}


def parse_duration(value: str, unit: str) -> int:
    scale = {"us": 1, "ms": 1000, "s": 1_000_000}[unit]
    result = int(value) * scale
    if result > 0xFFFF_FFFF:
        raise ValueError("duration exceeds the firmware's u32 microsecond range")
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
    wake_latency_us = 0
    task_records = records[2:]
    if task_records and task_records[0][1].startswith("wake "):
        number, line = task_records[0]
        match = WAKE.fullmatch(line)
        if not match:
            raise ValueError(f"line {number}: expected 'wake <duration>'")
        wake_latency_us = parse_duration(*match.groups())
        task_records = task_records[1:]
    if not task_records:
        raise ValueError("at least one control, sensor, or service task is required")
    tasks = []
    channels = []
    for number, line in task_records:
        match = LINE.fullmatch(line)
        if not match:
            raise ValueError(
                f"line {number}: expected '<role> <name> every <duration> "
                "[phase <duration>] [deadline <duration>] [-> <task>]'"
            )
        (role, name, value, unit, phase_value, phase_unit,
         deadline_value, deadline_unit, destination, budget_value, budget_unit,
         blocking_value, blocking_unit, flash_override, ram_override) = match.groups()
        criticality, flash, ram, divisor = ROLE[role]
        period = parse_duration(value, unit)
        if period > MAX_WRAP_SAFE_INTERVAL_US:
            raise ValueError(
                f"line {number}: period exceeds the wrap-safe 32-bit half-range"
            )
        phase = (parse_duration(phase_value, phase_unit) if phase_value else 0)
        deadline = (parse_duration(deadline_value, deadline_unit)
                    if deadline_value else period)
        budget = (parse_duration(budget_value, budget_unit)
                  if budget_value else min(deadline, max(1, period // divisor)))
        blocking = (parse_duration(blocking_value, blocking_unit)
                    if blocking_value else 0)
        if phase >= period:
            raise ValueError(f"line {number}: phase must be below period")
        if deadline > period:
            raise ValueError(f"line {number}: deadline exceeds period")
        if budget + blocking > deadline:
            raise ValueError(f"line {number}: budget + blocking exceeds deadline")
        tasks.append({"name": name, "role": role, "criticality": criticality,
                      "flash": int(flash_override or flash),
                      "ram": int(ram_override or ram), "period_us": period,
                      "phase_us": phase, "deadline_us": deadline,
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
        "profile": {"flash": flash_limit, "ram": ram_limit,
                    "pool": max(8, len(tasks) + 1),
                    "wake_latency_us": wake_latency_us},
        "tasks": [{"name": "kernel", "criticality": "hard_realtime",
                   "flash": 12 * 1024, "ram": 3 * 1024, "pool": 2,
                   "phase_us": 0, "deadline_us": 20_000,
                   "period_us": 20_000, "budget_us": 0}] + tasks,
        "channels": channels,
    }
    return {"app": app, "board": board, "workload": workload,
            "user_lines": len(records)}


def rust_main(spec: dict) -> str:
    task_count = len(spec["workload"]["tasks"])
    return f'''//! Generated from app.nobro. Regenerate instead of editing this file.
#![no_std]
#![no_main]
use cortex_m::asm;
use cortex_m_rt::entry;
use panic_halt as _;
use nobro_hal as _;
use nobro_kernel::NanoKernel;

include!(concat!(env!("OUT_DIR"), "/nobro_admitted.rs"));

#[no_mangle]
#[used]
static mut NOBRO_APP_REPORT: [u32; 4] = [0; 4];

#[entry]
fn main() -> ! {{
    let Ok(mut kernel) = NanoKernel::<{task_count}>::new(&NOBRO_ADMITTED_WORKLOAD, 0) else {{
        loop {{ asm::wfi(); }}
    }};
    let released = kernel.release_due(0);
    let first = kernel.take_next().map(|index| index as u32).unwrap_or(u32::MAX);
    let admitted_schema = unsafe {{
        core::ptr::read_volatile(core::ptr::addr_of!(NOBRO_ADMITTED_WORKLOAD.schema_version))
    }};
    unsafe {{
        core::ptr::write_volatile(core::ptr::addr_of_mut!(NOBRO_APP_REPORT),
            [0x4e42_4150 | u32::from(admitted_schema), NOBRO_ADMITTED_WORKLOAD.task_count as u32,
             u32::from(released), first]);
    }}
    loop {{ asm::wfi(); }}
}}
'''


def rust_build(spec: dict, source_name: str = "app.nobro") -> str:
    workload = spec["workload"]
    tasks = workload["tasks"]
    channel_users = {name for channel in workload["channels"] for name in channel}
    contracts = []
    for index, task in enumerate(tasks):
        priority = {"hard_realtime": 0, "system": 1, "driver": 2,
                    "user": 3, "best_effort": 4}[task["criticality"]]
        contract = f"TaskContract::new({index}).priority({priority})"
        if int(task.get("budget_us", 0)) > 0:
            period = int(task["period_us"])
            phase = int(task.get("phase_us", 0))
            deadline = int(task.get("deadline_us", period))
            jitter = min(
                deadline - 1,
                max(1, period // (200 if task.get("role") == "control" else 100)),
            ) if deadline > 1 else 0
            contract += (f".deadline({period}, {deadline}, {jitter}, "
                         f"{int(task['budget_us'])}, {int(task.get('blocking_us', 0))})"
                         f".phase({phase})")
        contract += (f".memory({int(task.get('flash', 0))}, {int(task.get('ram', 0))}, "
                     f"{int(task.get('pool', 0))})")
        capabilities = (1 << 13) if task["name"] in channel_users else 0
        contract += f".capabilities({capabilities}).object_quotas(8, 8, 8)"
        contracts.append(f"        {contract},")
    labels = ", ".join(json.dumps(task["name"]) for task in tasks)
    profile = workload["profile"]
    return f'''use nobro_admission::{{admit, AdmittedWorkload,
    AdmissionProfile, TaskContract}};
use std::{{env, fs, path::PathBuf}};

const LABELS: [&str; {len(tasks)}] = [{labels}];
const TASKS: [TaskContract; {len(tasks)}] = [
{os.linesep.join(contracts)}
];
const PROFILE: AdmissionProfile = AdmissionProfile::new(
    {int(profile['flash'])}, {int(profile['ram'])}, {int(profile['pool'])}, {len(tasks)})
    .wake_latency_us({int(profile['wake_latency_us'])});

fn emit(table: AdmittedWorkload<{len(tasks)}>, path: &PathBuf) {{
    let source = format!(r#"use nobro_admission::{{{{AdmittedTask, AdmittedWorkload}}}};
#[link_section = ".rodata.nobro.admission"]
#[no_mangle]
#[used]
pub static NOBRO_ADMITTED_WORKLOAD: AdmittedWorkload<{len(tasks)}> = {{:?}};
"#, table);
    fs::write(path.join("nobro_admitted.rs"), source).expect("write admitted table");
}}

fn main() {{
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).expect("copy memory.x");
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed={source_name}");
    println!("cargo:rustc-link-search={{}}", out.display());
    match admit(TASKS, PROFILE) {{
        Ok(table) => emit(table, &out),
        Err(error) => {{
            let task = if error.task_index == u16::MAX {{
                "<workload>"
            }} else {{
                LABELS[usize::from(error.task_index)]
            }};
            panic!("{{}}: task `{{}}`; observed={{}} limit={{}}",
                error.code.diagnostic(), task, error.observed, error.limit);
        }}
    }}
}}
'''


def load_source(source: pathlib.Path) -> tuple[dict, str, str]:
    """Load one compact-text or strict Python-JSON app without executing code."""

    if source.suffix.lower() == ".json":
        app = NobroApp.read_json(source)
        return app.firmware_spec(), "python-json", "app.json"
    text = source.read_text(encoding="utf-8")
    return parse(text), "compact-text", "app.nobro"


def generate(source: pathlib.Path, out_dir: pathlib.Path) -> dict:
    spec, source_format, source_name = load_source(source)
    project = (out_dir / spec["app"]).resolve()
    (project / "src").mkdir(parents=True, exist_ok=True)
    (project / ".cargo").mkdir(parents=True, exist_ok=True)
    if source_format == "python-json":
        canonical = NobroApp.read_json(source).to_dict()
        (project / source_name).write_text(
            json.dumps(canonical, indent=2) + "\n",
            encoding="utf-8",
            newline="\n",
        )
    else:
        (project / source_name).write_text(
            source.read_text(encoding="utf-8"),
            encoding="utf-8",
            newline="\n",
        )
    (project / "workload.json").write_text(
        json.dumps(spec["workload"], indent=2) + "\n", encoding="utf-8", newline="\n")
    kernel = ROOT / "core" / "crates" / "nobro_kernel"
    admission = ROOT / "core" / "crates" / "nobro_admission"
    hal = ROOT / "core" / "crates" / "nobro_hal"
    try:
        kernel_path = os.path.relpath(kernel, project).replace("\\", "/")
    except ValueError:
        kernel_path = str(kernel).replace("\\", "/")
    try:
        admission_path = os.path.relpath(admission, project).replace("\\", "/")
    except ValueError:
        admission_path = str(admission).replace("\\", "/")
    try:
        hal_path = os.path.relpath(hal, project).replace("\\", "/")
    except ValueError:
        hal_path = str(hal).replace("\\", "/")
    hal_feature = ("board-nicenano-s140" if spec["board"] == "nrf52840-s140"
                   else "board-promicro-nosd")
    cargo = f'''[package]
name = "nobro-app-{spec['app'].replace('_', '-')}"
version = "0.1.0"
edition = "2021"
publish = false
build = "build.rs"

[workspace]

[dependencies]
nobro-kernel = {{ path = {json.dumps(kernel_path)} }}
nobro-admission = {{ path = {json.dumps(admission_path)} }}
nobro-hal = {{ path = {json.dumps(hal_path)}, default-features = false, features = [{json.dumps(hal_feature)}] }}
cortex-m = "0.7"
cortex-m-rt = "0.7"
panic-halt = "0.2"

[build-dependencies]
nobro-admission = {{ path = {json.dumps(admission_path)} }}

[profile.release]
opt-level = "z"
lto = "fat"
codegen-units = 1
'''
    (project / "Cargo.toml").write_text(cargo, encoding="utf-8", newline="\n")
    (project / ".cargo" / "config.toml").write_text('''[build]
target = "thumbv7em-none-eabihf"

[target.thumbv7em-none-eabihf]
rustflags = [
  "-C", "link-arg=-Tlink.x",
  "-C", "link-arg=--nmagic",
]
''', encoding="utf-8", newline="\n")
    profile = BOARDS[spec["board"]][0]
    shutil.copyfile(ROOT / "core" / f"memory-{profile}.x", project / "memory.x")
    (project / "build.rs").write_text(
        rust_build(spec, source_name), encoding="utf-8", newline="\n"
    )
    (project / "src" / "main.rs").write_text(rust_main(spec), encoding="utf-8", newline="\n")
    metadata = {"schema": "nobro-firmware-project-v1", "app": spec["app"],
                "board": spec["board"], "memory_profile": profile,
                "source_format": source_format,
                "user_lines": spec["user_lines"], "generated_rust_lines": len(rust_main(spec).splitlines()),
                "task_count": len(spec["workload"]["tasks"]) - 1}
    (project / "generation.json").write_text(
        json.dumps(metadata, indent=2) + "\n", encoding="utf-8", newline="\n")
    return {"project": project, **metadata}


def build(project: pathlib.Path) -> subprocess.CompletedProcess:
    manifest = project / "Cargo.toml"
    lockfile = project / "Cargo.lock"
    if not lockfile.is_file():
        # A generated standalone project needs one explicit first resolution. Cargo
        # persists it beside the manifest; all builds below then fail closed on drift.
        resolved = subprocess.run(
            ["cargo", "generate-lockfile", "--manifest-path", str(manifest)],
            cwd=project,
            text=True,
            capture_output=True,
        )
        if resolved.returncode:
            return resolved
    return subprocess.run(
        [
            "cargo", "build", "--locked", "--release", "--target",
            "thumbv7em-none-eabihf", "--manifest-path", str(manifest),
        ],
        cwd=project,
        text=True,
        capture_output=True,
    )


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
        "control motor every 5ms phase 1ms deadline 4ms budget 400us blocking 100us memory 3072/640"))
    assert overridden["workload"]["tasks"][1]["budget_us"] == 400
    assert overridden["workload"]["tasks"][1]["blocking_us"] == 100
    assert overridden["workload"]["tasks"][1]["phase_us"] == 1000
    assert overridden["workload"]["tasks"][1]["deadline_us"] == 4000
    assert overridden["workload"]["tasks"][1]["ram"] == 640
    with_wake = parse(sample.replace(
        "board nrf52840-s140", "board nrf52840-s140\nwake 25us"))
    assert with_wake["workload"]["profile"]["wake_latency_us"] == 25
    shortest = parse(sample.replace("control motor every 5ms", "control motor every 1us"))
    assert shortest["workload"]["tasks"][1]["budget_us"] == 1
    with tempfile.TemporaryDirectory() as tmp:
        source = pathlib.Path(tmp) / "app.nobro"
        source.write_text(sample, encoding="utf-8")
        result = generate(source, pathlib.Path(tmp) / "out")
        assert result["memory_profile"] == "s140" and result["user_lines"] == 5
        assert (result["project"] / "src" / "main.rs").is_file()
        assert "-Tlink.x" in (result["project"] / ".cargo" / "config.toml").read_text()
        generated = (result["project"] / "src" / "main.rs").read_text()
        assert "NanoKernel::<4>" in generated and "NOBRO_ADMITTED_WORKLOAD" in generated
        build_source = (result["project"] / "build.rs").read_text()
        assert "nobro_admission::{admit" in build_source
        assert '"motor", "imu", "camera"' in build_source
        assert "TaskContract::new(1).priority(0).deadline(5000, 5000" in build_source
        assert "TaskContract::new(3).priority(4).deadline(40000, 40000" in build_source
        assert ".phase(0)" in build_source
        assert ".wake_latency_us(0)" in build_source
        python_app = (
            NobroApp("python_rover", board="nrf52840-nosd")
            .task("motor", 5_000, role="control")
            .task("imu", 10_000)
            .wire("imu", "motor", 8)
        )
        python_source = pathlib.Path(tmp) / "app.json"
        python_app.write_json(python_source)
        python_result = generate(python_source, pathlib.Path(tmp) / "python-out")
        assert python_result["source_format"] == "python-json"
        assert python_result["memory_profile"] == "nosd"
        python_workload = json.loads(
            (python_result["project"] / "workload.json").read_text(encoding="utf-8")
        )
        assert python_workload["channels"] == [["imu", "motor"]]
        assert python_workload["wire_capacities"] == [["imu", "motor", 8]]
        assert "rerun-if-changed=app.json" in (
            python_result["project"] / "build.rs"
        ).read_text(encoding="utf-8")
    for invalid in (sample.replace("motor every", "motor motor every"),
                    sample.replace("-> motor", "-> missing"),
                    sample.replace("nrf52840-s140", "unknown"),
                    sample.replace("control motor every 5ms",
                                   "control motor every 2147483648us")):
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
