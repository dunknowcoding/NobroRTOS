"""Host-contract loader and validation helpers for NobroRTOS tooling."""

from __future__ import annotations

from dataclasses import dataclass
from enum import IntEnum
from pathlib import Path
import json
from typing import Any

from .contracts import AiBackendKind, AiRoutePreference, AiRouteTarget, Capability


DEFAULT_CONTRACT_RELATIVE_PATH = Path("host") / "nobro-host-contract.json"
BUNDLED_CONTRACT_PATH = Path(__file__).with_name("nobro-host-contract.json")
EXPECTED_TOP_LEVEL_KEYS = frozenset(
    {
        "cdc",
        "upload",
        "ina_monitor",
        "boot_diagnostics",
        "module_tags",
        "capability_bits",
        "ai_contracts",
        "ros_bridge_contracts",
        "health_report",
        "event_log_report",
        "module_runtime_report",
        "degrade_application_report",
        "runtime_report",
        "board_profile_report",
        "board_package_report",
        "manifest_report",
        "adapter_compat_report",
        "admission_report",
    }
)
EXPECTED_SECTION_KEYS = {
    "cdc": {"maintenance_mi", "user_mi"},
    "upload": {"touch_baud", "lock_per_port"},
    "ina_monitor": {"jsonl_line_rate_hz_max"},
    "boot_diagnostics": {
        "stage_order",
        "status_labels",
        "diagnostic_code",
        "first_non_pass",
        "summary_fields",
    },
    "health_report": {"symbol", "magic", "version", "layout"},
    "event_log_report": {
        "symbol",
        "magic",
        "version",
        "layout",
        "event_kind_codes",
        "payload_kind_codes",
        "severity_codes",
    },
    "module_runtime_report": {"symbol", "magic", "version", "layout", "state_codes"},
    "degrade_application_report": {
        "symbol",
        "magic",
        "version",
        "layout",
        "reason_codes",
    },
    "runtime_report": {"symbol", "magic", "version", "layout", "state_codes"},
    "board_profile_report": {"symbol", "magic", "version", "layout", "catalog", "status"},
    "board_package_report": {
        "symbol",
        "magic",
        "version",
        "layout",
        "catalog",
        "status",
        "boot_layout_codes",
        "error_codes",
    },
    "manifest_report": {"symbol", "magic", "version", "layout", "status", "error_codes"},
    "adapter_compat_report": {
        "symbol",
        "magic",
        "version",
        "layout",
        "status",
        "error_codes",
    },
    "admission_report": {"symbol", "magic", "version", "layout", "status", "error_codes"},
}
EXPECTED_BOOT_STAGES = (
    "board_profile",
    "board_package",
    "manifest",
    "adapter_compatibility",
    "admission",
    "runtime",
)
EXPECTED_STATUS_LABELS = ("missing", "in_progress", "pass", "fail", "corrupt")
ERROR_REPORT_KEYS = {
    "board_package": "board_package_report",
    "manifest": "manifest_report",
    "adapter_compatibility": "adapter_compat_report",
    "admission": "admission_report",
}
EXPECTED_ROS_TRANSPORT_CODES = {
    "1": "serial",
    "2": "udp",
    "3": "radio",
    "4": "shared_memory",
    "255": "custom",
}
EXPECTED_REPORT_CONTRACTS = {
    "board_profile_report": ("NOBRO_BOARD_PROFILE_REPORT", "0x4E424250"),
    "board_package_report": ("NOBRO_BOARD_PACKAGE_REPORT", "0x4E42424B"),
    "manifest_report": ("NOBRO_MANIFEST_REPORT", "0x4E424D46"),
    "adapter_compat_report": ("NOBRO_ADAPTER_COMPAT_REPORT", "0x4E424143"),
    "admission_report": ("NOBRO_ADMISSION_REPORT", "0x4E424144"),
    "runtime_report": ("NOBRO_RUNTIME_REPORT", "0x4E425254"),
    "health_report": ("NOBRO_HEALTH_REPORT", "0x4E42484C"),
    "event_log_report": ("NOBRO_EVENT_LOG_REPORT", "0x4E42454C"),
    "module_runtime_report": ("NOBRO_MODULE_RUNTIME_REPORT", "0x4E424D52"),
    "degrade_application_report": ("NOBRO_DEGRADE_APPLICATION_REPORT", "0x4E424447"),
}


