#!/usr/bin/env python3
"""One-command project and diagnostic experience for NobroRTOS.

A single flow from "I have an idea" to a running, self-explaining app:

  nobro project new <name>            scaffold a workload + graph skeleton
  nobro project explain <workload>    explain the DERIVED contract in plain words
                                       (tasks, derived capabilities, startup order,
                                        marginal cost, schedulability, shed advice)
  nobro project run <project>         explain + real build + simulate + report
  nobro project report <report.json>  decode a project report
  nobro project shrink <report.json>  propose identity-bound capacity changes

Everything generated lands under an ignored work root (`_work/projects/<name>`)
unless `--out` says otherwise, so a scaffold never dirties the tree.

    python tools/nobro_project.py new blinky
    python tools/nobro_project.py explain _work/projects/blinky/workload.json
    python tools/nobro_project.py --selftest

Exit 0 on success; explain exits 1 only when the workload is INFEASIBLE.
"""
import argparse
import copy
import json
import os
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
import nobro_admission as adm  # noqa: E402  (marginal-cost + shed analysis)
import nobro_shrink as shrink  # noqa: E402  (fail-closed capacity proposals)

DEFAULT_OUT = ROOT / "_work" / "projects"
NAME = re.compile(r"^[a-z][a-z0-9_-]{0,47}$")
FEATURE_CATALOG_PATH = ROOT / "sdk" / "feature-catalog.json"

WORKLOAD_DIAGNOSTICS = {
    "shape": ("NOBRO-E030", "Workload must contain a non-empty task list."),
    "task-name": ("NOBRO-E031", "Every task needs one stable lowercase name."),
    "duplicate-task": ("NOBRO-E032", "Task names must be unique."),
    "missing-kernel": ("NOBRO-E033", "Every workload needs the kernel task."),
    "criticality": ("NOBRO-E034", "Task criticality must be one of the known roles."),
    "nonnegative": ("NOBRO-E035", "Resource and timing numbers must be non-negative."),
    "budget-period": ("NOBRO-E036", "A task budget must fit inside its period."),
    "after-list": ("NOBRO-E037", "Task dependencies must be a short unique list."),
    "dependency": ("NOBRO-E038", "Task dependencies must name existing tasks."),
    "kernel-after": ("NOBRO-E039", "The kernel starts first and cannot depend on app tasks."),
    "wire-shape": ("NOBRO-E040", "Each wire must be written as [from, to]."),
    "wire-endpoint": ("NOBRO-E041", "Wire endpoints must name existing tasks."),
    "cycle": ("NOBRO-E042", "Startup dependencies cannot form a cycle."),
    "feature-shape": ("NOBRO-E043", "Features must be one object of boolean switches."),
    "feature-target": ("NOBRO-E044", "Feature target is unsupported."),
    "feature-name": ("NOBRO-E045", "Feature name is unknown for this target."),
    "feature-value": ("NOBRO-E046", "Feature values must match the catalog."),
    "feature-unavailable": ("NOBRO-E047", "Feature is unavailable for this target."),
    "feature-conflict": ("NOBRO-E048", "Enabled features conflict."),
    "workload-schema": ("NOBRO-E049", "Workload schema version is unsupported."),
}
ADMISSION_DIAGNOSTIC_CODES = {f"NOBRO-E{number:03d}" for number in range(1, 22)}


class WorkloadDiagnostic(ValueError):
    """First-error diagnostic for beginner-facing workload validation."""

    def __init__(self, key: str, detail: str, *, task: str | None = None) -> None:
        code, summary = WORKLOAD_DIAGNOSTICS[key]
        self.code = code
        self.summary = summary
        self.detail = detail
        self.task = task
        prefix = f"{code}"
        if task is not None:
            prefix += f" task `{task}`"
        super().__init__(f"{prefix}: {detail}")

    def user_message(self) -> str:
        task = f" task `{self.task}`" if self.task is not None else ""
        return f"{self.code}{task}: {self.summary} {self.detail}"


def format_user_error(error: BaseException) -> str:
    if isinstance(error, WorkloadDiagnostic):
        return error.user_message()
    return str(error)


# --------------------------------------------------------------- new (scaffold)

WORKLOAD_TEMPLATE = {
    "schema": "nobro-workload-v1",
    "target": "nrf52840-nosd",
    "features": {},
    "profile": {"flash": 128 * 1024, "ram": 32 * 1024, "pool": 8},
    "tasks": [
        {"name": "kernel", "criticality": "hard_realtime",
         "flash": 12 * 1024, "ram": 3 * 1024, "pool": 2,
         "period_us": 20_000, "budget_us": 0},
        {"name": "control", "criticality": "hard_realtime",
         "flash": 2 * 1024, "ram": 512, "period_us": 20_000, "budget_us": 2_000},
        {"name": "sensor", "criticality": "driver",
         "flash": 1 * 1024, "ram": 256, "period_us": 100_000, "budget_us": 2_000},
    ],
    "channels": [["sensor", "control"]],
}


