"""Distribution metadata validation for NobroRTOS SDK/package surfaces."""

from __future__ import annotations

import ast
from dataclasses import dataclass
from pathlib import Path
import json
import tomllib
from typing import Any

from .host_contract import find_repo_root


EXPECTED_REPOSITORY = "https://github.com/dunknowcoding/NobroRTOS"
EXPECTED_REPOSITORY_GIT = f"{EXPECTED_REPOSITORY}.git"
EXPECTED_ARDUINO_REPOSITORY = "https://github.com/dunknowcoding/NobroRTOS-Arduino"
EXPECTED_LICENSE = "PolyForm-Noncommercial-1.0.0"
EXPECTED_INCLUDE = "NobroRTOS.h"
EXPECTED_CANONICAL_CONTRACT = "host/nobro-host-contract.json"
EXPECTED_PYTHON_PACKAGE = "bindings/python"
EXPECTED_PYTHON_PROJECT_NAME = "nobro-rtos-tools"
EXPECTED_PYTHON_REQUIRES = ">=3.10"
REQUIRED_REPORT_SURFACES = (
    ("board_profile", "nobro_board_profile_report_t", "BoardProfileReportView"),
    ("board_package", "nobro_board_package_report_t", "BoardPackageReportView"),
    ("manifest", "nobro_manifest_report_t", "ManifestReportView"),
    (
        "adapter_compat",
        "nobro_adapter_compat_report_t",
        "AdapterCompatReportView",
    ),
    ("ai_model", "nobro_ai_model_report_t", "AiModelReportView"),
    ("ros_bridge", "nobro_ros_bridge_report_t", "RosBridgeReportView"),
    ("admission", "nobro_admission_report_t", "AdmissionReportView"),
    ("runtime", "nobro_runtime_report_t", "RuntimeReportView"),
    ("health", "nobro_health_report_t", "HealthReportView"),
    ("event_log", "nobro_event_log_report_t", "EventLogReportView"),
    ("module_runtime", "nobro_module_runtime_report_t", "ModuleRuntimeReportView"),
    (
        "degrade_application",
        "nobro_degrade_application_report_t",
        "DegradeApplicationReportView",
    ),
)
REQUIRED_C_HELPERS = (
    "nobro_report_checksum_words",
    "nobro_report_status_from_checksum",
    "nobro_stable_hash32_cstr",
    "nobro_ai_effective_stale_after_us",
    "nobro_ai_route_decide",
    "nobro_ai_invocation_preflight",
    "nobro_ai_preflight_passing",
    "nobro_ai_preflight_has_error",
    "nobro_ros_topic_buffer_bytes",
    "nobro_ros_service_buffer_bytes",
    "nobro_ros_action_buffer_bytes",
    "nobro_ros_topic_preflight",
    "nobro_ros_service_preflight",
    "nobro_ros_action_preflight",
    "nobro_ros_parameter_preflight",
    "nobro_ros_preflight_passing",
    "nobro_ros_preflight_has_error",
)
REQUIRED_C_PREFLIGHT_BITS = (
    "NOBRO_AI_PREFLIGHT_MODEL_ID_MISMATCH",
    "NOBRO_AI_PREFLIGHT_INPUT_TOO_LARGE",
    "NOBRO_AI_PREFLIGHT_OUTPUT_TOO_SMALL",
    "NOBRO_AI_PREFLIGHT_RAM_EXCEEDED",
    "NOBRO_AI_PREFLIGHT_ROUTE_UNAVAILABLE",
    "NOBRO_AI_PREFLIGHT_DEGRADED_FALLBACK",
    "NOBRO_AI_PREFLIGHT_STALE_SNAPSHOT",
    "NOBRO_AI_PREFLIGHT_STALE_TOO_OLD",
    "NOBRO_AI_PREFLIGHT_ENDPOINT_CIRCUIT_OPEN",
    "NOBRO_AI_PREFLIGHT_LOCAL_ARENA_MISSING",
    "NOBRO_ROS_PREFLIGHT_PAYLOAD_TOO_LARGE",
    "NOBRO_ROS_PREFLIGHT_RESPONSE_TOO_SMALL",
    "NOBRO_ROS_PREFLIGHT_TIMEOUT_EXCEEDED",
    "NOBRO_ROS_PREFLIGHT_QUEUE_DEPTH_ZERO",
    "NOBRO_ROS_PREFLIGHT_TIMEOUT_ZERO",
)
REQUIRED_CPP_HELPERS = (
    "stable_hash32",
    "decide_ai_route",
    "preflight_ai_invocation",
    "AiRouteDecisionView",
    "AiInvocationPreflightView",
    "RosBridgeContractView",
    "RosBridgePreflightView",
    "preflight_ros_topic",
    "preflight_ros_service",
    "preflight_ros_action",
    "preflight_ros_parameter",
)
REQUIRED_PYTHON_EXPORTS = (
    "Capability",
    "MemoryBudget",
    "ModuleSpec",
    "NobroContractBundle",
    "ProjectTarget",
    "RecoveryRuntimeSimulator",
    "RuntimeDrillSimulator",
    "StartupDependency",
    "StartupImpact",
    "StartupPlan",
    "build_project_template",
    "plan_startup",
    "preflight_ai_invocation",
    "preflight_ros_action",
    "preflight_ros_parameter",
    "preflight_ros_service",
    "preflight_ros_topic",
    "startup_dependency_impact",
    "validate_distribution_metadata",
    "validate_public_header_surface",
    "validate_cli_command_surface",
    "validate_python_public_surface",
)
REQUIRED_CLI_COMMANDS = (
    "sample-ai-ros",
    "sample-ai-route",
    "check-ai-route",
    "check-ai-route-matrix",
    "sample-ai-preflight",
    "check-ai-preflight-matrix",
    "sample-ros-preflight",
    "check-ros-preflight-matrix",
    "check-bundle-matrix",
    "check-report-matrix",
    "sample-report",
    "sample-sensor",
    "sample-actuator",
    "sample-recovery",
    "check-recovery-matrix",
    "sample-watchdog",
    "check-watchdog-matrix",
    "check-scheduler-matrix",
    "sample-scheduler",
    "sample-event-log",
    "check-event-log-matrix",
    "sample-quota",
    "check-quota-matrix",
    "sample-degrade",
    "check-degrade-matrix",
    "sample-runtime-drill",
    "check-runtime-drill",
    "sample-startup",
    "check-startup-matrix",
    "check-boot-summary-matrix",
    "sample-project",
    "write-project",
    "check-project",
    "repair-project",
    "check-starter-templates",
    "check-host-contract",
    "check-distribution-metadata",
    "check-public-headers",
    "check-python-surface",
    "check-cli-command-surface",
    "check-software-surface",
    "doctor",
    "decode-boot",
    "validate-bundle",
    "decode-report",
    "summarize-boot",
)


