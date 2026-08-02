#!/usr/bin/env python3
"""Generate host-readable contracts and production firmware from one small app file.

The input is either the compact declaration below or strict JSON exported by
``nobro_rtos.NobroApp``. A ``nobro-workload-v1`` file produced by
``nobro project new`` is also a direct native-firmware input, so its top-level
``features`` object remains the only optional-feature switchboard. It is
configuration, not generated Rust boilerplate::

    app rover
    board nrf52840-s140
    wake 25us
    control motor every 5ms
    periodic imu every 10ms -> motor
    service camera every 40ms

Memory estimates are inferred by role. Execution budgets are never inferred: omit
``budget`` to leave the task unmeasured and outside deadline admission, or provide an
explicit measured bound. The original declaration remains the auditable source used
to regenerate firmware. Standalone-image and Arduino-composition routes build the
declared application. Maintained-port routes emit an integration plan and reject
``--build`` until that startup owns declaration linking. A compatible logical stack
can be selected explicitly, for example ``backend wifi_link
backend-wifi-arduino-esp8266`` on ``wemos-d1-mini``.
"""
import argparse
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "bindings" / "python"))
sys.path.insert(0, str(ROOT / "tools" / "cli" / "project"))

from nobro_rtos.app import NobroApp  # noqa: E402
import nobro_project as project_model  # noqa: E402

DEFAULT_OUT = ROOT / "_work" / "projects"
NAME = re.compile(r"^[a-z][a-z0-9_-]{0,47}$")
LINE = re.compile(
    r"^(periodic|sensor|control|service)\s+([a-z][a-z0-9_-]{0,47})\s+"
    r"every\s+([1-9][0-9]*)(us|ms|s)"
    r"(?:\s+phase\s+([0-9]+)(us|ms|s))?"
    r"(?:\s+deadline\s+([1-9][0-9]*)(us|ms|s))?"
    r"(?:\s*->\s*([a-z][a-z0-9_-]{0,47}))?"
    r"(?:\s+budget\s+([1-9][0-9]*)(us|ms|s))?"
    r"(?:\s+blocking\s+([1-9][0-9]*)(us|ms|s))?"
    r"(?:\s+memory\s+([1-9][0-9]*)/([1-9][0-9]*))?$"
)
BACKEND = re.compile(
    r"^backend\s+([a-z][a-z0-9_]*)\s+([a-z][a-z0-9-]*)$"
)
WAKE = re.compile(r"^wake\s+([1-9][0-9]*)(us|ms|s)$")
BOARD_ROOT = ROOT / "core" / "boards"
PROVIDER_REGISTRY = BOARD_ROOT / "feature_providers.json"
MAX_WRAP_SAFE_INTERVAL_US = 0x7FFF_FFFF
ROLE = {
    "periodic": ("driver", 1024, 256),
    "control": ("hard_realtime", 1024, 256),
    "service": ("best_effort", 1024, 256),
}


def load_board_profiles() -> dict:
    profiles = {}
    for path in sorted(BOARD_ROOT.glob("*/*/board.json")):
        profile = json.loads(path.read_text(encoding="utf-8"))
        name = path.parent.name
        profile["_path"] = path
        profile["_name"] = name
        profiles[name] = profile
    return profiles


def board_profile(name: str, *, require_generation: bool = True) -> dict:
    profiles = load_board_profiles()
    if name not in profiles:
        raise ValueError(
            f"unsupported board profile {name!r}; choose {', '.join(sorted(profiles))}"
        )
    profile = profiles[name]
    generation = profile.get("firmware_generation")
    if require_generation and (
        not isinstance(generation, dict)
        or generation.get("support") != "application-image"
    ):
        raise ValueError(
            f"board profile {name!r} has no standalone application-image contract; "
            "use its maintained port until that board owns one"
        )
    return profile