class ReportStatusClass(IntEnum):
    PASS = 0
    MISSING = 1
    IN_PROGRESS = 2
    CORRUPT = 3
    FAIL = 4

    @property
    def label(self) -> str:
        return {
            self.PASS: "pass",
            self.MISSING: "missing",
            self.IN_PROGRESS: "in_progress",
            self.CORRUPT: "corrupt",
            self.FAIL: "fail",
        }[self]


@dataclass(frozen=True)
class BootDiagnostic:
    """Decoded boot diagnostic code.

    The code layout mirrors `nobro-host`:
    stage_code << 24 | status_class << 16 | error_code_low16.
    """

    stage_code: int
    stage: str
    status_class: ReportStatusClass
    error_code: int
    error_label: str | None = None

    @classmethod
    def decode(
        cls, code: int, contract: "HostContract | None" = None
    ) -> "BootDiagnostic":
        contract = load_repo_host_contract() if contract is None else contract
        stage_code = (code >> 24) & 0xFF
        status_class_code = (code >> 16) & 0xFF
        error_code = code & 0xFFFF
        status_class = ReportStatusClass(status_class_code)
        stage = contract.boot_stage_label(stage_code)
        error_label = (
            contract.error_label(stage, error_code)
            if status_class == ReportStatusClass.FAIL
            else None
        )
        return cls(stage_code, stage, status_class, error_code, error_label)

    @property
    def status(self) -> str:
        return self.status_class.label

    @property
    def passing(self) -> bool:
        return self.status_class == ReportStatusClass.PASS

    def to_dict(self) -> dict[str, Any]:
        return {
            "stage_code": self.stage_code,
            "stage": self.stage,
            "status": self.status,
            "error_code": self.error_code,
            "error_label": self.error_label,
            "passing": self.passing,
        }