def feature_catalog() -> dict:
    catalog = json.loads(FEATURE_CATALOG_PATH.read_text(encoding="utf-8"))
    if catalog.get("schema") != "nobro-feature-catalog-v1":
        raise ValueError("unsupported Nobro feature catalog schema")
    targets = catalog.get("targets")
    if not isinstance(targets, dict) or not targets:
        raise ValueError("feature catalog needs at least one target")
    for target, entries in targets.items():
        if not NAME.fullmatch(target) or not isinstance(entries, dict):
            raise ValueError("feature catalog target entries are invalid")
        for name, entry in entries.items():
            if not NAME.fullmatch(name) or not isinstance(entry, dict):
                raise ValueError("feature catalog feature entries are invalid")
            if entry.get("value_type") != "boolean":
                raise ValueError(f"catalog feature {name} has an unsupported value type")
            if not isinstance(entry.get("kernel_features"), list):
                raise ValueError(f"catalog feature {name} needs kernel_features")
            if not isinstance(entry.get("conflicts"), list):
                raise ValueError(f"catalog feature {name} needs conflicts")
            if entry.get("status") == "selectable":
                price = entry.get("price")
                evidence = entry.get("evidence")
                fields = (
                    "flash_delta_bytes_max",
                    "static_ram_delta_bytes_max",
                    "total_ram_delta_bytes_max",
                )
                if not isinstance(price, dict) or any(
                    isinstance(price.get(field), bool)
                    or not isinstance(price.get(field), int)
                    or price[field] < 0
                    for field in fields
                ):
                    raise ValueError(f"catalog feature {name} needs non-negative prices")
                if not isinstance(price.get("latency"), dict) or not isinstance(
                    price["latency"].get("class"), str
                ):
                    raise ValueError(f"catalog feature {name} needs a latency class")
                if not isinstance(evidence, dict) or not evidence.get("level"):
                    raise ValueError(f"catalog feature {name} needs evidence")
                if evidence.get("level") == "linked-embedded-ab":
                    measured = evidence.get("measured")
                    if not isinstance(measured, dict) or set(measured) != {
                        "off", "on", "delta"
                    }:
                        raise ValueError(f"catalog feature {name} needs linked A/B evidence")
                    measurement_fields = {
                        "flash_bytes", "static_ram_bytes", "total_ram_bytes"
                    }
                    if any(
                        not isinstance(measured.get(side), dict)
                        or set(measured[side]) != measurement_fields
                        or any(
                            isinstance(value, bool)
                            or not isinstance(value, int)
                            or value < 0
                            for value in measured[side].values()
                        )
                        for side in ("off", "on", "delta")
                    ):
                        raise ValueError(
                            f"catalog feature {name} has invalid linked measurements"
                        )
                    if any(
                        measured["on"][field] - measured["off"][field]
                        != measured["delta"][field]
                        for field in measurement_fields
                    ):
                        raise ValueError(
                            f"catalog feature {name} linked deltas do not reconcile"
                        )
            elif entry.get("status") == "unavailable":
                if entry.get("price") is not None or not entry.get("reason"):
                    raise ValueError(f"unavailable feature {name} must remain unpriced")
            else:
                raise ValueError(f"catalog feature {name} has an invalid status")
    return catalog


def selected_features(
    workload: dict, catalog: dict | None = None
) -> list[tuple[str, dict]]:
    target = workload.get("target", "nrf52840-nosd")
    targets = (feature_catalog() if catalog is None else catalog).get("targets", {})
    if target not in targets:
        raise WorkloadDiagnostic("feature-target", f"`{target}` has no catalog entry.")
    configured = workload.get("features", {})
    if not isinstance(configured, dict):
        raise WorkloadDiagnostic(
            "feature-shape", "Use `features: {\"capacity-report\": true}`."
        )
    entries = targets[target]
    selected = []
    for name, value in configured.items():
        if name not in entries:
            raise WorkloadDiagnostic(
                "feature-name", f"`{name}` is not a catalog feature for `{target}`."
            )
        entry = entries[name]
        if entry.get("value_type") != "boolean" or not isinstance(value, bool):
            raise WorkloadDiagnostic("feature-value", f"`{name}` expects true or false.")
        if not value:
            continue
        if entry.get("status") != "selectable" or entry.get("price") is None:
            raise WorkloadDiagnostic(
                "feature-unavailable",
                f"`{name}`: {entry.get('reason', 'no verified price is available')}",
            )
        selected.append((name, entry))
    enabled = {name for name, _ in selected}
    for name, entry in selected:
        conflict = next((item for item in entry.get("conflicts", []) if item in enabled), None)
        if conflict is not None:
            raise WorkloadDiagnostic(
                "feature-conflict", f"`{name}` cannot be combined with `{conflict}`."
            )
    return selected