@dataclass(frozen=True)
class DistributionMetadataReport:
    """Summary of repository package metadata validation."""

    sdk_name: str
    arduino_name: str
    platformio_name: str
    python_package_name: str
    python_requires: str
    include_roots: tuple[str, ...]
    host_tools: tuple[str, ...]

    def to_dict(self) -> dict[str, Any]:
        return {
            "sdk_name": self.sdk_name,
            "arduino_name": self.arduino_name,
            "platformio_name": self.platformio_name,
            "python_package_name": self.python_package_name,
            "python_requires": self.python_requires,
            "include_roots": list(self.include_roots),
            "host_tools": list(self.host_tools),
        }


@dataclass(frozen=True)
class PublicHeaderSurfaceReport:
    """Summary of public C/C++/package header surface validation."""

    c_report_count: int
    cpp_view_count: int
    c_helpers: tuple[str, ...]
    c_preflight_bits: tuple[str, ...]
    cpp_helpers: tuple[str, ...]
    forwarding_headers: tuple[str, ...]

    def to_dict(self) -> dict[str, Any]:
        return {
            "c_report_count": self.c_report_count,
            "cpp_view_count": self.cpp_view_count,
            "c_helpers": list(self.c_helpers),
            "c_preflight_bits": list(self.c_preflight_bits),
            "cpp_helpers": list(self.cpp_helpers),
            "forwarding_headers": list(self.forwarding_headers),
        }