@dataclass(frozen=True)
class HostContract:
    """Parsed host contract mirrored from the repository JSON file."""

    payload: dict[str, Any]

    @classmethod
    def from_path(cls, path: str | Path) -> "HostContract":
        with Path(path).open("r", encoding="utf-8-sig") as handle:
            payload = json.load(handle)
        contract = cls(payload)
        contract.validate()
        return contract

    def validate(self) -> None:
        if not isinstance(self.payload, dict) or set(self.payload) != EXPECTED_TOP_LEVEL_KEYS:
            raise ValueError("unexpected host-contract top-level schema")
        for key in EXPECTED_TOP_LEVEL_KEYS:
            self._require_object(key)
        for key, expected in EXPECTED_SECTION_KEYS.items():
            self._require_exact_keys(key, expected)
        self._validate_basic_sections()
        boot = self._require_object("boot_diagnostics")

        stages = tuple(boot.get("stage_order", ()))
        if stages != EXPECTED_BOOT_STAGES:
            raise ValueError(f"unexpected boot stage order: {stages}")

        status_labels = tuple(boot.get("status_labels", ()))
        if status_labels != EXPECTED_STATUS_LABELS:
            raise ValueError(f"unexpected status labels: {status_labels}")

        self._validate_capability_bits()
        self._validate_report_contracts()
        self._validate_ai_contracts()
        self._validate_ros_bridge_contracts()

    def capability_label(self, capability: Capability) -> str:
        return self.payload["capability_bits"][str(int(capability))]

    def module_label(self, code: int | str) -> str:
        return self.payload["module_tags"][str(code)]

    def boot_stage_order(self) -> tuple[str, ...]:
        return tuple(self.payload["boot_diagnostics"]["stage_order"])

    def status_labels(self) -> tuple[str, ...]:
        return tuple(self.payload["boot_diagnostics"]["status_labels"])

    def boot_stage_label(self, stage_code: int) -> str:
        stages = self.boot_stage_order()
        if stage_code < 1 or stage_code > len(stages):
            raise ValueError(f"unknown boot stage code: {stage_code}")
        return stages[stage_code - 1]

    def error_label(self, stage: str, code: int) -> str | None:
        report_key = ERROR_REPORT_KEYS.get(stage)
        if report_key is None:
            return None
        report = self.payload.get(report_key)
        if not isinstance(report, dict):
            return None
        labels = report.get("error_codes")
        if not isinstance(labels, dict):
            return None
        return labels.get(str(code))

    def ai_backend_label(self, code: int | str) -> str | None:
        if int(code) == 0:
            return None
        return self.payload["ai_contracts"]["backend_codes"][str(code)]

    def ai_route_preference_label(self, code: int | str) -> str | None:
        if int(code) == 0:
            return None
        return self.payload["ai_contracts"]["route_preferences"][str(code)]

    def ai_route_target_label(self, code: int | str) -> str | None:
        if int(code) == 0:
            return None
        return self.payload["ai_contracts"]["route_targets"][str(code)]

    def ros_transport_label(self, code: int | str) -> str | None:
        if int(code) == 0:
            return None
        return self.payload["ros_bridge_contracts"]["transport_codes"][str(code)]

    def _require_object(self, key: str) -> dict[str, Any]:
        value = self.payload.get(key)
        if not isinstance(value, dict):
            raise ValueError(f"missing object: {key}")
        return value

    def _require_exact_keys(self, key: str, expected: set[str]) -> None:
        if set(self._require_object(key)) != expected:
            raise ValueError(f"unexpected {key} schema")

    def _validate_basic_sections(self) -> None:
        cdc = self.payload["cdc"]
        if not all(
            isinstance(cdc[key], str) and cdc[key]
            for key in ("maintenance_mi", "user_mi")
        ):
            raise ValueError("unexpected CDC contract")
        upload = self.payload["upload"]
        if type(upload["touch_baud"]) is not int or upload["touch_baud"] <= 0:
            raise ValueError("unexpected upload baud contract")
        if type(upload["lock_per_port"]) is not bool:
            raise ValueError("unexpected upload lock contract")
        monitor = self.payload["ina_monitor"]
        rate = monitor["jsonl_line_rate_hz_max"]
        if type(rate) is not int or rate <= 0:
            raise ValueError("unexpected monitor rate contract")
        boot = self.payload["boot_diagnostics"]
        if not isinstance(boot["diagnostic_code"], str):
            raise ValueError("unexpected diagnostic-code contract")
        if type(boot["first_non_pass"]) is not bool:
            raise ValueError("unexpected diagnostic selection contract")
        if not isinstance(boot["summary_fields"], list) or not all(
            isinstance(item, str) and item for item in boot["summary_fields"]
        ):
            raise ValueError("unexpected diagnostic summary contract")

    def _validate_capability_bits(self) -> None:
        capability_bits = self.payload["capability_bits"]
        for capability in Capability:
            label = capability_bits.get(str(int(capability)))
            expected = capability.name.lower()
            if label != expected:
                raise ValueError(
                    f"capability {capability.name} expected {expected}, got {label}"
                )

    def _validate_report_contracts(self) -> None:
        for key, (expected_symbol, expected_magic) in EXPECTED_REPORT_CONTRACTS.items():
            report = self._require_object(key)
            symbol = report.get("symbol")
            if symbol != expected_symbol:
                raise ValueError(f"unexpected {key} symbol: {symbol}")
            magic = report.get("magic")
            if magic != expected_magic:
                raise ValueError(f"unexpected {key} magic: {magic}")
            version = report.get("version")
            if type(version) is not int or version != 1:
                raise ValueError(f"unexpected {key} version: {version}")

    def _validate_ai_contracts(self) -> None:
        ai_contracts = self._require_object("ai_contracts")
        if set(ai_contracts) != {
            "backend_codes",
            "report",
            "route_preferences",
            "route_targets",
        }:
            raise ValueError("unexpected AI contract schema")
        report = ai_contracts.get("report")
        if not isinstance(report, dict):
            raise ValueError("missing AI model report contract")
        if set(report) != {"symbol", "magic", "version", "layout"}:
            raise ValueError("unexpected AI report schema")
        if report.get("symbol") != "NOBRO_AI_MODEL_REPORT":
            raise ValueError(f"unexpected AI model report symbol: {report.get('symbol')}")
        if report.get("magic") != "0x4E424149":
            raise ValueError(f"unexpected AI model report magic: {report.get('magic')}")
        if type(report.get("version")) is not int or report.get("version") != 1:
            raise ValueError("unexpected AI model report version")
        self._validate_enum_codes(
            ai_contracts.get("backend_codes"),
            AiBackendKind,
            "AI backend",
        )
        self._validate_enum_codes(
            ai_contracts.get("route_preferences"),
            AiRoutePreference,
            "AI route preference",
        )
        self._validate_enum_codes(
            ai_contracts.get("route_targets"),
            AiRouteTarget,
            "AI route target",
        )

    def _validate_ros_bridge_contracts(self) -> None:
        bridge = self._require_object("ros_bridge_contracts")
        if set(bridge) != {"entity_kinds", "hash", "report", "transport_codes"}:
            raise ValueError("unexpected ROS bridge contract schema")
        report = bridge.get("report")
        if not isinstance(report, dict):
            raise ValueError("missing ROS bridge report contract")
        if set(report) != {"symbol", "magic", "version", "layout"}:
            raise ValueError("unexpected ROS bridge report schema")
        if report.get("symbol") != "NOBRO_ROS_BRIDGE_REPORT":
            raise ValueError(f"unexpected ROS bridge report symbol: {report.get('symbol')}")
        if report.get("magic") != "0x4E425253":
            raise ValueError(f"unexpected ROS bridge report magic: {report.get('magic')}")
        if type(report.get("version")) is not int or report.get("version") != 1:
            raise ValueError("unexpected ROS bridge report version")
        if bridge.get("hash") != "fnv1a32_utf8":
            raise ValueError(f"unexpected ROS bridge hash: {bridge.get('hash')}")
        transports = bridge.get("transport_codes")
        if transports != EXPECTED_ROS_TRANSPORT_CODES:
            raise ValueError(f"unexpected ROS transport codes: {transports}")
        if tuple(bridge.get("entity_kinds", ())) != (
            "topic",
            "service",
            "action",
            "parameter",
        ):
            raise ValueError("unexpected ROS bridge entity kinds")

    def _validate_enum_codes(
        self,
        codes: Any,
        enum_type: type[AiBackendKind] | type[AiRoutePreference] | type[AiRouteTarget],
        label: str,
    ) -> None:
        if not isinstance(codes, dict):
            raise ValueError(f"missing {label} code table")
        for item in enum_type:
            actual = codes.get(str(int(item)))
            expected = item.name.lower()
            if actual != expected:
                raise ValueError(f"{label} {item.name} expected {expected}, got {actual}")


def find_repo_root(start: str | Path | None = None) -> Path:
    """Find a repository root containing the canonical host contract JSON."""

    current = Path.cwd() if start is None else Path(start).resolve()
    for candidate in (current, *current.parents):
        contract_path = candidate / DEFAULT_CONTRACT_RELATIVE_PATH
        if contract_path.exists():
            return candidate
    raise FileNotFoundError("could not find host/nobro-host-contract.json")


def load_repo_host_contract(start: str | Path | None = None) -> HostContract:
    try:
        root = find_repo_root(start)
    except FileNotFoundError:
        if start is not None or not BUNDLED_CONTRACT_PATH.is_file():
            raise
        return HostContract.from_path(BUNDLED_CONTRACT_PATH)
    return HostContract.from_path(root / DEFAULT_CONTRACT_RELATIVE_PATH)
