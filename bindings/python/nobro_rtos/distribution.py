"""Distribution metadata validation for NobroRTOS SDK/package surfaces."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import json
from typing import Any

from .host_contract import find_repo_root


EXPECTED_REPOSITORY = "https://github.com/dunknowcoding/NobroRTOS"
EXPECTED_REPOSITORY_GIT = f"{EXPECTED_REPOSITORY}.git"
EXPECTED_LICENSE = "Apache-2.0"
EXPECTED_INCLUDE = "NobroRTOS.h"
EXPECTED_CANONICAL_CONTRACT = "host/nobro-host-contract.json"


@dataclass(frozen=True)
class DistributionMetadataReport:
    """Summary of repository package metadata validation."""

    sdk_name: str
    arduino_name: str
    platformio_name: str
    include_roots: tuple[str, ...]
    host_tools: tuple[str, ...]

    def to_dict(self) -> dict[str, Any]:
        return {
            "sdk_name": self.sdk_name,
            "arduino_name": self.arduino_name,
            "platformio_name": self.platformio_name,
            "include_roots": list(self.include_roots),
            "host_tools": list(self.host_tools),
        }


def validate_distribution_metadata(
    start: str | Path | None = None,
) -> DistributionMetadataReport:
    """Validate SDK, Arduino, and PlatformIO metadata against repo contracts."""

    root = find_repo_root(start)
    sdk_manifest = _read_json(root / "sdk" / "sdk-manifest.json")
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
        include_roots=include_roots,
        host_tools=host_tools,
    )


def _read_json(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as handle:
        value = json.load(handle)
    if not isinstance(value, dict):
        raise ValueError(f"expected JSON object: {path}")
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


def _require_forwarding_header(path: Path, target: str) -> None:
    text = path.read_text(encoding="utf-8")
    if f'#include "{target}"' not in text:
        raise ValueError(f"{path} must forward to {target}")
