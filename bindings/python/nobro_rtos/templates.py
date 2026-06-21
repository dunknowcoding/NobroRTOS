"""Project template builders for NobroRTOS host tooling."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
import json
from pathlib import Path, PurePosixPath
import re

from .contracts import (
    Capability,
    Criticality,
    MemoryBudget,
    ModuleSpec,
    NobroContractBundle,
)


class ProjectTarget(str, Enum):
    """Supported starter project shapes."""

    STANDALONE_SDK = "standalone_sdk"
    ARDUINO = "arduino"
    PLATFORMIO = "platformio"
    PYTHON_HOST = "python_host"
    PYTHON_BOARD_BRIDGE = "python_board_bridge"


@dataclass(frozen=True)
class TemplateFile:
    """A generated file path and content pair."""

    path: str
    content: str

    @property
    def size_bytes(self) -> int:
        return len(self.content.encode("utf-8"))

    def to_dict(self) -> dict[str, int | str]:
        return {
            "path": self.path,
            "size_bytes": self.size_bytes,
            "content": self.content,
        }


@dataclass(frozen=True)
class ProjectTemplate:
    """A complete in-memory starter project template."""

    name: str
    target: ProjectTarget
    files: tuple[TemplateFile, ...]

    def to_dict(self) -> dict[str, object]:
        return {
            "name": self.name,
            "target": self.target.value,
            "file_count": len(self.files),
            "files": [file.to_dict() for file in self.files],
        }

    def file_map(self) -> dict[str, str]:
        return {file.path: file.content for file in self.files}


@dataclass(frozen=True)
class ProjectMaterializationReport:
    """Result of safely writing a starter template to disk."""

    root: str
    target: ProjectTarget
    written: tuple[str, ...]
    overwritten: tuple[str, ...] = ()

    def to_dict(self) -> dict[str, object]:
        return {
            "root": self.root,
            "target": self.target.value,
            "written_count": len(self.written),
            "overwritten_count": len(self.overwritten),
            "written": list(self.written),
            "overwritten": list(self.overwritten),
        }


@dataclass(frozen=True)
class ProjectValidationReport:
    """Structured validation result for a generated starter project."""

    root: str
    target: ProjectTarget | None
    files: tuple[str, ...]
    module_count: int
    errors: tuple[str, ...] = ()

    @property
    def passing(self) -> bool:
        return len(self.errors) == 0

    def to_dict(self) -> dict[str, object]:
        return {
            "root": self.root,
            "target": None if self.target is None else self.target.value,
            "passing": self.passing,
            "file_count": len(self.files),
            "module_count": self.module_count,
            "files": list(self.files),
            "errors": list(self.errors),
        }


@dataclass(frozen=True)
class ProjectRepairReport:
    """Structured result for conservative starter project self-repair."""

    root: str
    target: ProjectTarget | None
    repaired: tuple[str, ...]
    before_errors: tuple[str, ...]
    after_errors: tuple[str, ...]

    @property
    def passing(self) -> bool:
        return len(self.after_errors) == 0

    def to_dict(self) -> dict[str, object]:
        return {
            "root": self.root,
            "target": None if self.target is None else self.target.value,
            "passing": self.passing,
            "repaired": list(self.repaired),
            "before_errors": list(self.before_errors),
            "after_errors": list(self.after_errors),
        }


def build_project_template(
    name: str = "nobro_edge_app",
    target: str | ProjectTarget = ProjectTarget.PLATFORMIO,
    module_name: str = "app",
    author: str = "dunknowcoding",
) -> ProjectTemplate:
    """Build a deterministic starter template without touching the filesystem."""

    _validate_identifier(name, "project name")
    _validate_identifier(module_name, "module name")
    target = ProjectTarget(target)

    if target == ProjectTarget.STANDALONE_SDK:
        files = _standalone_sdk_files(name, module_name, author)
    elif target == ProjectTarget.ARDUINO:
        files = _arduino_files(name, module_name, author)
    elif target == ProjectTarget.PLATFORMIO:
        files = _platformio_files(name, module_name, author)
    elif target == ProjectTarget.PYTHON_HOST:
        files = _python_host_files(name, module_name, author)
    else:
        files = _python_board_bridge_files(name, module_name, author)

    return ProjectTemplate(name=name, target=target, files=files)


def materialize_project_template(
    template: ProjectTemplate,
    output_dir: str | Path,
    overwrite: bool = False,
) -> ProjectMaterializationReport:
    """Safely write a generated template into an output directory."""

    root = Path(output_dir).resolve()
    if root.exists() and not root.is_dir():
        raise ValueError(f"template output is not a directory: {root}")

    written: list[str] = []
    overwritten: list[str] = []
    root.mkdir(parents=True, exist_ok=True)

    for template_file in template.files:
        relative = _safe_template_relative_path(template_file.path)
        destination = (root / Path(*relative.parts)).resolve()
        if not _is_relative_to(destination, root):
            raise ValueError(f"template path escapes output directory: {template_file.path}")
        if destination.exists() and not overwrite:
            raise FileExistsError(f"template file already exists: {destination}")
        if destination.exists():
            overwritten.append(template_file.path)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(template_file.content, encoding="utf-8")
        written.append(template_file.path)

    return ProjectMaterializationReport(
        root=str(root),
        target=template.target,
        written=tuple(written),
        overwritten=tuple(overwritten),
    )


def validate_project_template(
    project_dir: str | Path,
    expected_target: str | ProjectTarget | None = None,
) -> ProjectValidationReport:
    """Validate a starter project directory without building or flashing it."""

    root = Path(project_dir).resolve()
    errors: list[str] = []
    files: list[str] = []
    module_count = 0
    target: ProjectTarget | None = None

    if not root.exists() or not root.is_dir():
        return ProjectValidationReport(
            root=str(root),
            target=None,
            files=(),
            module_count=0,
            errors=(f"project directory missing: {root}",),
        )

    for path in sorted(item for item in root.rglob("*") if item.is_file()):
        files.append(path.relative_to(root).as_posix())

    contract_path = root / "nobro-contract.json"
    if not contract_path.exists():
        errors.append("missing nobro-contract.json")
    else:
        try:
            bundle = NobroContractBundle.from_file(contract_path)
            bundle.validate()
            module_count = len(bundle.modules)
            if module_count == 0:
                errors.append("contract has no modules")
        except Exception as error:  # noqa: BLE001 - report validation context.
            errors.append(f"invalid nobro-contract.json: {error}")

    target = _detect_project_target(set(files))
    if target is None:
        errors.append("unable to detect project target")

    if expected_target is not None:
        expected = ProjectTarget(expected_target)
        if target != expected:
            label = None if target is None else target.value
            errors.append(f"target mismatch: expected {expected.value}, found {label}")

    if target is not None:
        errors.extend(_validate_vscode_tasks(root, target))

    return ProjectValidationReport(
        root=str(root),
        target=target,
        files=tuple(files),
        module_count=module_count,
        errors=tuple(errors),
    )


def repair_project_template(
    project_dir: str | Path,
    expected_target: str | ProjectTarget | None = None,
) -> ProjectRepairReport:
    """Repair generated IDE metadata without touching user code or contracts."""

    root = Path(project_dir).resolve()
    before = validate_project_template(root, expected_target=expected_target)
    repaired: list[str] = []

    if before.target is not None and _has_vscode_task_error(before.errors):
        tasks_path = root / ".vscode" / "tasks.json"
        tasks_path.parent.mkdir(parents=True, exist_ok=True)
        tasks_path.write_text(_vscode_tasks_json(before.target), encoding="utf-8")
        repaired.append(".vscode/tasks.json")

    after = validate_project_template(root, expected_target=expected_target)
    return ProjectRepairReport(
        root=str(root),
        target=after.target,
        repaired=tuple(repaired),
        before_errors=before.errors,
        after_errors=after.errors,
    )


def _standalone_sdk_files(
    name: str,
    module_name: str,
    author: str,
) -> tuple[TemplateFile, ...]:
    return (
        TemplateFile("README.md", _readme(name, "Standalone SDK", author)),
        TemplateFile("nobro-contract.json", _contract_json(name, module_name)),
        TemplateFile(".vscode/tasks.json", _vscode_tasks_json(ProjectTarget.STANDALONE_SDK)),
        TemplateFile(
            "src/main.rs",
            "\n".join(
                (
                    "#![no_std]",
                    "",
                    "use nobro_kernel::{Criticality, MemoryBudget, ModuleId, ModuleSpec};",
                    "",
                    "pub fn app_module_spec() -> ModuleSpec {",
                    "    ModuleSpec::new(ModuleId::App(1), Criticality::User)",
                    "        .memory(MemoryBudget::new(8192, 2048, 1))",
                    "}",
                    "",
                )
            ),
        ),
    )


def _arduino_files(
    name: str,
    module_name: str,
    author: str,
) -> tuple[TemplateFile, ...]:
    return (
        TemplateFile("README.md", _readme(name, "Arduino", author)),
        TemplateFile("nobro-contract.json", _contract_json(name, module_name)),
        TemplateFile(".vscode/tasks.json", _vscode_tasks_json(ProjectTarget.ARDUINO)),
        TemplateFile(
            f"{name}.ino",
            "\n".join(
                (
                    "#include <Arduino.h>",
                    "#include <NobroRTOS.h>",
                    "",
                    "void setup() {",
                    "  Serial.begin(115200);",
                    "}",
                    "",
                    "void loop() {",
                    "  delay(1000);",
                    "}",
                    "",
                )
            ),
        ),
    )


def _platformio_files(
    name: str,
    module_name: str,
    author: str,
) -> tuple[TemplateFile, ...]:
    return (
        TemplateFile("README.md", _readme(name, "PlatformIO", author)),
        TemplateFile(
            "platformio.ini",
            "\n".join(
                (
                    "[env:nobro_host_first]",
                    "platform = nordicnrf52",
                    "framework = arduino",
                    "board = nice_nano_v2",
                    "lib_deps = dunknowcoding/NobroRTOS",
                    "",
                )
            ),
        ),
        TemplateFile("nobro-contract.json", _contract_json(name, module_name)),
        TemplateFile(".vscode/tasks.json", _vscode_tasks_json(ProjectTarget.PLATFORMIO)),
        TemplateFile(
            "src/main.cpp",
            "\n".join(
                (
                    "#include <Arduino.h>",
                    "#include <NobroRTOS.h>",
                    "",
                    "void setup() {",
                    "    Serial.begin(115200);",
                    "}",
                    "",
                    "void loop() {",
                    "    delay(1000);",
                    "}",
                    "",
                )
            ),
        ),
    )


def _python_host_files(
    name: str,
    module_name: str,
    author: str,
) -> tuple[TemplateFile, ...]:
    return (
        TemplateFile("README.md", _readme(name, "Python host", author)),
        TemplateFile("nobro-contract.json", _contract_json(name, module_name)),
        TemplateFile(".vscode/tasks.json", _vscode_tasks_json(ProjectTarget.PYTHON_HOST)),
        TemplateFile(
            "tools/runtime_drill.py",
            "\n".join(
                (
                    "from nobro_rtos import (",
                    "    Capability,",
                    "    Criticality,",
                    "    MemoryBudget,",
                    "    ModuleSpec,",
                    "    RuntimeDrillSimulator,",
                    "    SystemProfile,",
                    ")",
                    "",
                    "",
                    "def main() -> None:",
                    "    modules = (",
                    "        ModuleSpec(",
                    f"            \"{module_name}\",",
                    "            Criticality.USER,",
                    "            MemoryBudget(8192, 2048, 1),",
                    "            requires=(Capability.TIMEBASE,),",
                    "        ),",
                    "    )",
                    "    drill = RuntimeDrillSimulator(",
                    "        modules=modules,",
                    "        profile=SystemProfile(72 * 1024, 16 * 1024, 5, 4),",
                    "    )",
                    "    print(drill.run(fault_count=3).to_dict())",
                    "",
                    "",
                    "if __name__ == \"__main__\":",
                    "    main()",
                    "",
                )
            ),
        ),
    )


def _python_board_bridge_files(
    name: str,
    module_name: str,
    author: str,
) -> tuple[TemplateFile, ...]:
    return (
        TemplateFile("README.md", _readme(name, "Python board bridge", author)),
        TemplateFile(
            "nobro-contract.json",
            _contract_json(
                name,
                module_name,
                requires=(Capability.TIMEBASE, Capability.STREAM),
                owns=(Capability.STREAM,),
            ),
        ),
        TemplateFile(
            ".vscode/tasks.json",
            _vscode_tasks_json(ProjectTarget.PYTHON_BOARD_BRIDGE),
        ),
        TemplateFile(
            "board/code.py",
            "\n".join(
                (
                    "\"\"\"MicroPython/CircuitPython-style NobroRTOS board bridge stub.\"\"\"",
                    "",
                    "import sys",
                    "",
                    "try:",
                    "    import time",
                    "except ImportError:",
                    "    time = None",
                    "",
                    f"MODULE = \"{module_name}\"",
                    "PERIOD_S = 1",
                    "RUNTIME = getattr(sys.implementation, \"name\", \"\")",
                    "BOARD_PYTHON = RUNTIME in (\"micropython\", \"circuitpython\")",
                    "",
                    "",
                    "def emit_status(counter):",
                    "    print(",
                    "        \"NOBRO_STATUS module={} counter={} state=ready\".format(",
                    "            MODULE,",
                    "            counter,",
                    "        )",
                    "    )",
                    "",
                    "",
                    "def main():",
                    "    counter = 0",
                    "    while True:",
                    "        emit_status(counter)",
                    "        counter += 1",
                    "        if time is None or not BOARD_PYTHON:",
                    "            break",
                    "        time.sleep(PERIOD_S)",
                    "",
                    "",
                    "main()",
                    "",
                )
            ),
        ),
        TemplateFile(
            "host/bridge_smoke.py",
            "\n".join(
                (
                    "\"\"\"Offline parser smoke test for the board bridge status line.\"\"\"",
                    "",
                    "",
                    "def parse_status(line: str) -> dict[str, str]:",
                    "    parts = line.strip().split()",
                    "    if not parts or parts[0] != \"NOBRO_STATUS\":",
                    "        raise ValueError(\"missing NOBRO_STATUS prefix\")",
                    "    fields: dict[str, str] = {}",
                    "    for item in parts[1:]:",
                    "        key, separator, value = item.partition(\"=\")",
                    "        if not separator:",
                    "            raise ValueError(f\"invalid status field: {item}\")",
                    "        fields[key] = value",
                    "    if fields.get(\"state\") != \"ready\":",
                    "        raise ValueError(\"bridge is not ready\")",
                    "    return fields",
                    "",
                    "",
                    "def main() -> None:",
                    f"    sample = \"NOBRO_STATUS module={module_name} counter=0 state=ready\"",
                    "    print(parse_status(sample))",
                    "",
                    "",
                    "if __name__ == \"__main__\":",
                    "    main()",
                    "",
                )
            ),
        ),
    )


def _readme(name: str, target: str, author: str) -> str:
    return "\n".join(
        (
            f"# {name}",
            "",
            f"Generated NobroRTOS {target} starter template.",
            "",
            f"Author: {author}",
            "",
            "Start by editing `nobro-contract.json`, then keep module budgets,",
            "capabilities, and recovery expectations visible in host tooling.",
            "",
        )
    )


def _contract_json(
    name: str,
    module_name: str,
    requires: tuple[Capability, ...] = (Capability.TIMEBASE,),
    owns: tuple[Capability, ...] = (),
) -> str:
    bundle = NobroContractBundle(
        metadata={"project": name, "template": "starter"},
        modules=(
            ModuleSpec(
                module_name,
                Criticality.USER,
                MemoryBudget(8192, 2048, 1),
                requires=requires,
                owns=owns,
            ),
        ),
    )
    return json.dumps(bundle.to_dict(), indent=2, sort_keys=True) + "\n"


def _vscode_tasks_json(target: ProjectTarget) -> str:
    tasks: list[dict[str, object]] = [
        {
            "label": "NobroRTOS: Check Project",
            "type": "shell",
            "command": "python",
            "args": [
                "-m",
                "nobro_rtos",
                "check-project",
                "${workspaceFolder}",
                "--target",
                target.value,
            ],
            "group": "test",
            "problemMatcher": [],
        }
    ]
    if target == ProjectTarget.PYTHON_HOST:
        tasks.append(
            {
                "label": "NobroRTOS: Runtime Drill",
                "type": "shell",
                "command": "python",
                "args": ["tools/runtime_drill.py"],
                "group": "test",
                "problemMatcher": [],
            }
        )
        tasks.append(
            {
                "label": "NobroRTOS: Runtime Drill Gate",
                "type": "shell",
                "command": "python",
                "args": [
                    "-m",
                    "nobro_rtos",
                    "check-runtime-drill",
                    "--fault-count",
                    "3",
                ],
                "group": "test",
                "problemMatcher": [],
            }
        )
        tasks.append(
            {
                "label": "NobroRTOS: AI Route Gate",
                "type": "shell",
                "command": "python",
                "args": [
                    "-m",
                    "nobro_rtos",
                    "check-ai-route",
                    "--backend",
                    "hybrid",
                    "--require-target",
                    "on_device",
                ],
                "group": "test",
                "problemMatcher": [],
            }
        )
        tasks.append(
            {
                "label": "NobroRTOS: AI Route Matrix Gate",
                "type": "shell",
                "command": "python",
                "args": [
                    "-m",
                    "nobro_rtos",
                    "check-ai-route-matrix",
                ],
                "group": "test",
                "problemMatcher": [],
            }
        )
        tasks.append(
            {
                "label": "NobroRTOS: Recovery Matrix Gate",
                "type": "shell",
                "command": "python",
                "args": [
                    "-m",
                    "nobro_rtos",
                    "check-recovery-matrix",
                ],
                "group": "test",
                "problemMatcher": [],
            }
        )
    if target == ProjectTarget.PYTHON_BOARD_BRIDGE:
        tasks.append(
            {
                "label": "NobroRTOS: Bridge Smoke",
                "type": "shell",
                "command": "python",
                "args": ["host/bridge_smoke.py"],
                "group": "test",
                "problemMatcher": [],
            }
        )

    return json.dumps({"version": "2.0.0", "tasks": tasks}, indent=2) + "\n"


def _validate_identifier(value: str, label: str) -> None:
    if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_-]{0,63}", value):
        raise ValueError(f"invalid {label}: {value}")


def _safe_template_relative_path(path: str) -> PurePosixPath:
    relative = PurePosixPath(path)
    if relative.is_absolute() or not relative.parts:
        raise ValueError(f"invalid template path: {path}")
    if any(part in ("", ".", "..") for part in relative.parts):
        raise ValueError(f"invalid template path: {path}")
    return relative


def _is_relative_to(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def _detect_project_target(files: set[str]) -> ProjectTarget | None:
    if "platformio.ini" in files and "src/main.cpp" in files:
        return ProjectTarget.PLATFORMIO
    if any(path.endswith(".ino") for path in files):
        return ProjectTarget.ARDUINO
    if "src/main.rs" in files:
        return ProjectTarget.STANDALONE_SDK
    if "tools/runtime_drill.py" in files:
        return ProjectTarget.PYTHON_HOST
    if "board/code.py" in files and "host/bridge_smoke.py" in files:
        return ProjectTarget.PYTHON_BOARD_BRIDGE
    return None


def _validate_vscode_tasks(root: Path, target: ProjectTarget) -> list[str]:
    tasks_path = root / ".vscode" / "tasks.json"
    if not tasks_path.exists():
        return ["missing .vscode/tasks.json"]
    try:
        payload = json.loads(tasks_path.read_text(encoding="utf-8"))
    except Exception as error:  # noqa: BLE001 - report validation context.
        return [f"invalid .vscode/tasks.json: {error}"]

    errors: list[str] = []
    tasks = payload.get("tasks")
    if payload.get("version") != "2.0.0":
        errors.append("invalid .vscode/tasks.json version")
    if not isinstance(tasks, list):
        return errors + ["invalid .vscode/tasks.json tasks"]

    check_task = _task_by_label(tasks, "NobroRTOS: Check Project")
    if check_task is None:
        errors.append("missing NobroRTOS: Check Project task")
    elif not _task_has_args(
        check_task,
        ("-m", "nobro_rtos", "check-project", "--target", target.value),
    ):
        errors.append("check task target mismatch")

    runtime_drill_task = _task_by_label(
        tasks,
        "NobroRTOS: Runtime Drill",
    )
    if target == ProjectTarget.PYTHON_HOST and runtime_drill_task is None:
        errors.append("missing NobroRTOS: Runtime Drill task")
    elif target == ProjectTarget.PYTHON_HOST and not _task_has_args(
        runtime_drill_task,
        ("tools/runtime_drill.py",),
    ):
        errors.append("runtime drill task command mismatch")

    runtime_gate_task = _task_by_label(
        tasks,
        "NobroRTOS: Runtime Drill Gate",
    )
    if target == ProjectTarget.PYTHON_HOST and runtime_gate_task is None:
        errors.append("missing NobroRTOS: Runtime Drill Gate task")
    elif target == ProjectTarget.PYTHON_HOST and not _task_has_args(
        runtime_gate_task,
        ("-m", "nobro_rtos", "check-runtime-drill"),
    ):
        errors.append("runtime drill gate task command mismatch")

    ai_route_gate_task = _task_by_label(
        tasks,
        "NobroRTOS: AI Route Gate",
    )
    if target == ProjectTarget.PYTHON_HOST and ai_route_gate_task is None:
        errors.append("missing NobroRTOS: AI Route Gate task")
    elif target == ProjectTarget.PYTHON_HOST and not _task_has_args(
        ai_route_gate_task,
        ("-m", "nobro_rtos", "check-ai-route", "--backend", "hybrid"),
    ):
        errors.append("AI route gate task command mismatch")

    ai_route_matrix_gate_task = _task_by_label(
        tasks,
        "NobroRTOS: AI Route Matrix Gate",
    )
    if target == ProjectTarget.PYTHON_HOST and ai_route_matrix_gate_task is None:
        errors.append("missing NobroRTOS: AI Route Matrix Gate task")
    elif target == ProjectTarget.PYTHON_HOST and not _task_has_args(
        ai_route_matrix_gate_task,
        ("-m", "nobro_rtos", "check-ai-route-matrix"),
    ):
        errors.append("AI route matrix gate task command mismatch")

    recovery_matrix_gate_task = _task_by_label(
        tasks,
        "NobroRTOS: Recovery Matrix Gate",
    )
    if target == ProjectTarget.PYTHON_HOST and recovery_matrix_gate_task is None:
        errors.append("missing NobroRTOS: Recovery Matrix Gate task")
    elif target == ProjectTarget.PYTHON_HOST and not _task_has_args(
        recovery_matrix_gate_task,
        ("-m", "nobro_rtos", "check-recovery-matrix"),
    ):
        errors.append("recovery matrix gate task command mismatch")

    bridge_task = _task_by_label(
        tasks,
        "NobroRTOS: Bridge Smoke",
    )
    if target == ProjectTarget.PYTHON_BOARD_BRIDGE and bridge_task is None:
        errors.append("missing NobroRTOS: Bridge Smoke task")
    elif target == ProjectTarget.PYTHON_BOARD_BRIDGE and not _task_has_args(
        bridge_task,
        ("host/bridge_smoke.py",),
    ):
        errors.append("bridge smoke task command mismatch")
    return errors


def _task_by_label(tasks: list[object], label: str) -> dict[str, object] | None:
    for task in tasks:
        if isinstance(task, dict) and task.get("label") == label:
            return task
    return None


def _task_has_args(task: dict[str, object] | None, expected: tuple[str, ...]) -> bool:
    if task is None:
        return False
    args = task.get("args")
    if not isinstance(args, list):
        return False
    return all(item in args for item in expected)


def _has_vscode_task_error(errors: tuple[str, ...]) -> bool:
    return any(".vscode/tasks.json" in error or " task" in error for error in errors)