def select_backends(profile: dict, requests: list[tuple[str, str]]) -> list[dict]:
    """Resolve explicit logical-stack selections against the public registry."""

    registry = json.loads(PROVIDER_REGISTRY.read_text(encoding="utf-8"))
    known = {item["id"] for item in registry.get("backends", [])}
    framework = str(profile["composition"].get("framework_core", ""))
    composition = "arduino" if "arduino" in framework else "native"
    selected = []
    seen = set()
    for capability, backend_id in requests:
        if capability in seen:
            raise ValueError(f"backend for {capability!r} is selected more than once")
        if backend_id not in known:
            raise ValueError(f"unknown backend {backend_id!r}")
        matches = [
            binding for binding in registry.get("bindings", [])
            if binding.get("backend_id") == backend_id
            and binding.get("capability_kind") == capability
            and binding.get("platform") == profile["platform_id"]
            and binding.get("composition") == composition
            and binding.get("maturity") not in {"absent", "stub"}
        ]
        if len(matches) != 1:
            raise ValueError(
                f"backend {backend_id!r} is unavailable for capability {capability!r} "
                f"on {profile['_name']!r} ({composition})"
            )
        binding = matches[0]
        selected.append({
            "capability": capability,
            "backend_id": backend_id,
            "binding_id": binding["id"],
            "instance": binding.get("instance"),
            "maturity": binding["maturity"],
        })
        seen.add(capability)
    return selected


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
    profile = board_profile(board, require_generation=False)
    wake_latency_us = 0
    task_records = records[2:]
    if task_records and task_records[0][1].startswith("wake "):
        number, line = task_records[0]
        match = WAKE.fullmatch(line)
        if not match:
            raise ValueError(f"line {number}: expected 'wake <duration>'")
        wake_latency_us = parse_duration(*match.groups())
        task_records = task_records[1:]
    backend_requests = []
    while task_records and task_records[0][1].startswith("backend "):
        number, line = task_records[0]
        match = BACKEND.fullmatch(line)
        if not match:
            raise ValueError(
                f"line {number}: expected 'backend <capability_kind> <backend-id>'"
            )
        backend_requests.append(match.groups())
        task_records = task_records[1:]
    if not task_records:
        raise ValueError("at least one periodic, control, or service task is required")
    tasks = []
    channels = []
    for number, line in task_records:
        match = LINE.fullmatch(line)
        if not match:
            raise ValueError(
                f"line {number}: expected '<role> <name> every <duration> "
                "[phase <duration>] [deadline <duration>] [-> <task>] "
                "[budget <duration>] [blocking <duration>] [memory <flash>/<ram>]'"
            )
        (role, name, value, unit, phase_value, phase_unit,
         deadline_value, deadline_unit, destination, budget_value, budget_unit,
         blocking_value, blocking_unit, flash_override, ram_override) = match.groups()
        role = "periodic" if role == "sensor" else role
        criticality, flash, ram = ROLE[role]
        period = parse_duration(value, unit)
        if period > MAX_WRAP_SAFE_INTERVAL_US:
            raise ValueError(
                f"line {number}: period exceeds the wrap-safe 32-bit half-range"
            )
        phase = (parse_duration(phase_value, phase_unit) if phase_value else 0)
        deadline = (parse_duration(deadline_value, deadline_unit)
                    if deadline_value else period)
        budget = parse_duration(budget_value, budget_unit) if budget_value else 0
        blocking = (parse_duration(blocking_value, blocking_unit)
                    if blocking_value else 0)
        if phase >= period:
            raise ValueError(f"line {number}: phase must be below period")
        if deadline > period:
            raise ValueError(f"line {number}: deadline exceeds period")
        if blocking and not budget:
            raise ValueError(
                f"line {number}: blocking requires an explicit execution budget"
            )
        if budget and budget + blocking > deadline:
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
    capacity = profile["capacity"]
    flash_limit = int(capacity["flash_budget_bytes"])
    ram_limit = int(capacity["ram_budget_bytes"])
    workload = {
        "schema": "nobro-workload-v1",
        "target": board,
        "features": {},
        "profile": {"flash": flash_limit, "ram": ram_limit,
                    "pool": max(8, len(tasks) + 1),
                    "wake_latency_us": wake_latency_us},
        "tasks": [{"name": "kernel", "criticality": "hard_realtime",
                   "flash": 12 * 1024, "ram": 3 * 1024, "pool": 2,
                   "phase_us": 0, "deadline_us": 20_000,
                   "period_us": 20_000, "budget_us": 0}] + tasks,
        "channels": channels,
        "backends": select_backends(profile, backend_requests),
    }
    return {"app": app, "board": board, "workload": workload,
            "user_lines": len(records)}