def priced_workload(
    workload: dict, catalog: dict | None = None
) -> tuple[dict, dict]:
    priced = copy.deepcopy(workload)
    selected = selected_features(priced, catalog)
    totals = {"flash": 0, "static_ram": 0, "total_ram": 0}
    for _, entry in selected:
        price = entry["price"]
        totals["flash"] += int(price["flash_delta_bytes_max"])
        totals["static_ram"] += int(price["static_ram_delta_bytes_max"])
        totals["total_ram"] += int(price["total_ram_delta_bytes_max"])
    kernel = next(task for task in priced["tasks"] if task.get("name") == "kernel")
    kernel["flash"] = int(kernel.get("flash", 0)) + totals["flash"]
    kernel["ram"] = int(kernel.get("ram", 0)) + totals["total_ram"]
    return priced, totals


def cargo_kernel_features(workload: dict) -> list[str]:
    return sorted({
        feature
        for _, entry in selected_features(workload)
        for feature in entry.get("kernel_features", [])
    })

GRAPH_SKELETON = '''\
// Generated by `nobro project new` - a graph-declared starting point.
// Build the contract once; the kernel derives manifest, startup, and tasks.
use nobro_kernel::{AppGraph, TaskDecl};

pub fn app() -> AppGraph<2> {
    AppGraph::<2>::new()
        .task(TaskDecl::control("control", 20_000)).unwrap()
        .task(TaskDecl::periodic("sensor", 100_000)).unwrap()
        .channel("sensor", "control").unwrap()
}
'''

RUST_CRITICALITY = {
    "best_effort": "BestEffort",
    "user": "User",
    "driver": "Driver",
    "system": "System",
    "hard_realtime": "HardRealtime",
}


def render_host_main(workload: dict) -> str:
    """Render the build input from the same workload that explain/simulate price."""
    startup_order(workload)
    tasks = [task for task in workload["tasks"] if task["name"] != "kernel"]
    chain = f"AppGraph::<{len(tasks)}>::new()\n"
    for task in tasks:
        name = json.dumps(task["name"])
        period = int(task.get("period_us", 0))
        if period <= 0:
            raise ValueError(f"task {task['name']}: build requires a positive period_us")
        criticality = task.get("criticality", "best_effort")
        constructor = {
            "hard_realtime": "control",
            "best_effort": "service",
        }.get(criticality, "periodic")
        expression = f"TaskDecl::{constructor}({name}, {period})"
        expression += f".criticality(Criticality::{RUST_CRITICALITY[criticality]})"
        if int(task.get("budget_us", 0)):
            expression += f".budget_us({int(task['budget_us'])})"
        if int(task.get("blocking_us", 0)):
            expression += f".blocking_us({int(task['blocking_us'])})"
        for dependency in task.get("after", []):
            expression += f".after({json.dumps(dependency)})"
        chain += f"        .task({expression}).unwrap()\n"
    for channel in workload.get("channels", []):
        chain += (f"        .channel({json.dumps(channel[0])}, "
                  f"{json.dumps(channel[1])}).unwrap()\n")
    chain += (f"        .build_for::<{len(tasks) + 1}>"
              "(SystemProfile::NRF52840_CORE).unwrap()")
    enabled = {name for name, _ in selected_features(workload)}
    feature_import = ""
    feature_marker = ""
    feature_body = ""
    if "capacity-report" in enabled:
        feature_import = "use nobro_kernel::CapacityRegistry;\n"
        feature_marker = (
            "\n#[no_mangle]\n#[used]\n"
            "pub static NOBRO_FEATURE_CAPACITY_REPORT: u8 = 1;\n"
        )
        feature_body = (
            "    let capacity = CapacityRegistry::<1>::new();\n"
            "    std::hint::black_box(capacity.len());\n"
        )
    return (
        "// Generated from workload.json by `nobro project build`; edit the workload.\n"
        "use nobro_kernel::{AppGraph, Criticality, SystemProfile, TaskDecl};\n\n"
        f"{feature_import}{feature_marker}\n"
        "fn main() {\n"
        f"    let built = {chain};\n"
        f"{feature_body}"
        "    println!(\"NOBRO_PROJECT tasks={} startup={} admitted=1\",\n"
        "             built.task_len, built.startup_len);\n"
        "}\n"
    )


def host_target() -> str:
    output = subprocess.check_output(["rustc", "-vV"], text=True)
    return next(line.split(":", 1)[1].strip()
                for line in output.splitlines() if line.startswith("host:"))


def checked_project(name: str, out_dir: pathlib.Path) -> pathlib.Path:
    if not NAME.fullmatch(name):
        raise ValueError("project name must match [a-z][a-z0-9_-]{0,47}")
    return out_dir.resolve() / name


def render_cargo(project: pathlib.Path, name: str, workload: dict) -> str:
    kernel_source = ROOT / "core" / "crates" / "nobro_kernel"
    try:
        kernel_path = os.path.relpath(kernel_source, project)
    except ValueError:
        kernel_path = str(kernel_source)
    features = cargo_kernel_features(workload)
    feature_clause = f", features = {json.dumps(features)}" if features else ""
    return (
        "[package]\n"
        f"name = \"nobro-project-{name.replace('_', '-')}\"\n"
        "version = \"0.1.0\"\n"
        "edition = \"2021\"\n"
        "publish = false\n\n"
        "[dependencies]\n"
        f"nobro-kernel = {{ path = {json.dumps(kernel_path)}{feature_clause} }}\n"
        "critical-section = { version = \"1.2\", features = [\"std\"] }\n\n"
        "[workspace]\n"
    )