@dataclass(frozen=True)
class PythonPublicSurfaceReport:
    """Summary of Python package public re-export validation."""

    exported_count: int
    imported_count: int
    required_exports: tuple[str, ...]
    exported_names: tuple[str, ...]

    def to_dict(self) -> dict[str, Any]:
        return {
            "exported_count": self.exported_count,
            "imported_count": self.imported_count,
            "required_exports": list(self.required_exports),
            "exported_names": list(self.exported_names),
        }


@dataclass(frozen=True)
class CliCommandSurfaceReport:
    """Summary of CLI command registration and documentation coverage."""

    command_count: int
    required_commands: tuple[str, ...]
    commands: tuple[str, ...]
    documented_commands: tuple[str, ...]

    def to_dict(self) -> dict[str, Any]:
        return {
            "command_count": self.command_count,
            "required_commands": list(self.required_commands),
            "commands": list(self.commands),
            "documented_commands": list(self.documented_commands),
        }


def validate_distribution_metadata(
    start: str | Path | None = None,
) -> DistributionMetadataReport:
    """Validate SDK, Arduino, and PlatformIO metadata against repo contracts."""

    root = find_repo_root(start)
    sdk_manifest = _read_json(root / "sdk" / "sdk-manifest.json")
    pyproject = _read_toml(root / "bindings" / "python" / "pyproject.toml")
    platformio = _read_json(root / "packages" / "platformio" / "library.json")
    arduino = _read_properties(root / "packages" / "arduino" / "library.properties")

    _require_equal(sdk_manifest.get("license"), EXPECTED_LICENSE, "SDK license")
    _require_equal(
        sdk_manifest.get("canonical_contract"),
        EXPECTED_CANONICAL_CONTRACT,
        "SDK canonical contract",
    )
    _require_equal(
        sdk_manifest.get("repository"),
        EXPECTED_REPOSITORY,
        "SDK repository",
    )
    include_roots = tuple(sdk_manifest.get("include_roots", ()))
    _require_contains(include_roots, "bindings/c/include", "SDK include roots")
    _require_contains(include_roots, "bindings/cpp/include", "SDK include roots")
    host_tools = tuple(sdk_manifest.get("host_tools", ()))
    _require_contains(host_tools, "tools/nobro_contract_tool.py", "SDK host tools")
    _require_equal(
        sdk_manifest.get("python_package"),
        EXPECTED_PYTHON_PACKAGE,
        "SDK Python package",
    )

    project = pyproject.get("project", {})
    _require_equal(
        project.get("name"),
        EXPECTED_PYTHON_PROJECT_NAME,
        "Python project name",
    )
    _require_equal(
        project.get("requires-python"),
        EXPECTED_PYTHON_REQUIRES,
        "Python requires-python",
    )
    _require_equal(
        project.get("license"),
        EXPECTED_LICENSE,
        "Python project license",
    )
    _require_equal(
        project.get("license-files"),
        ["LICENSE"],
        "Python project license files",
    )
    _require_equal(
        pyproject.get("tool", {}).get("setuptools", {}).get("packages"),
        ["nobro_rtos"],
        "Python package list",
    )

    generated_policy = sdk_manifest.get("generated_output_policy", {})
    for key in ("commit_generated_archives", "commit_compiler_outputs", "commit_cache_dirs"):
        _require_equal(generated_policy.get(key), False, f"SDK generated policy {key}")

    _require_equal(arduino.get("name"), "NobroRTOS", "Arduino package name")
    _require_equal(
        arduino.get("url"), EXPECTED_ARDUINO_REPOSITORY, "Arduino repository"
    )
    _require_equal(arduino.get("includes"), EXPECTED_INCLUDE, "Arduino include")
    _require_vendored_header(
        root / "packages" / "arduino" / "src" / EXPECTED_INCLUDE,
        "nobro_rtos.h",
        root / "bindings" / "c" / "include" / "nobro_rtos.h",
    )

    _require_equal(platformio.get("name"), "NobroRTOS", "PlatformIO package name")
    _require_equal(platformio.get("license"), EXPECTED_LICENSE, "PlatformIO license")
    _require_equal(
        platformio.get("repository", {}).get("url"),
        EXPECTED_REPOSITORY_GIT,
        "PlatformIO repository",
    )
    _require_equal(platformio.get("headers"), [EXPECTED_INCLUDE], "PlatformIO headers")
    _require_vendored_header(
        root / "packages" / "platformio" / "include" / EXPECTED_INCLUDE,
        "nobro_rtos.h",
        root / "bindings" / "c" / "include" / "nobro_rtos.h",
    )
    _require_equal(
        platformio.get("$schema"),
        "https://raw.githubusercontent.com/platformio/platformio-core/develop/"
        "platformio/assets/schema/library.json",
        "PlatformIO schema",
    )
    _require_equal(
        platformio.get("export", {}).get("include"),
        ["include", "LICENSE", "README.md", "library.json"],
        "PlatformIO export include",
    )
    canonical_license = root / "LICENSE"
    for relative in (
        "packages/arduino/LICENSE",
        "packages/platformio/LICENSE",
        "bindings/python/LICENSE",
    ):
        _require_file_equal(root / relative, canonical_license, f"{relative} license")

    return DistributionMetadataReport(
        sdk_name=str(sdk_manifest.get("name")),
        arduino_name=str(arduino.get("name")),
        platformio_name=str(platformio.get("name")),
        python_package_name=str(project.get("name")),
        python_requires=str(project.get("requires-python")),
        include_roots=include_roots,
        host_tools=host_tools,
    )


