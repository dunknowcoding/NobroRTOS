"""Distribution metadata validation for NobroRTOS SDK/package surfaces."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import json
import tomllib
from typing import Any

from .host_contract import find_repo_root


EXPECTED_REPOSITORY = "https://github.com/dunknowcoding/NobroRTOS"
EXPECTED_REPOSITORY_GIT = f"{EXPECTED_REPOSITORY}.git"
EXPECTED_LICENSE = "Apache-2.0"
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
        project.get("license", {}).get("text"),
        EXPECTED_LICENSE,
        "Python project license",
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
    _require_equal(arduino.get("url"), EXPECTED_REPOSITORY, "Arduino repository")
    _require_equal(arduino.get("includes"), EXPECTED_INCLUDE, "Arduino include")
    _require_forwarding_header(
        root / "packages" / "arduino" / "src" / EXPECTED_INCLUDE,
        "../../../bindings/c/include/nobro_rtos.h",
    )

    _require_equal(platformio.get("name"), "NobroRTOS", "PlatformIO package name")
    _require_equal(platformio.get("license"), EXPECTED_LICENSE, "PlatformIO license")
    _require_equal(
        platformio.get("repository", {}).get("url"),
        EXPECTED_REPOSITORY_GIT,
        "PlatformIO repository",
    )
    _require_equal(platformio.get("headers"), [EXPECTED_INCLUDE], "PlatformIO headers")
    _require_forwarding_header(
        root / "packages" / "platformio" / "include" / EXPECTED_INCLUDE,
        "../../../bindings/c/include/nobro_rtos.h",
    )

    return DistributionMetadataReport(
        sdk_name=str(sdk_manifest.get("name")),
        arduino_name=str(arduino.get("name")),
        platformio_name=str(platformio.get("name")),
        python_package_name=str(project.get("name")),
        python_requires=str(project.get("requires-python")),
        include_roots=include_roots,
        host_tools=host_tools,
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

    _require_forwarding_header(
        arduino_header_path,
        "../../../bindings/c/include/nobro_rtos.h",
    )
    _require_forwarding_header(
        platformio_header_path,
        "../../../bindings/c/include/nobro_rtos.h",
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


def _require_forwarding_header(path: Path, target: str) -> None:
    text = path.read_text(encoding="utf-8")
    if f'#include "{target}"' not in text:
        raise ValueError(f"{path} must forward to {target}")