def rust_main(spec: dict) -> str:
    task_count = len(spec["workload"]["tasks"])
    enabled = {name for name, _ in project_model.selected_features(spec["workload"])}
    capacity_import = ""
    capacity_marker = ""
    capacity_setup = ""
    report_len = 4
    report_tail = "first"
    if "capacity-report" in enabled:
        capacity_import = "use nobro_kernel::{CapacityRegistry, CapacityResource, NanoKernel};"
        capacity_marker = """
#[no_mangle]
#[used]
pub static NOBRO_FEATURE_CAPACITY_REPORT: u8 = 1;
"""
        capacity_setup = """
    let mut capacity = CapacityRegistry::<1>::new();
    let identity_byte = u8::try_from(admitted_schema).unwrap_or(u8::MAX);
    let resource = CapacityResource::mailbox([identity_byte; 32], 1, 1);
    let capacity_len = capacity
        .register(resource)
        .map(|()| capacity.len() as u32)
        .unwrap_or(u32::MAX);
"""
        report_len = 5
        report_tail = "first, capacity_len"
    else:
        capacity_import = "use nobro_kernel::NanoKernel;"
    return f'''//! Generated from app.nobro. Regenerate instead of editing this file.
#![no_std]
#![no_main]
use cortex_m::asm;
use cortex_m_rt::entry;
use panic_halt as _;
use nobro_hal as _;
{capacity_import}

include!(concat!(env!("OUT_DIR"), "/nobro_admitted.rs"));

#[no_mangle]
#[used]
static mut NOBRO_APP_REPORT: [u32; {report_len}] = [0; {report_len}];
{capacity_marker}

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
{capacity_setup}
    unsafe {{
        core::ptr::write_volatile(core::ptr::addr_of_mut!(NOBRO_APP_REPORT),
            [0x4e42_4150 | u32::from(admitted_schema), NOBRO_ADMITTED_WORKLOAD.task_count as u32,
             u32::from(released), {report_tail}]);
    }}
    loop {{ asm::wfi(); }}
}}
'''


def rust_build(spec: dict, source_name: str = "app.nobro") -> str:
    workload, _ = project_model.priced_workload(spec["workload"])
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
    """Load compact text, Python JSON, or a canonical workload without executing code."""

    if source.suffix.lower() == ".json":
        record = json.loads(source.read_text(encoding="utf-8"))
        if record.get("schema") in {"nobro-app-v1", "nobro-python-app-v1"}:
            app = NobroApp.from_dict(record)
            return app.firmware_spec(), "python-json", "app.json"
        if record.get("schema") == "nobro-workload-v1":
            app = record.get("app")
            if not isinstance(app, str) or not NAME.fullmatch(app):
                raise ValueError("canonical workload needs an `app` name")
            project_model.startup_order(
                record, allow_empty_features_without_catalog=True
            )
            board = record.get("target")
            board_profile(board, require_generation=False)
            return {
                "app": app,
                "board": board,
                "workload": record,
                "user_lines": len(record.get("tasks", [])),
            }, "canonical-workload", "workload.json"
        raise ValueError("JSON must use nobro-app-v1 or nobro-workload-v1")
    text = source.read_text(encoding="utf-8")
    return parse(text), "compact-text", "app.nobro"


def copy_canonical_inputs(
    source: pathlib.Path,
    project: pathlib.Path,
    spec: dict,
    source_format: str,
    source_name: str,
) -> None:
    project.mkdir(parents=True, exist_ok=True)
    if source_format == "python-json":
        canonical = NobroApp.read_json(source).to_dict()
        (project / source_name).write_text(
            json.dumps(canonical, indent=2) + "\n", encoding="utf-8", newline="\n"
        )
    elif source_format == "compact-text":
        (project / source_name).write_text(
            source.read_text(encoding="utf-8"), encoding="utf-8", newline="\n"
        )
    (project / "workload.json").write_text(
        json.dumps(spec["workload"], indent=2) + "\n", encoding="utf-8", newline="\n"
    )