def validate_cli_command_surface(
    start: str | Path | None = None,
) -> CliCommandSurfaceReport:
    """Validate CLI command registration and command documentation coverage."""

    root = find_repo_root(start)
    cli_path = root / "bindings" / "python" / "nobro_rtos" / "cli.py"
    tree = ast.parse(cli_path.read_text(encoding="utf-8"), filename=str(cli_path))
    commands = _extract_cli_commands(tree)
    command_set = set(commands)

    if len(command_set) != len(commands):
        duplicates = _duplicates(commands)
        raise ValueError(f"CLI command parser contains duplicates: {duplicates}")

    for command in REQUIRED_CLI_COMMANDS:
        _require_contains(commands, command, "CLI command")

    docs_text = "\n".join(
        (
            (root / "bindings" / "python" / "README.md").read_text(encoding="utf-8"),
            (root / "tools" / "README.md").read_text(encoding="utf-8"),
        )
    )
    documented = tuple(command for command in REQUIRED_CLI_COMMANDS if command in docs_text)
    missing_docs = tuple(command for command in REQUIRED_CLI_COMMANDS if command not in docs_text)
    if missing_docs:
        raise ValueError(f"CLI commands missing documentation: {missing_docs}")

    return CliCommandSurfaceReport(
        command_count=len(commands),
        required_commands=REQUIRED_CLI_COMMANDS,
        commands=commands,
        documented_commands=documented,
    )


