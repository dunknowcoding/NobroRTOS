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
    else:
        files = _python_host_files(name, module_name, author)

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


def _standalone_sdk_files(
    name: str,
    module_name: str,
    author: str,
) -> tuple[TemplateFile, ...]:
    return (
        TemplateFile("README.md", _readme(name, "Standalone SDK", author)),
        TemplateFile("nobro-contract.json", _contract_json(name, module_name)),
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


def _contract_json(name: str, module_name: str) -> str:
    bundle = NobroContractBundle(
        metadata={"project": name, "template": "starter"},
        modules=(
            ModuleSpec(
                module_name,
                Criticality.USER,
                MemoryBudget(8192, 2048, 1),
                requires=(Capability.TIMEBASE,),
            ),
        ),
    )
    return json.dumps(bundle.to_dict(), indent=2, sort_keys=True) + "\n"


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