def arduino_sketch(spec: dict, profile: dict) -> str:
    tasks = spec["workload"]["tasks"][1:]
    channels = spec["workload"]["channels"]
    names = {task["name"]: f"task_{index}" for index, task in enumerate(tasks)}
    lines = [
        "// Generated from the adjacent canonical declaration; regenerate instead of editing.",
        "#include <NobroRTOS.h>",
        "",
        f"nobro::NobroApp<{len(tasks)}, {max(1, len(channels))}> app(",
        f"    {int(profile['capacity']['flash_budget_bytes'])}ul,",
        f"    {int(profile['capacity']['ram_budget_bytes'])}ul);",
        "",
        "void setup() {",
        "  Serial.begin(115200);",
    ]
    roles = {"control": "nobro::CONTROL", "periodic": "nobro::PERIODIC", "service": "nobro::SERVICE"}
    for task in tasks:
        variable = names[task["name"]]
        lines.append(
            f'  nobro::TaskId {variable} = app.task("{task["name"]}", '
            f'{int(task["period_us"])}ul, {roles[task["role"]]});'
        )
        if int(task.get("budget_us", 0)):
            lines.append(f"  app.budget({variable}, {int(task['budget_us'])}ul);")
        lines.append(
            f"  app.memory({variable}, {int(task['flash'])}ul, {int(task['ram'])}ul);"
        )
    for source, destination in channels:
        lines.append(f"  app.wire({names[source]}, {names[destination]});")
    lines.extend([
        '  Serial.println(app.admit() ? "NobroRTOS app ready" : app.errorText());',
        "}",
        "",
        "void loop() {}",
        "",
    ])
    return "\n".join(lines)


def route_metadata(spec: dict, profile: dict, source_format: str) -> dict:
    generation = profile["firmware_generation"]
    support = generation["support"]
    capacity = profile["capacity"]
    metadata = {
        "schema": "nobro-firmware-project-v2",
        "app": spec["app"],
        "board": spec["board"],
        "board_id": profile["board_id"],
        "route": support,
        "source_format": source_format,
        "user_lines": spec["user_lines"],
        "task_count": len(spec["workload"]["tasks"]) - 1,
        "selected_backends": spec["workload"].get("backends", []),
        "budget": {
            "flash_bytes": int(capacity["flash_budget_bytes"]),
            "ram_bytes": int(capacity["ram_budget_bytes"]),
            "sample_pool_slots": int(capacity["sample_pool_slots"]),
            "max_modules": int(capacity["max_modules"]),
        },
        "image_layout": profile["boot"],
        "generation_contract": {
            key: generation[key]
            for key in ("entry", "interrupts", "dma", "clock", "boot")
            if key in generation
        },
    }
    if support == "arduino-composition":
        metadata.update({
            "fqbn": generation["fqbn"],
            "required_header": generation["header"],
            "build_command": [
                "arduino-cli", "compile", "--fqbn", generation["fqbn"],
                "--library", "<NOBRO_RTOS>/packages/arduino", ".",
            ],
        })
    elif support == "maintained-port":
        metadata.update({
            "cargo_target": generation["cargo_target"],
            "runtime_manifest": generation["runtime_manifest"],
            "build_available": False,
            "diagnostic": (
                "The maintained startup port can be built independently, but this declaration "
                "is not linked into that image. Select a board-owned application-image or "
                "Arduino-composition route for generated application firmware."
            ),
        })
    return metadata