def validate_python_public_surface(
    start: str | Path | None = None,
) -> PythonPublicSurfaceReport:
    """Validate top-level Python re-exports without importing the package."""

    root = find_repo_root(start)
    init_path = root / "bindings" / "python" / "nobro_rtos" / "__init__.py"
    tree = ast.parse(init_path.read_text(encoding="utf-8"), filename=str(init_path))
    exported_names = _extract_all_names(tree, init_path)
    imported_names = _collect_public_imports(tree)
    exported_set = set(exported_names)
    imported_set = set(imported_names)

    if len(exported_set) != len(exported_names):
        duplicates = _duplicates(exported_names)
        raise ValueError(f"Python __all__ contains duplicate exports: {duplicates}")

    for symbol in REQUIRED_PYTHON_EXPORTS:
        _require_contains(exported_names, symbol, "Python public export")

    missing_imports = tuple(name for name in exported_names if name not in imported_set)
    if missing_imports:
        raise ValueError(f"Python __all__ references missing imports: {missing_imports}")

    missing_exports = tuple(name for name in imported_names if name not in exported_set)
    if missing_exports:
        raise ValueError(f"Python imports are missing from __all__: {missing_exports}")

    return PythonPublicSurfaceReport(
        exported_count=len(exported_names),
        imported_count=len(imported_names),
        required_exports=REQUIRED_PYTHON_EXPORTS,
        exported_names=exported_names,
    )


def validate_public_header_surface(
    start: str | Path | None = None,
) -> PublicHeaderSurfaceReport:
    """Validate allocation-free public headers without invoking a compiler."""

    root = find_repo_root(start)
    c_header_path = root / "bindings" / "c" / "include" / "nobro_rtos.h"
    cpp_header_path = root / "bindings" / "cpp" / "include" / "nobro_rtos.hpp"
    arduino_header_path = root / "packages" / "arduino" / "src" / EXPECTED_INCLUDE
    platformio_header_path = root / "packages" / "platformio" / "include" / EXPECTED_INCLUDE

    c_header = c_header_path.read_text(encoding="utf-8")
    cpp_header = cpp_header_path.read_text(encoding="utf-8")

    for symbol in REQUIRED_C_HELPERS:
        _require_text(c_header, symbol, "C ABI helper")
    for symbol in REQUIRED_C_PREFLIGHT_BITS:
        _require_text(c_header, symbol, "C preflight bit")
    for symbol in REQUIRED_CPP_HELPERS:
        _require_text(cpp_header, symbol, "C++ helper")

    report_count = 0
    view_count = 0
    for report_name, c_struct, cpp_view in REQUIRED_REPORT_SURFACES:
        _require_text(c_header, c_struct, f"{report_name} C report")
        _require_text(
            c_header,
            f"nobro_{report_name}_report_checksum",
            f"{report_name} checksum helper",
        )
        _require_text(
            c_header,
            f"nobro_{report_name}_report_status",
            f"{report_name} status helper",
        )
        _require_text(cpp_header, cpp_view, f"{report_name} C++ report view")
        report_count += 1
        view_count += 1

    _require_vendored_header(
        arduino_header_path,
        "nobro_rtos.h",
        root / "bindings" / "c" / "include" / "nobro_rtos.h",
    )
    _require_vendored_header(
        platformio_header_path,
        "nobro_rtos.h",
        root / "bindings" / "c" / "include" / "nobro_rtos.h",
    )

    return PublicHeaderSurfaceReport(
        c_report_count=report_count,
        cpp_view_count=view_count,
        c_helpers=REQUIRED_C_HELPERS,
        c_preflight_bits=REQUIRED_C_PREFLIGHT_BITS,
        cpp_helpers=REQUIRED_CPP_HELPERS,
        forwarding_headers=(
            arduino_header_path.relative_to(root).as_posix(),
            platformio_header_path.relative_to(root).as_posix(),
        ),
    )


