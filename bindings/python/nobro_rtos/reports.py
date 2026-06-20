"""Fixed report decoding helpers for NobroRTOS host tooling."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from pathlib import Path
import json
from typing import Any

from .host_contract import HostContract, load_repo_host_contract


MANIFEST_REPORT_MAGIC = 0x4E42_4D46
ADAPTER_COMPAT_REPORT_MAGIC = 0x4E42_4143
REPORT_VERSION = 1

REPORT_FIELDS = (
    "magic",
    "version",
    "completed",
    "ok",
    "item_count",
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


class ReportKind(Enum):
    MANIFEST = "manifest"
    ADAPTER_COMPAT = "adapter_compatibility"


class ReportStatus(str, Enum):
    MISSING = "missing"
    IN_PROGRESS = "in_progress"
    PASS = "pass"
    FAIL = "fail"
    CORRUPT = "corrupt"


@dataclass(frozen=True)
class FixedReport:
    kind: ReportKind
    fields: dict[str, int]
    expected_magic: int
    ok_field: str
    count_field: str
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
        tag = self.fields["error_module_tag"]
        if tag == 0:
            return None
        try:
            return self.contract.module_label(tag)
        except KeyError:
            return None

    def to_dict(self) -> dict[str, Any]:
        return {
            "kind": self.kind.value,
            "status": self.status.value,
            "passing": self.passing,
            "checksum_ok": self.verify_checksum(),
            "count": self.fields[self.count_field],
            "required_bits": self.fields["required_bits"],
            "owned_bits": self.fields["owned_bits"],
            "flash_used_bytes": self.fields["flash_used_bytes"],
            "ram_used_bytes": self.fields["ram_used_bytes"],
            "pool_used_slots": self.fields["pool_used_slots"],
            "error_code": self.fields["error_code"],
            "error_label": self.error_label(),
            "error_module_label": self.error_module_label(),
            "error_capability_bits": self.fields["error_capability_bits"],
        }


def seal_report(kind: ReportKind | str, payload: dict[str, Any]) -> dict[str, int]:
    """Return a copy of a report payload with magic/version/completed/checksum set."""

    report_kind = ReportKind(kind)
    fields = dict(payload)
    if report_kind == ReportKind.MANIFEST:
        expected_magic = MANIFEST_REPORT_MAGIC
        field_names = MANIFEST_FIELDS
    elif report_kind == ReportKind.ADAPTER_COMPAT:
        expected_magic = ADAPTER_COMPAT_REPORT_MAGIC
        field_names = ADAPTER_COMPAT_FIELDS
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