def deployment_guide(profile: dict, metadata: dict) -> str:
    composition = profile["composition"]
    route = metadata["route"]
    if route == "application-image":
        action = (
            "Build with `nobro firmware <source> --build`. Flash the resulting image "
            "with the board's documented bootloader/debug method; image addresses must "
            "match `image_layout` in `generation.json`."
        )
    elif route == "arduino-composition":
        action = (
            f"Build with Arduino CLI using `{metadata['fqbn']}` or select the equivalent "
            "board in Arduino IDE. Upload/recovery stays owned by that board core and "
            "bootloader; do not substitute another board's erase or reset recipe."
        )
    else:
        action = (
            f"The maintained startup lives at `{metadata['runtime_manifest']}`. This "
            "project is an audited integration plan, not an application binary; `--build` "
            "fails until that port owns declaration linking."
        )
    usb = composition.get("usb_stack") or "no MCU-owned USB stack declared"
    return (
        f"# Deploy {metadata['app']} on {metadata['board']}\n\n"
        f"{action}\n\n"
        f"- Boot layout: `{profile['boot']['layout']}`\n"
        f"- Framework/core: `{composition.get('framework_core') or 'none'}`\n"
        f"- USB/serial ownership: `{usb}`\n"
        "- Recovery rule: use only the exact board/core procedure and preserve any "
        "bootloader or radio-reserved region. Host port names are intentionally not "
        "embedded in generated projects.\n"
        "- Budgets and selected backends are printed in `generation.json`; omitted "
        "backends remain detached.\n"
    )


def generate_routed_project(
    source: pathlib.Path,
    out_dir: pathlib.Path,
    spec: dict,
    source_format: str,
    source_name: str,
    profile: dict,
) -> dict:
    support = profile["firmware_generation"]["support"]
    if support == "unavailable":
        raise ValueError(
            f"board profile {spec['board']!r} has no generated firmware route: "
            f"{profile['firmware_generation']['reason']}"
        )
    project = (out_dir / spec["app"]).resolve()
    copy_canonical_inputs(source, project, spec, source_format, source_name)
    metadata = route_metadata(spec, profile, source_format)
    if support == "arduino-composition":
        (project / f"{spec['app']}.ino").write_text(
            arduino_sketch(spec, profile), encoding="utf-8", newline="\n"
        )
    (project / "generation.json").write_text(
        json.dumps(metadata, indent=2) + "\n", encoding="utf-8", newline="\n"
    )
    (project / "DEPLOY.md").write_text(
        deployment_guide(profile, metadata), encoding="utf-8", newline="\n"
    )
    return {"project": project, **metadata}