def _read_json(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as handle:
        value = json.load(handle)
    if not isinstance(value, dict):
        raise ValueError(f"expected JSON object: {path}")
    return value


def _read_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        value = tomllib.load(handle)
    if not isinstance(value, dict):
        raise ValueError(f"expected TOML table: {path}")
    return value


def _read_properties(path: Path) -> dict[str, str]:
    properties: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if "=" not in stripped:
            raise ValueError(f"invalid properties line in {path}: {line}")
        key, value = stripped.split("=", 1)
        properties[key] = value
    return properties


def _require_equal(actual: Any, expected: Any, label: str) -> None:
    if actual != expected:
        raise ValueError(f"{label} expected {expected!r}, got {actual!r}")


def _require_contains(values: tuple[str, ...], expected: str, label: str) -> None:
    if expected not in values:
        raise ValueError(f"{label} missing {expected!r}")


def _require_text(text: str, expected: str, label: str) -> None:
    if expected not in text:
        raise ValueError(f"{label} missing {expected!r}")


def _require_vendored_header(path: Path, local: str, canonical: Path) -> None:
    """Self-contained package contract: the umbrella header includes the LOCAL vendored
    header (a Library-Manager install has no repo around it), and the vendored copy's
    content matches the canonical one (tools/package_arduino.py --sync keeps it fresh;
    its --check is the CI drift gate)."""
    text = path.read_text(encoding="utf-8")
    if f'#include "{local}"' not in text:
        raise ValueError(f"{path} must include the vendored {local}")
    vendored = path.parent / local
    if not vendored.exists():
        raise ValueError(f"{vendored} missing - run tools/package_arduino.py --sync")
    if canonical.read_text(encoding="utf-8") not in vendored.read_text(encoding="utf-8"):
        raise ValueError(f"{vendored} drifted from {canonical} - re-run --sync")


def _require_file_equal(path: Path, canonical: Path, label: str) -> None:
    if not path.is_file():
        raise ValueError(f"{label} missing")
    if path.read_text(encoding="utf-8") != canonical.read_text(encoding="utf-8"):
        raise ValueError(f"{label} drifted from {canonical}")


def _extract_all_names(tree: ast.Module, path: Path) -> tuple[str, ...]:
    for node in tree.body:
        if not isinstance(node, ast.Assign):
            continue
        if not any(
            isinstance(target, ast.Name) and target.id == "__all__"
            for target in node.targets
        ):
            continue
        if not isinstance(node.value, (ast.List, ast.Tuple)):
            raise ValueError(f"{path} __all__ must be a list or tuple literal")
        names: list[str] = []
        for item in node.value.elts:
            if not isinstance(item, ast.Constant) or not isinstance(item.value, str):
                raise ValueError(f"{path} __all__ must contain only string literals")
            names.append(item.value)
        return tuple(names)
    raise ValueError(f"{path} is missing __all__")


def _collect_public_imports(tree: ast.Module) -> tuple[str, ...]:
    names: list[str] = []
    for node in tree.body:
        if not isinstance(node, ast.ImportFrom):
            continue
        if node.level == 0:
            continue
        for alias in node.names:
            if alias.name == "*":
                raise ValueError("Python public surface must not use star imports")
            name = alias.asname or alias.name
            if not name.startswith("_"):
                names.append(name)
    return tuple(names)


def _extract_cli_commands(tree: ast.Module) -> tuple[str, ...]:
    commands: list[str] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        if not isinstance(node.func, ast.Attribute):
            continue
        if node.func.attr != "add_parser":
            continue
        if not node.args:
            continue
        first_arg = node.args[0]
        if isinstance(first_arg, ast.Constant) and isinstance(first_arg.value, str):
            commands.append(first_arg.value)
    return tuple(commands)


def _duplicates(values: tuple[str, ...]) -> tuple[str, ...]:
    seen: set[str] = set()
    duplicated: list[str] = []
    for value in values:
        if value in seen and value not in duplicated:
            duplicated.append(value)
        seen.add(value)
    return tuple(duplicated)