def scaffold(name: str, out_dir: pathlib.Path) -> dict:
    project = checked_project(name, out_dir)
    (project / "src").mkdir(parents=True, exist_ok=True)
    workload = copy.deepcopy(WORKLOAD_TEMPLATE)
    workload["app"] = name
    (project / "workload.json").write_text(
        json.dumps(workload, indent=2) + "\n", encoding="utf-8")
    (project / "app_graph.rs").write_text(GRAPH_SKELETON, encoding="utf-8")
    cargo = render_cargo(project, name, workload)
    (project / "Cargo.toml").write_text(cargo, encoding="utf-8", newline="\n")
    (project / "src" / "main.rs").write_text(
        render_host_main(workload), encoding="utf-8")
    (project / "README.txt").write_text(
        f"NobroRTOS project '{name}'\n\n"
        f"Explain the derived contract:\n"
        f"  python tools/nobro_project.py explain {project / 'workload.json'}\n\n"
        f"The graph in app_graph.rs derives the whole kernel contract from one\n"
        f"declaration; workload.json prices it for admission.\n",
        encoding="utf-8")
    return {
        "name": name,
        "dir": str(project),
        "files": ["workload.json", "app_graph.rs", "Cargo.toml", "src/main.rs",
                  "README.txt"],
    }


def build_project(project: pathlib.Path) -> dict:
    manifest = project / "Cargo.toml"
    if not manifest.is_file():
        raise ValueError("project has no Cargo.toml; run `nobro project new` first")
    workload = json.loads((project / "workload.json").read_text(encoding="utf-8"))
    (project / "Cargo.toml").write_text(
        render_cargo(project, project.name, workload), encoding="utf-8", newline="\n"
    )
    (project / "src" / "main.rs").write_text(
        render_host_main(workload), encoding="utf-8")
    lockfile = project / "Cargo.lock"
    lock_created = not lockfile.is_file()
    if lock_created:
        # A fresh standalone project cannot use --locked until this persistent
        # lockfile exists. Resolution happens once; every actual build is locked.
        lock_command = ["cargo", "generate-lockfile", "--manifest-path", str(manifest)]
        resolved = subprocess.run(
            lock_command, cwd=ROOT, capture_output=True, text=True
        )
        if resolved.returncode:
            return {
                "ok": False,
                "command": lock_command,
                "detail": (resolved.stdout + resolved.stderr).splitlines()[-5:],
                "lock_created": False,
            }
    command = ["cargo", "build", "--locked", "--manifest-path", str(manifest),
               "--target", host_target()]
    completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    return {
        "ok": completed.returncode == 0,
        "command": command,
        "detail": (completed.stdout + completed.stderr).splitlines()[-5:],
        "lock_created": lock_created,
    }


def startup_order(workload: dict) -> list[str]:
    schema = workload.get("schema", "nobro-workload-v1")
    if schema != "nobro-workload-v1":
        raise WorkloadDiagnostic(
            "workload-schema",
            f"`{schema}` is unsupported; use `nobro-workload-v1`.",
        )
    selected_features(workload)
    tasks = workload.get("tasks")
    if not isinstance(tasks, list) or not tasks:
        raise WorkloadDiagnostic("shape", "Add `tasks: [...]` with at least `kernel`.")
    names = [task.get("name") for task in tasks]
    if any(not isinstance(name, str) or not name for name in names):
        raise WorkloadDiagnostic(
            "task-name", "Use names like `control`, `imu`, or `telemetry`."
        )
    if len(set(names)) != len(names):
        duplicate = next(name for name in names if names.count(name) > 1)
        raise WorkloadDiagnostic(
            "duplicate-task", f"`{duplicate}` appears more than once.", task=duplicate
        )
    if any(not NAME.fullmatch(name) for name in names):
        invalid = next(name for name in names if not NAME.fullmatch(name))
        raise WorkloadDiagnostic(
            "task-name",
            "Use `[a-z][a-z0-9_-]{0,47}` so diagnostics and generated code stay stable.",
            task=invalid,
        )
    if "kernel" not in names:
        raise WorkloadDiagnostic("missing-kernel", "Add a `kernel` task entry.")
    for task in tasks:
        criticality = task.get("criticality", "best_effort")
        if criticality not in adm.CRITICALITY:
            raise WorkloadDiagnostic(
                "criticality",
                f"`{criticality}` is unknown; choose one of {', '.join(adm.CRITICALITY)}.",
                task=task["name"],
            )
        for field in ("flash", "ram", "pool", "period_us", "budget_us", "blocking_us"):
            if int(task.get(field, 0)) < 0:
                raise WorkloadDiagnostic(
                    "nonnegative", f"`{field}` must be zero or positive.", task=task["name"]
                )
        if int(task.get("budget_us", 0)) > int(task.get("period_us", 0)):
            raise WorkloadDiagnostic(
                "budget-period", "`budget_us` exceeds `period_us`.", task=task["name"]
            )
        if (int(task.get("budget_us", 0)) + int(task.get("blocking_us", 0))
                > int(task.get("period_us", 0))):
            raise WorkloadDiagnostic(
                "budget-period",
                "`budget_us + blocking_us` exceeds `period_us`.",
                task=task["name"],
            )
        after = task.get("after", [])
        if not isinstance(after, list) or len(after) > 4 or len(set(after)) != len(after):
            raise WorkloadDiagnostic(
                "after-list",
                "`after` must contain up to four unique task names.",
                task=task["name"],
            )
    known = set(names)
    dependencies = {name: set() for name in names}
    for task in tasks:
        for dependency in task.get("after", []):
            if dependency not in known:
                raise WorkloadDiagnostic(
                    "dependency",
                    f"`after` references unknown task `{dependency}`.",
                    task=task["name"],
                )
            dependencies[task["name"]].add(dependency)
    if dependencies["kernel"]:
        raise WorkloadDiagnostic(
            "kernel-after",
            "Remove `after` from `kernel`; application tasks can depend on each other instead.",
            task="kernel",
        )
    for channel in workload.get("channels", []):
        if not isinstance(channel, list) or len(channel) != 2:
            raise WorkloadDiagnostic(
                "wire-shape", "Write each wire/channel as `[producer, consumer]`."
            )
        for endpoint in channel:
            if endpoint not in known:
                raise WorkloadDiagnostic(
                    "wire-endpoint",
                    f"Wire endpoint `{endpoint}` is not a declared task.",
                    task=endpoint,
                )
    order = []
    remaining = set(names)
    while remaining:
        ready = [name for name in names if name in remaining
                 and not (dependencies[name] & remaining)]
        if not ready:
            cycle = next(name for name in names if name in remaining)
            raise WorkloadDiagnostic(
                "cycle", "Break the cycle in this task's `after` list.", task=cycle
            )
        for name in ready:
            order.append(name)
            remaining.remove(name)
    # The kernel is always the first admitted node. Its dependency set is rejected
    # above, so moving it to the front preserves every valid application edge.
    return ["kernel"] + [name for name in order if name != "kernel"]