def generate(source: pathlib.Path, out_dir: pathlib.Path) -> dict:
    spec, source_format, source_name = load_source(source)
    spec["workload"].setdefault("schema", "nobro-workload-v1")
    spec["workload"].setdefault("target", spec["board"])
    spec["workload"].setdefault("features", {})
    project_model.startup_order(
        spec["workload"], allow_empty_features_without_catalog=True
    )
    profile = board_profile(spec["board"], require_generation=False)
    requested = spec["workload"].get("backends", [])
    if not isinstance(requested, list) or any(
        not isinstance(item, dict)
        or not isinstance(item.get("capability"), str)
        or not isinstance(item.get("backend_id"), str)
        for item in requested
    ):
        raise ValueError(
            "backends must be a list of {capability, backend_id} selections"
        )
    spec["workload"]["backends"] = select_backends(
        profile,
        [(item["capability"], item["backend_id"]) for item in requested],
    )
    if profile["firmware_generation"]["support"] != "application-image":
        return generate_routed_project(
            source, out_dir, spec, source_format, source_name, profile
        )
    project = (out_dir / spec["app"]).resolve()
    (project / "src").mkdir(parents=True, exist_ok=True)
    (project / ".cargo").mkdir(parents=True, exist_ok=True)
    # Generation rewrites the manifest, so a lockfile from an earlier SDK
    # version is not evidence for the new project graph. Remove it here and let
    # build() perform one explicit resolution before enforcing --locked.
    (project / "Cargo.lock").unlink(missing_ok=True)
    copy_canonical_inputs(source, project, spec, source_format, source_name)
    generation = profile["firmware_generation"]
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
    hal_feature = generation["hal_feature"]
    kernel_features = project_model.cargo_kernel_features(spec["workload"])
    kernel_feature_clause = (
        f", features = {json.dumps(kernel_features)}" if kernel_features else ""
    )
    cargo = f'''[package]
name = "nobro-app-{spec['app'].replace('_', '-')}"
version = "0.1.0"
edition = "2021"
publish = false
build = "build.rs"

[workspace]

[dependencies]
nobro-kernel = {{ path = {json.dumps(kernel_path)}{kernel_feature_clause} }}
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
    cargo_target = generation["cargo_target"]
    rustflags = generation["rustflags"]
    config = (
        f"[build]\ntarget = {json.dumps(cargo_target)}\n\n"
        f"[target.{cargo_target}]\n"
        f"rustflags = {json.dumps(rustflags)}\n"
    )
    (project / ".cargo" / "config.toml").write_text(
        config, encoding="utf-8", newline="\n"
    )
    linker_source = ROOT / generation["linker_script"]
    shutil.copyfile(linker_source, project / "memory.x")
    (project / "build.rs").write_text(
        rust_build(spec, source_name), encoding="utf-8", newline="\n"
    )
    (project / "src" / "main.rs").write_text(rust_main(spec), encoding="utf-8", newline="\n")
    metadata = {"schema": "nobro-firmware-project-v2", "app": spec["app"],
                "board": spec["board"], "board_id": profile["board_id"],
                "route": "application-image",
                "cargo_target": cargo_target,
                "memory_profile": generation["memory_profile"],
                "budget": {
                    "flash_bytes": int(profile["capacity"]["flash_budget_bytes"]),
                    "ram_bytes": int(profile["capacity"]["ram_budget_bytes"]),
                    "sample_pool_slots": int(profile["capacity"]["sample_pool_slots"]),
                    "max_modules": int(profile["capacity"]["max_modules"]),
                },
                "image_layout": profile["boot"],
                "selected_backends": spec["workload"].get("backends", []),
                "generation_contract": {
                    key: generation[key]
                    for key in ("entry", "interrupts", "dma", "clock", "boot")
                },
                "source_format": source_format,
                "user_lines": spec["user_lines"], "generated_rust_lines": len(rust_main(spec).splitlines()),
                "task_count": len(spec["workload"]["tasks"]) - 1,
                "features": [name for name, _ in project_model.selected_features(
                    spec["workload"])]}
    (project / "generation.json").write_text(
        json.dumps(metadata, indent=2) + "\n", encoding="utf-8", newline="\n")
    (project / "DEPLOY.md").write_text(
        deployment_guide(profile, metadata), encoding="utf-8", newline="\n"
    )
    return {"project": project, **metadata}


def build(project: pathlib.Path) -> subprocess.CompletedProcess:
    metadata = json.loads((project / "generation.json").read_text(encoding="utf-8"))
    route = metadata.get("route", "application-image")
    if route == "maintained-port":
        return subprocess.CompletedProcess(
            args=["nobro", "firmware", "--build"],
            returncode=2,
            stdout="",
            stderr=f"FIRMWARE BUILD: unavailable ({metadata['diagnostic']})\n",
        )
    if route == "arduino-composition":
        build_path = project / ".nobro-build"
        shutil.rmtree(build_path, ignore_errors=True)
        return subprocess.run(
            [
                "arduino-cli", "compile", "--fqbn", metadata["fqbn"],
                "--library", str(ROOT / "packages" / "arduino"),
                "--build-path", str(build_path), str(project),
            ],
            cwd=project,
            text=True,
            encoding="utf-8",
            errors="replace",
            capture_output=True,
        )
    manifest = project / "Cargo.toml"
    lockfile = project / "Cargo.lock"
    if not lockfile.is_file():
        # A generated standalone project needs one explicit first resolution. Cargo
        # persists it beside the manifest; all builds below then fail closed on drift.
        resolved = subprocess.run(
            ["cargo", "generate-lockfile", "--manifest-path", str(manifest)],
            cwd=project,
            text=True,
            encoding="utf-8",
            errors="replace",
            capture_output=True,
        )
        if resolved.returncode:
            return resolved
    return subprocess.run(
        [
            "cargo", "build", "--locked", "--release", "--target",
            metadata["cargo_target"], "--manifest-path", str(manifest),
        ],
        cwd=project,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
    )


def selftest() -> int:
    import tempfile
    sample = """app rover
board nrf52840-s140
control motor every 5ms
periodic imu every 10ms -> motor
service camera every 40ms
"""
    spec = parse(sample)
    assert spec["user_lines"] == 5 and len(spec["workload"]["tasks"]) == 4
    assert spec["workload"]["channels"] == [["imu", "motor"]]
    assert spec["workload"]["tasks"][1]["budget_us"] == 0
    alias = parse(sample.replace("periodic imu", "sensor imu"))
    assert alias["workload"]["tasks"][2]["role"] == "periodic"
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
    assert shortest["workload"]["tasks"][1]["budget_us"] == 0
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
        assert "TaskContract::new(1).priority(0).deadline(" not in build_source
        assert "TaskContract::new(3).priority(4).deadline(" not in build_source
        assert ".wake_latency_us(0)" in build_source
        stale_lock = result["project"] / "Cargo.lock"
        stale_lock.write_text("# stale SDK graph\n", encoding="utf-8")
        generate(source, pathlib.Path(tmp) / "out")
        assert not stale_lock.exists()
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
        assert python_workload["tasks"][1]["budget_us"] == 0
        assert "rerun-if-changed=app.json" in (
            python_result["project"] / "build.rs"
        ).read_text(encoding="utf-8")

        canonical = json.loads(
            (result["project"] / "workload.json").read_text(encoding="utf-8")
        )
        canonical["target"] = "nrf52840-nosd"
        canonical["app"] = "priced_rover"
        canonical["features"] = {"capacity-report": True}
        workload_source = pathlib.Path(tmp) / "priced-workload.json"
        workload_source.write_text(
            json.dumps(canonical, indent=2) + "\n", encoding="utf-8"
        )
        priced_result = generate(workload_source, pathlib.Path(tmp) / "priced-out")
        assert priced_result["source_format"] == "canonical-workload"
        assert priced_result["features"] == ["capacity-report"]
        priced_main = (priced_result["project"] / "src" / "main.rs").read_text(
            encoding="utf-8"
        )
        priced_cargo = (priced_result["project"] / "Cargo.toml").read_text(
            encoding="utf-8"
        )
        assert "NOBRO_FEATURE_CAPACITY_REPORT" in priced_main
        assert "CapacityResource::mailbox" in priced_main
        assert 'features = ["capacity-report"]' in priced_cargo
        canonical["app"] = "unpriced_rover"
        canonical["features"] = {}
        workload_source.write_text(
            json.dumps(canonical, indent=2) + "\n", encoding="utf-8"
        )
        plain_result = generate(workload_source, pathlib.Path(tmp) / "plain-out")
        assert "NOBRO_FEATURE_CAPACITY_REPORT" not in (
            plain_result["project"] / "src" / "main.rs"
        ).read_text(encoding="utf-8")
        for board, target in (
            ("uno-r4-wifi", "thumbv7em-none-eabihf"),
            ("samd21-uf2", "thumbv6m-none-eabi"),
        ):
            portable = sample.replace("nrf52840-s140", board).replace(
                "app rover", f"app {board.replace('-', '_')}"
            )
            portable_source = pathlib.Path(tmp) / f"{board}.nobro"
            portable_source.write_text(portable, encoding="utf-8")
            portable_result = generate(portable_source, pathlib.Path(tmp) / "portable")
            assert portable_result["cargo_target"] == target
            assert portable_result["generation_contract"]["boot"]
            python_portable = NobroApp(
                f"python_{board.replace('-', '_')}", board=board
            ).task("control", 5_000, role="control", budget_us=200)
            python_portable_source = pathlib.Path(tmp) / f"{board}.json"
            python_portable.write_json(python_portable_source)
            python_portable_result = generate(
                python_portable_source, pathlib.Path(tmp) / "python-portable"
            )
            assert python_portable_result["cargo_target"] == target
        arduino_source = pathlib.Path(tmp) / "nano.nobro"
        arduino_source.write_text(
            sample.replace("nrf52840-s140", "nano-v3-atmega328p").replace(
                "app rover", "app nano_rover"
            ),
            encoding="utf-8",
        )
        arduino_result = generate(arduino_source, pathlib.Path(tmp) / "arduino")
        assert arduino_result["route"] == "arduino-composition"
        assert arduino_result["fqbn"] == "arduino:avr:nano:cpu=atmega328old"
        assert arduino_result["budget"]["ram_bytes"] == 1024
        assert (arduino_result["project"] / "nano_rover.ino").is_file()
        assert "app.wire(task_1, task_0)" in (
            arduino_result["project"] / "nano_rover.ino"
        ).read_text(encoding="utf-8")

        backend_source = pathlib.Path(tmp) / "esp8266.nobro"
        backend_source.write_text(
            sample.replace("nrf52840-s140", "wemos-d1-mini")
            .replace("app rover", "app wifi_rover")
            .replace(
                "control motor every 5ms",
                "backend wifi_link backend-wifi-arduino-esp8266\n"
                "control motor every 5ms",
            ),
            encoding="utf-8",
        )
        backend_result = generate(backend_source, pathlib.Path(tmp) / "backend")
        assert backend_result["selected_backends"][0]["binding_id"] == (
            "binding-wifi-arduino-esp8266"
        )

        port_source = pathlib.Path(tmp) / "c3.nobro"
        port_source.write_text(
            sample.replace("nrf52840-s140", "esp32c3-supermini").replace(
                "app rover", "app c3_rover"
            ),
            encoding="utf-8",
        )
        port_result = generate(port_source, pathlib.Path(tmp) / "port")
        assert port_result["route"] == "maintained-port"
        assert port_result["build_available"] is False
        assert not (port_result["project"] / "Cargo.toml").exists()
        assert build(port_result["project"]).returncode == 2
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
    for invalid_backend in (
        sample.replace("board nrf52840-s140", (
            "board nano-v3-atmega328p\n"
            "backend wifi_link backend-wifi-arduino-esp8266"
        )),
        sample.replace("board nrf52840-s140", (
            "board wemos-d1-mini\n"
            "backend wifi_link backend-wifi-arduino-esp8266\n"
            "backend wifi_link backend-wifi-arduino-esp8266"
        )),
    ):
        try:
            parse(invalid_backend)
            raise AssertionError("invalid backend selection accepted")
        except ValueError:
            pass
    with tempfile.TemporaryDirectory() as tmp:
        unavailable = pathlib.Path(tmp) / "unavailable.nobro"
        unavailable.write_text(
            sample.replace("nrf52840-s140", "cortexm-generic"), encoding="utf-8"
        )
        try:
            generate(unavailable, pathlib.Path(tmp) / "out")
            raise AssertionError("unavailable board route accepted")
        except ValueError as error:
            assert "no generated firmware route" in str(error)
    print("NOBRO FIRMWARE PROJECT SELFTEST: PASS (parse/generate/profiles/validation)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("source", nargs="?", type=pathlib.Path)
    parser.add_argument("--out", type=pathlib.Path, default=DEFAULT_OUT)
    parser.add_argument("--build", action="store_true")
    parser.add_argument("--explain", action="store_true",
                        help="print the expanded route, budgets, layout, and backends")
    parser.add_argument("--list-boards", action="store_true",
                        help="list exact profiles and their generation routes")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if args.list_boards:
        for name, profile in sorted(load_board_profiles().items()):
            generation = profile["firmware_generation"]
            detail = generation.get("reason", generation.get("entry", ""))
            print(f"{name}: {generation['support']} - {detail}")
        return 0
    if not args.source:
        parser.error("source is required")
    try:
        result = generate(args.source, args.out)
    except (OSError, ValueError) as error:
        print(f"FIRMWARE PROJECT: FAIL ({error})")
        return 1
    print(f"FIRMWARE PROJECT: generated {result['project']} from {result['user_lines']} user lines")
    if args.explain:
        explained = {key: value for key, value in result.items() if key != "project"}
        print(json.dumps(explained, indent=2))
    if args.build:
        completed = build(result["project"])
        print(f"FIRMWARE BUILD: {'PASS' if completed.returncode == 0 else 'FAIL'}")
        if completed.returncode:
            print("\n".join((completed.stdout + completed.stderr).splitlines()[-12:]))
        return completed.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
