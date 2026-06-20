"""Fixed report decoding helpers for NobroRTOS host tooling."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from pathlib import Path
import json
from typing import Any

from .host_contract import HostContract, load_repo_host_contract


BOOT_STAGE_TO_REPORT_KIND = {
    "board_profile": "board_profile",
    "board_package": "board_package",
    "manifest": "manifest",
    "adapter_compatibility": "adapter_compatibility",
    "admission": "admission",
    "runtime": "runtime",
}
BOARD_PROFILE_REPORT_MAGIC = 0x4E42_4250
BOARD_PACKAGE_REPORT_MAGIC = 0x4E42_424B
MANIFEST_REPORT_MAGIC = 0x4E42_4D46
ADAPTER_COMPAT_REPORT_MAGIC = 0x4E42_4143
ADMISSION_REPORT_MAGIC = 0x4E42_4144
RUNTIME_REPORT_MAGIC = 0x4E42_5254
HEALTH_REPORT_MAGIC = 0x4E42_484C
EVENT_LOG_REPORT_MAGIC = 0x4E42_454C
MODULE_RUNTIME_REPORT_MAGIC = 0x4E42_4D52
DEGRADE_APPLICATION_REPORT_MAGIC = 0x4E42_4447
AI_MODEL_REPORT_MAGIC = 0x4E42_4149
ROS_BRIDGE_REPORT_MAGIC = 0x4E42_5253
REPORT_VERSION = 1

BOARD_PROFILE_FIELDS = (
    "magic",
    "version",
    "completed",
    "platform_hash",
    "board_hash",
    "app_flash_start",
    "flash_budget_bytes",
    "ram_budget_bytes",
    "sample_pool_slots",
    "max_modules",
    "servo_pin",
    "servo_center_us",
    "led_pin",
    "mvk_trigger_pin",
    "checksum",
)

BOARD_PACKAGE_FIELDS = (
    "magic",
    "version",
    "completed",
    "valid",
    "platform_hash",
    "board_hash",
    "boot_layout",
    "app_flash_start",
    "app_flash_len_bytes",
    "ram_start",
    "ram_len_bytes",
    "flash_budget_bytes",
    "ram_budget_bytes",
    "sample_pool_slots",
    "max_modules",
    "led_pin",
    "servo_pin",
    "mvk_trigger_pin",
    "error_code",
    "checksum",
)

MANIFEST_FIELDS = (
    "magic",
    "version",
    "completed",
    "valid",
    "module_count",
    "fingerprint",
    "required_bits",
    "owned_bits",
    "flash_used_bytes",
    "ram_used_bytes",
    "pool_used_slots",
    "error_code",
    "error_module_tag",
    "error_capability_bits",
    "checksum",
)

ADAPTER_COMPAT_FIELDS = (
    "magic",
    "version",
    "completed",
    "compatible",
    "adapter_count",
    "required_bits",
    "owned_bits",
    "flash_used_bytes",
    "ram_used_bytes",
    "pool_used_slots",
    "error_code",
    "error_module_tag",
    "error_capability_bits",
    "checksum",
)

AI_MODEL_FIELDS = (
    "magic",
    "version",
    "completed",
    "backend",
    "model_id",
    "input_bytes_max",
    "output_bytes_max",
    "arena_bytes",
    "timeout_us",
    "route_preference",
    "stale_after_us",
    "endpoint_failure_limit",
    "checksum",
)

ROS_BRIDGE_FIELDS = (
    "magic",
    "version",
    "completed",
    "transport",
    "bridge_id_hash",
    "topic_count",
    "service_count",
    "action_count",
    "parameter_count",
    "total_buffer_bytes",
    "max_timeout_us",
    "checksum",
)

ADMISSION_FIELDS = (
    "magic",
    "version",
    "completed",
    "admitted",
    "module_count",
    "startup_len",
    "flash_used_bytes",
    "flash_limit_bytes",
    "ram_used_bytes",
    "ram_limit_bytes",
    "pool_used_slots",
    "pool_limit_slots",
    "error_code",
    "checksum",
)

RUNTIME_FIELDS = (
    "magic",
    "version",
    "completed",
    "state",
    "module_count",
    "mailbox_len",
    "mailbox_dropped",
    "alarm_len",
    "next_alarm_due_us_lo",
    "next_alarm_due_us_hi",
    "kv_len",
    "kv_writes",
    "kv_deletes",
    "quota_flash_used_bytes",
    "quota_ram_used_bytes",
    "quota_pool_used_slots",
    "event_count",
    "dropped_events",
    "checksum",
)

HEALTH_FIELDS = (
    "magic",
    "version",
    "completed",
    "module_tag",
    "total_errors",
    "consecutive_errors",
    "last_error",
    "last_action",
    "event_count",
    "dropped_events",
    "error_events",
    "fatal_events",
    "last_seen_us_lo",
    "last_seen_us_hi",
    "checksum",
)

EVENT_LOG_FIELDS = (
    "magic",
    "version",
    "completed",
    "event_count",
    "capacity",
    "dropped_events",
    "latest_seq",
    "latest_at_us_lo",
    "latest_at_us_hi",
    "latest_module_tag",
    "latest_severity",
    "latest_kind",
    "latest_payload_kind",
    "latest_payload0",
    "latest_payload1",
    "checksum",
)

MODULE_RUNTIME_FIELDS = (
    "magic",
    "version",
    "completed",
    "module_count",
    "capacity",
    "active_count",
    "suspended_count",
    "faulted_count",
    "recovering_count",
    "disabled_count",
    "latest_module_tag",
    "latest_state",
    "latest_fault_count",
    "latest_recovery_count",
    "latest_change_us_lo",
    "latest_change_us_hi",
    "checksum",
)

DEGRADE_APPLICATION_FIELDS = (
    "magic",
    "version",
    "completed",
    "requested_count",
    "disabled_count",
    "already_disabled_count",
    "reason",
    "applied_at_us_lo",
    "applied_at_us_hi",
    "checksum",
)


class ReportKind(Enum):
    BOARD_PROFILE = "board_profile"
    BOARD_PACKAGE = "board_package"
    MANIFEST = "manifest"
    ADAPTER_COMPAT = "adapter_compatibility"
    ADMISSION = "admission"
    RUNTIME = "runtime"
    HEALTH = "health"
    EVENT_LOG = "event_log"
    MODULE_RUNTIME = "module_runtime"
    DEGRADE_APPLICATION = "degrade_application"
    AI_MODEL = "ai_model"
    ROS_BRIDGE = "ros_bridge"


class ReportStatus(str, Enum):
    MISSING = "missing"
    IN_PROGRESS = "in_progress"
    PASS = "pass"
    FAIL = "fail"
    CORRUPT = "corrupt"


SUMMARY_STATUS_ORDER = (
    ReportStatus.PASS,
    ReportStatus.MISSING,
    ReportStatus.IN_PROGRESS,
    ReportStatus.FAIL,
    ReportStatus.CORRUPT,
)
STATUS_CLASS_CODES = {
    ReportStatus.PASS: 0,
    ReportStatus.MISSING: 1,
    ReportStatus.IN_PROGRESS: 2,
    ReportStatus.CORRUPT: 3,
    ReportStatus.FAIL: 4,
}


@dataclass(frozen=True)
class ReportSlot:
    stage: str
    status: ReportStatus
    symbol: str
    error_code: int = 0
    error_label: str | None = None
    detail: dict[str, Any] | None = None

    @property
    def passing(self) -> bool:
        return self.status == ReportStatus.PASS

    def stage_code(self, contract: HostContract) -> int:
        return contract.boot_stage_order().index(self.stage) + 1

    def status_class_code(self) -> int:
        return STATUS_CLASS_CODES[self.status]

    def diagnostic_code(self, contract: HostContract) -> int:
        return (
            (self.stage_code(contract) << 24)
            | (self.status_class_code() << 16)
            | (self.error_code & 0xFFFF)
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "stage": self.stage,
            "symbol": self.symbol,
            "status": self.status.value,
            "passing": self.passing,
            "error_code": self.error_code,
            "error_label": self.error_label,
            "detail": self.detail,
        }


@dataclass(frozen=True)
class BootReportSummary:
    slots: tuple[ReportSlot, ...]
    contract: HostContract

    @classmethod
    def from_dict(
        cls, payload: dict[str, Any], contract: HostContract | None = None
    ) -> "BootReportSummary":
        contract = load_repo_host_contract() if contract is None else contract
        reports = payload.get("reports", payload)
        slots: list[ReportSlot] = []
        for stage in contract.boot_stage_order():
            report_payload = reports.get(stage)
            slots.append(_slot_from_payload(stage, report_payload, contract))
        return cls(tuple(slots), contract)

    @classmethod
    def from_json_file(
        cls, path: str | Path, contract: HostContract | None = None
    ) -> "BootReportSummary":
        with Path(path).open("r", encoding="utf-8-sig") as handle:
            return cls.from_dict(json.load(handle), contract)

    @property
    def first_diagnostic(self) -> ReportSlot:
        for slot in self.slots:
            if not slot.passing:
                return slot
        return self.slots[-1]

    @property
    def passing(self) -> bool:
        return all(slot.passing for slot in self.slots)

    def status_counts(self) -> dict[str, int]:
        counts = {status.value: 0 for status in SUMMARY_STATUS_ORDER}
        for slot in self.slots:
            counts[slot.status.value] += 1
        return counts

    def diagnostic_code(self) -> int:
        return self.first_diagnostic.diagnostic_code(self.contract)

    def observed_count(self) -> int:
        return len(self.slots)

    def to_dict(self) -> dict[str, Any]:
        first = self.first_diagnostic
        counts = self.status_counts()
        return {
            "passing": self.passing,
            "diagnostic_code": self.diagnostic_code(),
            "diagnostic": first.to_dict(),
            "first_stage": first.stage,
            "first_status": first.status.value,
            "first_symbol": first.symbol,
            "first_error_code": first.error_code,
            "first_error_label": first.error_label,
            "pass_count": counts[ReportStatus.PASS.value],
            "missing_count": counts[ReportStatus.MISSING.value],
            "in_progress_count": counts[ReportStatus.IN_PROGRESS.value],
            "fail_count": counts[ReportStatus.FAIL.value],
            "corrupt_count": counts[ReportStatus.CORRUPT.value],
            "observed_count": self.observed_count(),
            "status_counts": counts,
            "slots": [slot.to_dict() for slot in self.slots],
        }


@dataclass(frozen=True)
class FixedReport:
    kind: ReportKind
    fields: dict[str, int]
    expected_magic: int
    ok_field: str | None
    count_field: str | None
    contract: HostContract

    @classmethod
    def from_json_file(
        cls, kind: ReportKind | str, path: str | Path, contract: HostContract | None = None
    ) -> "FixedReport":
        with Path(path).open("r", encoding="utf-8-sig") as handle:
            return cls.from_dict(kind, json.load(handle), contract)

    @classmethod
    def from_dict(
        cls, kind: ReportKind | str, payload: dict[str, Any], contract: HostContract | None = None
    ) -> "FixedReport":
        report_kind = ReportKind(kind)
        contract = load_repo_host_contract() if contract is None else contract
        if report_kind == ReportKind.BOARD_PROFILE:
            return cls(
                report_kind,
                _normalize_fields(payload, BOARD_PROFILE_FIELDS),
                BOARD_PROFILE_REPORT_MAGIC,
                None,
                None,
                contract,
            )
        if report_kind == ReportKind.BOARD_PACKAGE:
            return cls(
                report_kind,
                _normalize_fields(payload, BOARD_PACKAGE_FIELDS),
                BOARD_PACKAGE_REPORT_MAGIC,
                "valid",
                None,
                contract,
            )
        if report_kind == ReportKind.MANIFEST:
            return cls(
                report_kind,
                _normalize_fields(payload, MANIFEST_FIELDS),
                MANIFEST_REPORT_MAGIC,
                "valid",
                "module_count",
                contract,
            )
        if report_kind == ReportKind.ADAPTER_COMPAT:
            return cls(
                report_kind,
                _normalize_fields(payload, ADAPTER_COMPAT_FIELDS),
                ADAPTER_COMPAT_REPORT_MAGIC,
                "compatible",
                "adapter_count",
                contract,
            )
        if report_kind == ReportKind.ADMISSION:
            return cls(
                report_kind,
                _normalize_fields(payload, ADMISSION_FIELDS),
                ADMISSION_REPORT_MAGIC,
                "admitted",
                "module_count",
                contract,
            )
        if report_kind == ReportKind.RUNTIME:
            return cls(
                report_kind,
                _normalize_fields(payload, RUNTIME_FIELDS),
                RUNTIME_REPORT_MAGIC,
                None,
                "module_count",
                contract,
            )
        if report_kind == ReportKind.HEALTH:
            return cls(
                report_kind,
                _normalize_fields(payload, HEALTH_FIELDS),
                HEALTH_REPORT_MAGIC,
                None,
                None,
                contract,
            )
        if report_kind == ReportKind.EVENT_LOG:
            return cls(
                report_kind,
                _normalize_fields(payload, EVENT_LOG_FIELDS),
                EVENT_LOG_REPORT_MAGIC,
                None,
                "event_count",
                contract,
            )
        if report_kind == ReportKind.MODULE_RUNTIME:
            return cls(
                report_kind,
                _normalize_fields(payload, MODULE_RUNTIME_FIELDS),
                MODULE_RUNTIME_REPORT_MAGIC,
                None,
                "module_count",
                contract,
            )
        if report_kind == ReportKind.DEGRADE_APPLICATION:
            return cls(
                report_kind,
                _normalize_fields(payload, DEGRADE_APPLICATION_FIELDS),
                DEGRADE_APPLICATION_REPORT_MAGIC,
                None,
                "requested_count",
                contract,
            )
        if report_kind == ReportKind.AI_MODEL:
            return cls(
                report_kind,
                _normalize_fields(payload, AI_MODEL_FIELDS),
                AI_MODEL_REPORT_MAGIC,
                None,
                None,
                contract,
            )
        if report_kind == ReportKind.ROS_BRIDGE:
            return cls(
                report_kind,
                _normalize_fields(payload, ROS_BRIDGE_FIELDS),
                ROS_BRIDGE_REPORT_MAGIC,
                None,
                None,
                contract,
            )
        raise ValueError(f"unsupported report kind: {kind}")

    @property
    def status(self) -> ReportStatus:
        if (
            self.fields["magic"] == 0
            and self.fields["version"] == 0
            and self.fields["checksum"] == 0
        ):
            return ReportStatus.MISSING
        if (
            self.fields["magic"] != self.expected_magic
            or self.fields["version"] != REPORT_VERSION
        ):
            return ReportStatus.CORRUPT
        if self.fields["completed"] == 0:
            return ReportStatus.IN_PROGRESS
        if not self.verify_checksum():
            return ReportStatus.CORRUPT
        if self.ok_field is None:
            return ReportStatus.PASS
        if self.fields[self.ok_field] != 0:
            return ReportStatus.PASS
        return ReportStatus.FAIL

    @property
    def passing(self) -> bool:
        return self.status == ReportStatus.PASS

    def verify_checksum(self) -> bool:
        return self.fields["checksum"] == self.compute_checksum()

    def compute_checksum(self) -> int:
        checksum = 0
        for name, value in self.fields.items():
            if name != "checksum":
                checksum ^= value
        return checksum & 0xFFFF_FFFF

    def error_label(self) -> str | None:
        if self.status != ReportStatus.FAIL:
            return None
        return self.contract.error_label(self.kind.value, self.fields["error_code"])

    def error_module_label(self) -> str | None:
        tag = self.fields.get("error_module_tag", 0)
        if tag == 0:
            return None
        try:
            return self.contract.module_label(tag)
        except KeyError:
            return None

    def to_dict(self) -> dict[str, Any]:
        decoded = {
            "kind": self.kind.value,
            "status": self.status.value,
            "passing": self.passing,
            "checksum_ok": self.verify_checksum(),
            "count": self.fields[self.count_field] if self.count_field else None,
            "required_bits": self.fields.get("required_bits", 0),
            "owned_bits": self.fields.get("owned_bits", 0),
            "flash_used_bytes": self.fields.get("flash_used_bytes", 0),
            "ram_used_bytes": self.fields.get("ram_used_bytes", 0),
            "pool_used_slots": self.fields.get("pool_used_slots", 0),
            "error_code": self.fields.get("error_code", 0),
            "error_label": self.error_label(),
            "error_module_label": self.error_module_label(),
            "error_capability_bits": self.fields.get("error_capability_bits", 0),
            "raw": dict(self.fields),
        }
        decoded.update(self._domain_fields())
        return decoded

    def _domain_fields(self) -> dict[str, Any]:
        if self.kind == ReportKind.AI_MODEL:
            return {
                "backend": self.contract.ai_backend_label(self.fields["backend"]),
                "model_id": self.fields["model_id"],
                "input_bytes_max": self.fields["input_bytes_max"],
                "output_bytes_max": self.fields["output_bytes_max"],
                "arena_bytes": self.fields["arena_bytes"],
                "timeout_us": self.fields["timeout_us"],
                "route_preference": self.contract.ai_route_preference_label(
                    self.fields["route_preference"]
                ),
                "stale_after_us": self.fields["stale_after_us"],
                "endpoint_failure_limit": self.fields["endpoint_failure_limit"],
            }
        if self.kind == ReportKind.ROS_BRIDGE:
            return {
                "transport": self.contract.ros_transport_label(self.fields["transport"]),
                "bridge_id_hash": self.fields["bridge_id_hash"],
                "topic_count": self.fields["topic_count"],
                "service_count": self.fields["service_count"],
                "action_count": self.fields["action_count"],
                "parameter_count": self.fields["parameter_count"],
                "total_buffer_bytes": self.fields["total_buffer_bytes"],
                "max_timeout_us": self.fields["max_timeout_us"],
            }
        if self.kind == ReportKind.ADMISSION:
            return {
                "admitted": self.fields["admitted"] != 0,
                "startup_len": self.fields["startup_len"],
                "flash_limit_bytes": self.fields["flash_limit_bytes"],
                "ram_limit_bytes": self.fields["ram_limit_bytes"],
                "pool_limit_slots": self.fields["pool_limit_slots"],
            }
        if self.kind == ReportKind.RUNTIME:
            return {
                "state_code": self.fields["state"],
                "module_count": self.fields["module_count"],
                "mailbox_len": self.fields["mailbox_len"],
                "mailbox_dropped": self.fields["mailbox_dropped"],
                "alarm_len": self.fields["alarm_len"],
                "next_alarm_due_us": _u64(
                    self.fields["next_alarm_due_us_lo"],
                    self.fields["next_alarm_due_us_hi"],
                ),
                "kv_len": self.fields["kv_len"],
                "quota_flash_used_bytes": self.fields["quota_flash_used_bytes"],
                "quota_ram_used_bytes": self.fields["quota_ram_used_bytes"],
                "quota_pool_used_slots": self.fields["quota_pool_used_slots"],
                "event_count": self.fields["event_count"],
                "dropped_events": self.fields["dropped_events"],
            }
        if self.kind == ReportKind.HEALTH:
            return {
                "module_label": self._module_label(self.fields["module_tag"]),
                "total_errors": self.fields["total_errors"],
                "consecutive_errors": self.fields["consecutive_errors"],
                "last_error": self.fields["last_error"],
                "last_action": self.fields["last_action"],
                "event_count": self.fields["event_count"],
                "dropped_events": self.fields["dropped_events"],
                "error_events": self.fields["error_events"],
                "fatal_events": self.fields["fatal_events"],
                "last_seen_us": _u64(
                    self.fields["last_seen_us_lo"],
                    self.fields["last_seen_us_hi"],
                ),
            }
        if self.kind == ReportKind.EVENT_LOG:
            return {
                "event_count": self.fields["event_count"],
                "capacity": self.fields["capacity"],
                "dropped_events": self.fields["dropped_events"],
                "latest_seq": self.fields["latest_seq"],
                "latest_at_us": _u64(
                    self.fields["latest_at_us_lo"],
                    self.fields["latest_at_us_hi"],
                ),
                "latest_module_label": self._module_label(
                    self.fields["latest_module_tag"]
                ),
                "latest_severity": self.fields["latest_severity"],
                "latest_kind": self.fields["latest_kind"],
                "latest_payload_kind": self.fields["latest_payload_kind"],
                "latest_payload0": self.fields["latest_payload0"],
                "latest_payload1": self.fields["latest_payload1"],
            }
        if self.kind == ReportKind.MODULE_RUNTIME:
            return {
                "module_count": self.fields["module_count"],
                "capacity": self.fields["capacity"],
                "active_count": self.fields["active_count"],
                "suspended_count": self.fields["suspended_count"],
                "faulted_count": self.fields["faulted_count"],
                "recovering_count": self.fields["recovering_count"],
                "disabled_count": self.fields["disabled_count"],
                "latest_module_label": self._module_label(
                    self.fields["latest_module_tag"]
                ),
                "latest_state": self.fields["latest_state"],
                "latest_fault_count": self.fields["latest_fault_count"],
                "latest_recovery_count": self.fields["latest_recovery_count"],
                "latest_change_us": _u64(
                    self.fields["latest_change_us_lo"],
                    self.fields["latest_change_us_hi"],
                ),
            }
        if self.kind == ReportKind.DEGRADE_APPLICATION:
            return {
                "requested_count": self.fields["requested_count"],
                "disabled_count": self.fields["disabled_count"],
                "already_disabled_count": self.fields["already_disabled_count"],
                "reason": self.fields["reason"],
                "applied_at_us": _u64(
                    self.fields["applied_at_us_lo"],
                    self.fields["applied_at_us_hi"],
                ),
            }
        return {}

    def _module_label(self, tag: int) -> str | None:
        if tag == 0:
            return None
        try:
            return self.contract.module_label(tag)
        except KeyError:
            return None


def seal_report(kind: ReportKind | str, payload: dict[str, Any]) -> dict[str, int]:
    """Return a copy of a report payload with magic/version/completed/checksum set."""

    report_kind = ReportKind(kind)
    fields = dict(payload)
    if report_kind == ReportKind.BOARD_PROFILE:
        expected_magic = BOARD_PROFILE_REPORT_MAGIC
        field_names = BOARD_PROFILE_FIELDS
    elif report_kind == ReportKind.BOARD_PACKAGE:
        expected_magic = BOARD_PACKAGE_REPORT_MAGIC
        field_names = BOARD_PACKAGE_FIELDS
    elif report_kind == ReportKind.MANIFEST:
        expected_magic = MANIFEST_REPORT_MAGIC
        field_names = MANIFEST_FIELDS
    elif report_kind == ReportKind.ADAPTER_COMPAT:
        expected_magic = ADAPTER_COMPAT_REPORT_MAGIC
        field_names = ADAPTER_COMPAT_FIELDS
    elif report_kind == ReportKind.ADMISSION:
        expected_magic = ADMISSION_REPORT_MAGIC
        field_names = ADMISSION_FIELDS
    elif report_kind == ReportKind.RUNTIME:
        expected_magic = RUNTIME_REPORT_MAGIC
        field_names = RUNTIME_FIELDS
    elif report_kind == ReportKind.HEALTH:
        expected_magic = HEALTH_REPORT_MAGIC
        field_names = HEALTH_FIELDS
    elif report_kind == ReportKind.EVENT_LOG:
        expected_magic = EVENT_LOG_REPORT_MAGIC
        field_names = EVENT_LOG_FIELDS
    elif report_kind == ReportKind.MODULE_RUNTIME:
        expected_magic = MODULE_RUNTIME_REPORT_MAGIC
        field_names = MODULE_RUNTIME_FIELDS
    elif report_kind == ReportKind.DEGRADE_APPLICATION:
        expected_magic = DEGRADE_APPLICATION_REPORT_MAGIC
        field_names = DEGRADE_APPLICATION_FIELDS
    elif report_kind == ReportKind.AI_MODEL:
        expected_magic = AI_MODEL_REPORT_MAGIC
        field_names = AI_MODEL_FIELDS
    elif report_kind == ReportKind.ROS_BRIDGE:
        expected_magic = ROS_BRIDGE_REPORT_MAGIC
        field_names = ROS_BRIDGE_FIELDS
    else:
        raise ValueError(f"unsupported report kind: {kind}")

    fields["magic"] = expected_magic
    fields["version"] = REPORT_VERSION
    fields["completed"] = 1
    fields["checksum"] = 0
    normalized = _normalize_fields(fields, field_names)
    checksum = 0
    for name, value in normalized.items():
        if name != "checksum":
            checksum ^= value
    normalized["checksum"] = checksum & 0xFFFF_FFFF
    return normalized


def _normalize_fields(payload: dict[str, Any], field_names: tuple[str, ...]) -> dict[str, int]:
    return {name: int(payload.get(name, 0)) & 0xFFFF_FFFF for name in field_names}


def _u64(lo: int, hi: int) -> int:
    return ((hi & 0xFFFF_FFFF) << 32) | (lo & 0xFFFF_FFFF)


def _slot_from_payload(
    stage: str, payload: Any, contract: HostContract
) -> ReportSlot:
    symbol = contract.payload.get(f"{stage}_report", {}).get("symbol", stage)
    if stage == "adapter_compatibility":
        symbol = contract.payload.get("adapter_compat_report", {}).get("symbol", stage)

    if payload is None:
        return ReportSlot(stage, ReportStatus.MISSING, symbol)

    report_kind = BOOT_STAGE_TO_REPORT_KIND.get(stage)
    if report_kind is not None:
        report = FixedReport.from_dict(report_kind, payload, contract)
        detail = report.to_dict()
        return ReportSlot(
            stage=stage,
            status=report.status,
            symbol=symbol,
            error_code=detail["error_code"],
            error_label=detail["error_label"],
            detail=detail,
        )

    status = ReportStatus(str(payload.get("status", ReportStatus.MISSING.value)))
    error_code = int(payload.get("error_code", 0))
    return ReportSlot(
        stage=stage,
        status=status,
        symbol=symbol,
        error_code=error_code,
        error_label=contract.error_label(stage, error_code),
        detail=dict(payload),
    )