def simulate(project: pathlib.Path) -> tuple[pathlib.Path, dict]:
    workload_path = project / "workload.json"
    workload = json.loads(workload_path.read_text(encoding="utf-8"))
    startup_order(workload)
    priced, _ = priced_workload(workload)
    analysis = adm.analyze(priced)
    feasible = analysis["schedulable"] or analysis["shed_plan"].get("feasible", False)
    report = {
        "schema": "nobro-project-report-v1",
        "mode": "simulation",
        "completed": True,
        "all_pass": feasible,
        "task_count": len(workload["tasks"]),
        "ticks": 100 if feasible else 0,
        "shed": analysis["shed_plan"].get("shed", []),
        "used": analysis["used"],
    }
    path = project / "reports" / "simulation.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return path, report


def read_report(path: pathlib.Path) -> tuple[str, bool]:
    record = json.loads(path.read_text(encoding="utf-8"))
    if record.get("schema") == "nobro-project-report-v1":
        ok = bool(record.get("completed") and record.get("all_pass"))
        return (f"PROJECT REPORT: {'PASS' if ok else 'FAIL'} "
                f"mode={record.get('mode')} tasks={record.get('task_count')} "
                f"ticks={record.get('ticks')} shed={len(record.get('shed', []))}"), ok
    if "ok" in record and "app" in record:
        ok = bool(record.get("ok"))
        return (f"HARDWARE REPORT: {'PASS' if ok else 'FAIL'} "
                f"app={record.get('app')} completed={record.get('fields', {}).get('completed', 0)}"), ok
    raise ValueError("unknown report schema")


def shrink_report(
    report: pathlib.Path | None,
    output: pathlib.Path | None = None,
    *,
    bindings: bool = False,
    device_report: pathlib.Path | None = None,
    campaign: pathlib.Path | None = None,
    workload: pathlib.Path | None = None,
    build_manifest: pathlib.Path | None = None,
) -> int:
    """Dispatch analysis, binding, and device-report decoding to one engine."""
    inputs = (campaign, workload, build_manifest)
    if bindings or device_report is not None:
        if report is not None or any(path is None for path in inputs):
            raise ValueError(
                "binding/device-report modes require campaign, workload, and build manifest"
            )
        if bindings and device_report is not None:
            raise ValueError("choose either bindings or device report")
        if bindings:
            return shrink.run_bindings(campaign, workload, build_manifest, output)
        return shrink.run_device_report(
            device_report, campaign, workload, build_manifest, output
        )
    if any(path is not None for path in inputs) or report is None:
        raise ValueError("an occupancy report is required")
    return shrink.run(report, output)


# ------------------------------------------------------------------- explain

def explain(workload: dict) -> tuple[str, bool]:
    """Plain-language account of the derived contract + admission verdict."""
    order = startup_order(workload)
    priced, feature_totals = priced_workload(workload)
    result = adm.analyze(priced)
    tasks = [t for t in workload["tasks"] if t["name"] != "kernel"]
    lines = [f"This system has {len(tasks)} task(s) plus the kernel."]
    selected = selected_features(workload)
    if selected:
        lines.append("Enabled optional features (catalog ceiling for this composition):")
        for name, entry in selected:
            price = entry["price"]
            latency = price["latency"]
            lines.append(
                f"  {name}: <= {price['flash_delta_bytes_max']} B flash, "
                f"<= {price['static_ram_delta_bytes_max']} B static RAM, "
                f"<= {price['total_ram_delta_bytes_max']} B total RAM; "
                f"latency={latency['class']}; evidence={entry['evidence']['level']}."
            )
        lines.append(
            f"Additive feature reserve total: <= {feature_totals['flash']} B flash, "
            f"<= {feature_totals['static_ram']} B static RAM, "
            f"<= {feature_totals['total_ram']} B total RAM."
        )
    else:
        lines.append("Optional features: none enabled (zero feature reserve).")

    # Derived capabilities: any task that shares a channel needs Mailbox; the
    # explain mirrors the graph's derivation rule.
    channels = workload.get("channels", [])
    if channels:
        endpoints = sorted({e for ch in channels for e in ch})
        lines.append(f"Wires/channels are declared, so {', '.join(endpoints)} each get the "
                     f"Mailbox capability (the kernel owns it) - derived, not hand-written.")

    lines.append(f"Startup order: {' -> '.join(order)}.")

    # Marginal cost table + schedulability.
    lines.append("")
    lines.append(adm.render(result))
    lines.append("")
    if result["schedulable"]:
        flash_free = result["headroom"]["flash"]
        ram_free = result["headroom"]["ram"]
        util_free = result["headroom"]["util"] / 100
        lines.append(f"VERDICT: this admits. Headroom: {flash_free} B flash, "
                     f"{ram_free} B RAM, {util_free:.1f}% CPU. You can add more "
                     f"best-effort features until any of those reaches zero.")
        return "\n".join(lines), True

    plan = result["shed_plan"]
    if plan.get("feasible"):
        lines.append(f"VERDICT: over budget. The kernel would drop these best-effort "
                     f"tasks first (never your deadline-critical work): "
                     f"{', '.join(plan['shed'])}. Either accept that, cut a budget, "
                     f"or move to a bigger profile.")
        return "\n".join(lines), True
    lines.append("VERDICT: INFEASIBLE. Even after shedding every best-effort task the "
                 "deadline-critical core does not fit. Cut task budgets or choose a "
                 "bigger board profile; safety-critical work is never dropped.")
    return "\n".join(lines), False


# ------------------------------------------------------------------- selftest

def selftest() -> int:
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        out = pathlib.Path(tmp)
        assert not (ADMISSION_DIAGNOSTIC_CODES &
                    {code for code, _ in WORKLOAD_DIAGNOSTICS.values()})
        # Golden session 1 - the SIMPLE path: scaffold -> explain -> admits.
        info = scaffold("blinky", out)
        assert {"workload.json", "app_graph.rs", "Cargo.toml", "src/main.rs"}.issubset(
            info["files"]
        )
        workload = json.loads((out / "blinky" / "workload.json").read_text(encoding="utf-8"))
        text, ok = explain(workload)
        assert ok and "VERDICT: this admits" in text, text
        built = build_project(out / "blinky")
        assert built["ok"], "\n".join(built["detail"])
        assert built["lock_created"] and "--locked" in built["command"]
        lockfile = out / "blinky" / "Cargo.lock"
        assert lockfile.is_file()
        locked_graph = lockfile.read_bytes()
        workload["tasks"].append(
            {"name": "telemetry", "criticality": "best_effort", "flash": 512,
             "ram": 128, "period_us": 250_000, "budget_us": 1_000,
             "after": ["sensor"]}
        )
        (out / "blinky" / "workload.json").write_text(
            json.dumps(workload, indent=2) + "\n", encoding="utf-8")
        rebuilt = build_project(out / "blinky")
        assert rebuilt["ok"], "\n".join(rebuilt["detail"])
        assert not rebuilt["lock_created"] and "--locked" in rebuilt["command"]
        assert lockfile.read_bytes() == locked_graph, "a locked rebuild changed Cargo.lock"
        generated = (out / "blinky" / "src" / "main.rs").read_text(encoding="utf-8")
        assert 'TaskDecl::service("telemetry"' in generated and '.after("sensor")' in generated

        # One feature object drives validation, pricing, Cargo features, and source.
        workload["features"] = {"capacity-report": True}
        text, ok = explain(workload)
        assert ok and "Additive feature reserve total: <= 384 B flash" in text, text
        (out / "blinky" / "workload.json").write_text(
            json.dumps(workload, indent=2) + "\n", encoding="utf-8")
        feature_build = build_project(out / "blinky")
        assert feature_build["ok"], "\n".join(feature_build["detail"])
        generated = (out / "blinky" / "src" / "main.rs").read_text(encoding="utf-8")
        cargo = (out / "blinky" / "Cargo.toml").read_text(encoding="utf-8")
        assert "NOBRO_FEATURE_CAPACITY_REPORT" in generated
        assert 'features = ["capacity-report"]' in cargo
        workload["features"] = {}
        assert "NOBRO_FEATURE_CAPACITY_REPORT" not in render_host_main(workload)
        cargo_without_feature = render_cargo(out / "blinky", "blinky", workload)
        assert 'nobro-kernel = { path =' in cargo_without_feature
        assert 'features = ["capacity-report"]' not in cargo_without_feature

        invalid_feature = json.loads(json.dumps(WORKLOAD_TEMPLATE))
        invalid_feature["features"] = {"missing": True}
        try:
            selected_features(invalid_feature)
            raise AssertionError("an unknown feature must be rejected")
        except WorkloadDiagnostic as error:
            assert error.code == "NOBRO-E045", error

        invalid_feature["features"] = {"capacity-report": "yes"}
        try:
            selected_features(invalid_feature)
            raise AssertionError("a non-boolean feature must be rejected")
        except WorkloadDiagnostic as error:
            assert error.code == "NOBRO-E046", error

        invalid_feature["features"] = {"preemptive": True}
        try:
            selected_features(invalid_feature)
            raise AssertionError("an unpriced feature must be unavailable")
        except WorkloadDiagnostic as error:
            assert error.code == "NOBRO-E047", error

        invalid_feature["target"] = "missing-target"
        invalid_feature["features"] = {}
        try:
            selected_features(invalid_feature)
            raise AssertionError("an unsupported target must be rejected")
        except WorkloadDiagnostic as error:
            assert error.code == "NOBRO-E044", error

        invalid_schema = json.loads(json.dumps(WORKLOAD_TEMPLATE))
        invalid_schema["schema"] = "nobro-workload-v999"
        try:
            startup_order(invalid_schema)
            raise AssertionError("an unsupported workload schema must be rejected")
        except WorkloadDiagnostic as error:
            assert error.code == "NOBRO-E049", error

        base_entry = copy.deepcopy(
            feature_catalog()["targets"]["nrf52840-nosd"]["capacity-report"]
        )
        second_entry = copy.deepcopy(base_entry)
        conflict_entry = copy.deepcopy(base_entry)
        conflict_entry["conflicts"] = ["second"]
        synthetic = {
            "targets": {
                "nrf52840-nosd": {
                    "first": conflict_entry,
                    "second": second_entry,
                }
            }
        }
        pair = json.loads(json.dumps(WORKLOAD_TEMPLATE))
        pair["features"] = {"first": True, "second": True}
        try:
            selected_features(pair, synthetic)
            raise AssertionError("a catalog conflict must be rejected")
        except WorkloadDiagnostic as error:
            assert error.code == "NOBRO-E048", error
        synthetic["targets"]["nrf52840-nosd"]["first"]["conflicts"] = []
        _, aggregate = priced_workload(pair, synthetic)
        assert aggregate == {"flash": 768, "static_ram": 32, "total_ram": 64}

        overflow = json.loads(json.dumps(WORKLOAD_TEMPLATE))
        overflow["features"] = {"capacity-report": True}
        overflow["profile"]["flash"] = sum(
            int(task["flash"])
            for task in overflow["tasks"]
            if task["criticality"] != "driver"
        )
        _, ok = explain(overflow)
        assert not ok, "feature reserve must participate in admission overflow"
        report_path, report = simulate(out / "blinky")
        assert report["all_pass"] and report_path.is_file()
        rendered, ok = read_report(report_path)
        assert ok and "PROJECT REPORT: PASS" in rendered

        # Golden session 2 - the COMPLEX path: the robotics workload over a tight
        # RAM profile explains, sheds best-effort first, and stays feasible.
        complex_wl = adm.robotics_workload()  # tight 8KB RAM -> over budget
        text, ok = explain(complex_wl)
        assert ok, "should be rescuable by shedding"
        assert "drop these best-effort tasks first" in text
        assert "camera_ai" in text and "motor" not in text.split("VERDICT")[1]

        # Golden session 3 - INFEASIBLE path exits non-ok.
        tiny = adm.robotics_workload()
        tiny["profile"] = {"flash": 4 * 1024, "ram": 1024, "pool": 1}
        _, ok = explain(tiny)
        assert not ok, "critical-core overflow must be infeasible"

        invalid = json.loads(json.dumps(WORKLOAD_TEMPLATE))
        invalid["tasks"][0]["after"] = ["sensor"]
        try:
            startup_order(invalid)
            raise AssertionError("a kernel dependency must be rejected")
        except WorkloadDiagnostic as error:
            assert error.code == "NOBRO-E039", error
            assert "kernel starts first" in error.user_message()

        invalid = json.loads(json.dumps(WORKLOAD_TEMPLATE))
        invalid["channels"].append(["sensor", "missing"])
        try:
            startup_order(invalid)
            raise AssertionError("an unknown wire endpoint must be rejected")
        except WorkloadDiagnostic as error:
            assert error.code == "NOBRO-E041", error
            assert "Wire endpoint `missing`" in error.user_message()

    print("NOBRO PROJECT SELFTEST: PASS (scaffold/build/simulate/report/diagnostics)")
    return 0


# ------------------------------------------------------------------- main

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--selftest", action="store_true")
    sub = parser.add_subparsers(dest="command")

    p_new = sub.add_parser("new", help="scaffold a project")
    p_new.add_argument("name")
    p_new.add_argument("--out", type=pathlib.Path, default=DEFAULT_OUT)

    p_explain = sub.add_parser("explain", help="explain a workload's derived contract")
    p_explain.add_argument("workload", type=pathlib.Path)

    p_build = sub.add_parser("build", help="compile the generated graph scaffold")
    p_build.add_argument("project", type=pathlib.Path)

    p_sim = sub.add_parser("simulate", help="run bounded host admission simulation")
    p_sim.add_argument("project", type=pathlib.Path)

    p_report = sub.add_parser("report", help="read a project report")
    p_report.add_argument("report", type=pathlib.Path)

    p_shrink = sub.add_parser(
        "shrink", help="propose capacity changes from an occupancy report"
    )
    p_shrink.add_argument("report", nargs="?", type=pathlib.Path)
    p_shrink.add_argument(
        "--json", type=pathlib.Path, metavar="FILE", help="write proposal JSON"
    )
    p_shrink.add_argument(
        "--bindings", action="store_true", help="derive firmware campaign identities"
    )
    p_shrink.add_argument(
        "--device-report",
        type=pathlib.Path,
        metavar="REPORT.BIN",
        help="decode a report captured from firmware",
    )
    p_shrink.add_argument("--campaign", type=pathlib.Path, metavar="FILE")
    p_shrink.add_argument("--workload", type=pathlib.Path, metavar="FILE")
    p_shrink.add_argument("--build-manifest", type=pathlib.Path, metavar="FILE")

    p_run = sub.add_parser("run", help="explain, build, then simulate")
    p_run.add_argument("project", type=pathlib.Path)

    args = parser.parse_args()
    if args.selftest:
        return selftest()

    if args.command == "new":
        try:
            info = scaffold(args.name, args.out)
        except ValueError as error:
            print(f"cannot create project: {format_user_error(error)}")
            return 1
        print(f"created project '{info['name']}' in {info['dir']}")
        print(f"  files: {', '.join(info['files'])}")
        print(f"  next:  python tools/nobro_project.py explain "
              f"{pathlib.Path(info['dir']) / 'workload.json'}")
        return 0

    if args.command == "explain":
        try:
            workload = json.loads(args.workload.read_text(encoding="utf-8"))
            text, ok = explain(workload)
        except (OSError, ValueError, KeyError, TypeError) as error:
            print(f"cannot explain workload: {format_user_error(error)}")
            return 1
        print(text)
        return 0 if ok else 1

    if args.command == "build":
        try:
            result = build_project(args.project.resolve())
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            print(f"PROJECT BUILD: FAIL ({format_user_error(error)})")
            return 1
        print(f"PROJECT BUILD: {'PASS' if result['ok'] else 'FAIL'}")
        if not result["ok"]:
            print("\n".join(result["detail"]))
        return 0 if result["ok"] else 1

    if args.command == "simulate":
        try:
            path, record = simulate(args.project.resolve())
            rendered, ok = read_report(path)
        except (OSError, ValueError, KeyError, TypeError) as error:
            print(f"PROJECT SIMULATION: FAIL ({format_user_error(error)})")
            return 1
        print(rendered)
        print(f"report: {path}")
        return 0 if ok and record["all_pass"] else 1

    if args.command == "report":
        try:
            rendered, ok = read_report(args.report)
        except (OSError, ValueError, TypeError) as error:
            print(f"REPORT: FAIL ({format_user_error(error)})")
            return 1
        print(rendered)
        return 0 if ok else 1

    if args.command == "shrink":
        try:
            return shrink_report(
                args.report,
                args.json,
                bindings=args.bindings,
                device_report=args.device_report,
                campaign=args.campaign,
                workload=args.workload,
                build_manifest=args.build_manifest,
            )
        except ValueError as error:
            print(f"PROJECT SHRINK: FAIL ({format_user_error(error)})")
            return 1

    if args.command == "run":
        project = args.project.resolve()
        try:
            workload = json.loads((project / "workload.json").read_text(encoding="utf-8"))
            explanation, feasible = explain(workload)
            print(explanation)
            if not feasible:
                return 1
            built = build_project(project)
        except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
            print(f"PROJECT RUN: FAIL ({format_user_error(error)})")
            return 1
        print(f"PROJECT BUILD: {'PASS' if built['ok'] else 'FAIL'}")
        if not built["ok"]:
            print("\n".join(built["detail"]))
            return 1
        path, _ = simulate(project)
        rendered, ok = read_report(path)
        print(rendered)
        print(f"PROJECT RUN: {'PASS' if ok else 'FAIL'}")
        return 0 if ok else 1

    parser.print_help()
    return 2


if __name__ == "__main__":
    sys.exit(main())
