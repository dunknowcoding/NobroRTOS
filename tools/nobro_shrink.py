#!/usr/bin/env python3
"""Fail-closed capacity right-sizing for a NobroRTOS workload.

The input is an observed occupancy report.  The output is a *proposal* bound to
the workload and the exact declarations that produced the observations; this
tool never edits a workload or source file.

Report schema (all fields are required and unknown fields are rejected)::

  {
    "schema": "nobro-shrink-report-v1",
    "workload_id": "sha256:<64 lower-case hex digits>",
    "declaration_id": "sha256:<digest of the resource declarations>",
    "margin_percent": 25,
    "coverage": {
      "workload_complete": true,
      "isr_paths_covered": true,
      "unseen_path_reserve_percent": 10
    },
    "resources": [
      {
        "name": "control.stack",
        "kind": "stack_bytes",
        "declared": 1024,
        "observed_peak": 240,
        "granularity": 8,
        "saturated": false,
        "dropped": false
      }
    ]
  }

``declaration_id`` is the SHA-256 identity returned by
``declaration_identity(resources)``.  A report is unsafe when a resource met
its declaration, any producer counter saturated, any event was dropped, or
workload/ISR coverage is incomplete.  Unsafe results contain no proposed
declarations.  A positive unseen-path reserve is always required and is added
to the requested margin before alignment.

Usage::

    python tools/nobro_shrink.py report.json
    python tools/nobro_shrink.py report.json --json recommendation.json
    python tools/nobro_shrink.py --bindings --campaign campaign.json \
        --workload workload.json --build-manifest build.json
    python tools/nobro_shrink.py --device-report capacity-report.bin \
        --campaign campaign.json --workload workload.json \
        --build-manifest build.json --json report.json
    python tools/nobro_shrink.py --selftest

Exit 0 only for a valid, safe recommendation; malformed or unsafe evidence
exits 1.
"""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import os
import pathlib
import re
import struct
import subprocess
import sys
import tempfile
from typing import Any

REPORT_SCHEMA = "nobro-shrink-report-v1"
RESULT_SCHEMA = "nobro-shrink-recommendation-v1"
MAX_CAPACITY = (1 << 31) - 1
MAX_PERCENT = 1000
MAX_RESOURCES = 4096

CAMPAIGN_SCHEMA = "nobro-capacity-campaign-v1"
BINDINGS_SCHEMA = "nobro-capacity-bindings-v1"
CAPACITY_REPORT_MAGIC = 0x4E425243
CAPACITY_REPORT_VERSION = 1
CAPACITY_REPORT_FIXED_BYTES = 184
CAPACITY_RESOURCE_RECORD_BYTES = 60
CAPACITY_REPORT_HEADER_WORDS = 21
CAPACITY_RESOURCE_KINDS = {1: "stack_bytes", 2: "queue_slots", 3: "pool_slots"}
FNV1A32_OFFSET = 0x811C9DC5
FNV1A32_PRIME = 0x01000193

# Per-kind minimum floor so a quiet run never shrinks a resource to an
# unusable capacity.
FLOORS = {"stack_bytes": 64, "queue_slots": 1, "pool_slots": 1}

_IDENTITY = re.compile(r"^sha256:[0-9a-f]{64}$")
_RESOURCE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:/@+>~-]{0,95}$")
_REPORT_KEYS = {
    "schema", "workload_id", "declaration_id", "margin_percent", "coverage", "resources"
}
_COVERAGE_KEYS = {
    "workload_complete", "isr_paths_covered", "unseen_path_reserve_percent"
}
_RESOURCE_KEYS = {
    "name", "kind", "declared", "observed_peak", "granularity", "saturated", "dropped"
}
_CAMPAIGN_KEYS = {
    "schema",
    "session_id",
    "margin_percent",
    "unseen_path_reserve_percent",
    "required_path_mask",
    "isr_path_mask",
    "resources",
}
_DECLARATION_KEYS = {"name", "kind", "declared", "granularity"}
_PATH_MASK = re.compile(r"^0x[0-9a-f]{16}$")


def _object_with_unique_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _load_json_object(path: pathlib.Path, location: str) -> dict[str, Any]:
    """Load an object without accepting duplicate keys or non-finite constants."""

    def reject_constant(value: str) -> None:
        raise ValueError(f"non-finite JSON number is not allowed: {value}")

    with path.open(encoding="utf-8") as stream:
        value = json.load(
            stream,
            object_pairs_hook=_object_with_unique_keys,
            parse_constant=reject_constant,
        )
    if type(value) is not dict:
        raise ValueError(f"{location} must be a JSON object")
    return value


def load_report(path: pathlib.Path) -> dict[str, Any]:
    return _load_json_object(path, "report")


def _exact_keys(value: Any, expected: set[str], location: str) -> dict[str, Any]:
    if type(value) is not dict:
        raise ValueError(f"{location} must be an object")
    if any(type(key) is not str for key in value):
        raise ValueError(f"{location} field names must be strings")
    actual = set(value)
    missing = sorted(expected - actual)
    unknown = sorted(actual - expected)
    if missing:
        raise ValueError(f"{location} missing fields: {', '.join(missing)}")
    if unknown:
        raise ValueError(f"{location} has unknown fields: {', '.join(unknown)}")
    return value


def _integer(value: Any, name: str, minimum: int, maximum: int) -> int:
    # bool is an int subclass in Python; exact type checking is intentional.
    if type(value) is not int:
        raise ValueError(f"{name} must be an integer (booleans are not integers here)")
    if not minimum <= value <= maximum:
        raise ValueError(f"{name} must be between {minimum} and {maximum}")
    return value


def _boolean(value: Any, name: str) -> bool:
    if type(value) is not bool:
        raise ValueError(f"{name} must be a boolean")
    return value


def _identity(value: Any, name: str) -> str:
    if type(value) is not str or not _IDENTITY.fullmatch(value):
        raise ValueError(f"{name} must be sha256:<64 lower-case hex digits>")
    return value


def _canonical_declarations(resources: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return sorted(
        (
            {
                "declared": resource["declared"],
                "granularity": resource["granularity"],
                "kind": resource["kind"],
                "name": resource["name"],
            }
            for resource in resources
        ),
        key=lambda item: item["name"],
    )


def declaration_identity(resources: list[dict[str, Any]]) -> str:
    """Return the order-independent identity of the declared resource set."""
    canonical = json.dumps(
        _canonical_declarations(resources),
        ensure_ascii=True,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("ascii")
    return "sha256:" + hashlib.sha256(canonical).hexdigest()


def _path_mask(value: Any, name: str) -> int:
    if type(value) is not str or not _PATH_MASK.fullmatch(value):
        raise ValueError(f"{name} must be 0x followed by 16 lower-case hex digits")
    return int(value, 16)


def _domain_digest(domain: bytes, *parts: bytes) -> bytes:
    digest = hashlib.sha256()
    digest.update(domain)
    for part in parts:
        digest.update(len(part).to_bytes(8, "little"))
        digest.update(part)
    return digest.digest()


def _identity_text(value: bytes) -> str:
    return "sha256:" + value.hex()


def resource_identity(name: str) -> str:
    """Strong identity used in the binary report in place of a public name."""
    return _identity_text(_domain_digest(b"nobro-resource-v1\0", name.encode("ascii")))


def _validated_campaign(value: Any) -> dict[str, Any]:
    campaign = _exact_keys(value, _CAMPAIGN_KEYS, "campaign")
    if campaign["schema"] != CAMPAIGN_SCHEMA:
        raise ValueError(f"campaign.schema must be {CAMPAIGN_SCHEMA!r}")
    session_id = _integer(campaign["session_id"], "campaign.session_id", 1, 0xFFFFFFFF)
    margin = _integer(campaign["margin_percent"], "campaign.margin_percent", 0, MAX_PERCENT)
    reserve = _integer(
        campaign["unseen_path_reserve_percent"],
        "campaign.unseen_path_reserve_percent",
        1,
        MAX_PERCENT,
    )
    required_paths = _path_mask(campaign["required_path_mask"], "campaign.required_path_mask")
    isr_paths = _path_mask(campaign["isr_path_mask"], "campaign.isr_path_mask")
    if required_paths == 0:
        raise ValueError("campaign.required_path_mask must declare at least one path")
    if isr_paths & ~required_paths:
        raise ValueError("campaign.isr_path_mask contains a path that is not required")

    raw_resources = campaign["resources"]
    if type(raw_resources) is not list or not raw_resources:
        raise ValueError("campaign.resources must be a non-empty list")
    if len(raw_resources) > MAX_RESOURCES:
        raise ValueError(f"campaign.resources must contain at most {MAX_RESOURCES} entries")
    names: set[str] = set()
    singleton_kinds: set[str] = set()
    resources: list[dict[str, Any]] = []
    for index, raw in enumerate(raw_resources):
        location = f"campaign.resources[{index}]"
        resource = _exact_keys(raw, _DECLARATION_KEYS, location)
        name = resource["name"]
        if type(name) is not str or not _RESOURCE_NAME.fullmatch(name):
            raise ValueError(f"{location}.name has invalid characters or length")
        if name in names:
            raise ValueError(f"duplicate campaign resource name: {name}")
        names.add(name)
        kind = resource["kind"]
        if type(kind) is not str or kind not in FLOORS:
            raise ValueError(f"{name}: unknown kind {kind!r}")
        if kind in {"queue_slots", "pool_slots"}:
            if kind in singleton_kinds:
                raise ValueError(f"campaign may declare only one {kind} resource")
            singleton_kinds.add(kind)
        resources.append(
            {
                "name": name,
                "kind": kind,
                "declared": _integer(
                    resource["declared"], f"{name}.declared", 1, MAX_CAPACITY
                ),
                "granularity": _integer(
                    resource["granularity"], f"{name}.granularity", 1, MAX_CAPACITY
                ),
            }
        )
    return {
        "schema": CAMPAIGN_SCHEMA,
        "session_id": session_id,
        "margin_percent": margin,
        "unseen_path_reserve_percent": reserve,
        "required_path_mask": f"0x{required_paths:016x}",
        "isr_path_mask": f"0x{isr_paths:016x}",
        "resources": sorted(resources, key=lambda resource: resource["name"]),
    }


def load_campaign(path: pathlib.Path) -> dict[str, Any]:
    return _validated_campaign(_load_json_object(path, "campaign"))


def capacity_bindings(
    campaign: dict[str, Any], workload_bytes: bytes, build_manifest_bytes: bytes
) -> dict[str, Any]:
    """Derive every strong identity embedded by the firmware producer."""
    campaign = _validated_campaign(campaign)
    required_paths = int(campaign["required_path_mask"], 16)
    isr_paths = int(campaign["isr_path_mask"], 16)
    coverage_contract = struct.pack(
        "<QQII",
        required_paths,
        isr_paths,
        campaign["margin_percent"],
        campaign["unseen_path_reserve_percent"],
    )
    build_id = _identity_text(
        _domain_digest(b"nobro-build-v1\0", build_manifest_bytes)
    )
    workload_id = _identity_text(
        _domain_digest(b"nobro-workload-v1\0", workload_bytes, coverage_contract)
    )
    declaration_id = declaration_identity(campaign["resources"])
    return {
        "schema": BINDINGS_SCHEMA,
        "build_id": build_id,
        "workload_id": workload_id,
        "declaration_id": declaration_id,
        "session_id": campaign["session_id"],
        "margin_percent": campaign["margin_percent"],
        "unseen_path_reserve_percent": campaign["unseen_path_reserve_percent"],
        "required_path_mask": campaign["required_path_mask"],
        "isr_path_mask": campaign["isr_path_mask"],
        "resources": [
            {**resource, "resource_id": resource_identity(resource["name"])}
            for resource in campaign["resources"]
        ],
    }


def _fnv1a32(data: bytes) -> int:
    value = FNV1A32_OFFSET
    for byte in data:
        value = ((value ^ byte) * FNV1A32_PRIME) & 0xFFFFFFFF
    return value


def _identity_bytes(value: str) -> bytes:
    _identity(value, "identity")
    return bytes.fromhex(value.removeprefix("sha256:"))


def convert_capacity_binary(
    data: bytes,
    campaign: dict[str, Any],
    workload_bytes: bytes,
    build_manifest_bytes: bytes,
) -> dict[str, Any]:
    """Verify one target-produced report and convert it to the strict v1 schema."""
    campaign = _validated_campaign(campaign)
    bindings = capacity_bindings(campaign, workload_bytes, build_manifest_bytes)
    if len(data) < CAPACITY_REPORT_FIXED_BYTES:
        raise ValueError("capacity report is truncated")
    header = struct.unpack_from(f"<{CAPACITY_REPORT_HEADER_WORDS}I", data, 0)
    (
        magic,
        version,
        report_bytes,
        completed,
        flags,
        session_id,
        resource_count,
        resource_capacity,
        margin_percent,
        reserve_percent,
        coverage_finished,
        workload_complete,
        isr_paths_covered,
        required_paths_lo,
        required_paths_hi,
        observed_paths_lo,
        observed_paths_hi,
        required_isr_paths_lo,
        required_isr_paths_hi,
        observed_isr_paths_lo,
        observed_isr_paths_hi,
    ) = header
    if magic != CAPACITY_REPORT_MAGIC or version != CAPACITY_REPORT_VERSION:
        raise ValueError("capacity report magic/version mismatch")
    if report_bytes != len(data):
        raise ValueError("capacity report length field does not match the input")
    if resource_capacity > MAX_RESOURCES:
        raise ValueError("capacity report resource capacity exceeds the host limit")
    expected_bytes = CAPACITY_REPORT_FIXED_BYTES + (
        resource_capacity * CAPACITY_RESOURCE_RECORD_BYTES
    )
    if report_bytes != expected_bytes:
        raise ValueError("capacity report has an invalid fixed-layout size")
    expected_checksum = _fnv1a32(data[:-4])
    (checksum,) = struct.unpack_from("<I", data, len(data) - 4)
    if checksum != expected_checksum:
        raise ValueError("capacity report checksum mismatch")
    if completed != 1 or flags != 0:
        raise ValueError("capacity report is structurally incomplete")
    if resource_count == 0 or resource_count != len(campaign["resources"]):
        raise ValueError("capacity report resource count does not match the campaign")
    if resource_count > resource_capacity:
        raise ValueError("capacity report resource count exceeds its fixed capacity")
    if session_id != campaign["session_id"]:
        raise ValueError("capacity report session does not match the campaign")
    if margin_percent != campaign["margin_percent"] or reserve_percent != campaign[
        "unseen_path_reserve_percent"
    ]:
        raise ValueError("capacity report margin/reserve policy does not match the campaign")

    for value, name in (
        (coverage_finished, "coverage_finished"),
        (workload_complete, "workload_complete"),
        (isr_paths_covered, "isr_paths_covered"),
    ):
        if value not in (0, 1):
            raise ValueError(f"capacity report {name} is not a boolean word")
    if coverage_finished != 1:
        raise ValueError("capacity report was captured before the campaign was sealed")
    required_paths = required_paths_lo | (required_paths_hi << 32)
    observed_paths = observed_paths_lo | (observed_paths_hi << 32)
    required_isr_paths = required_isr_paths_lo | (required_isr_paths_hi << 32)
    observed_isr_paths = observed_isr_paths_lo | (observed_isr_paths_hi << 32)
    if required_paths != int(campaign["required_path_mask"], 16):
        raise ValueError("capacity report required-path mask does not match the campaign")
    if required_isr_paths != int(campaign["isr_path_mask"], 16):
        raise ValueError("capacity report ISR-path mask does not match the campaign")
    if observed_paths & ~required_paths:
        raise ValueError("capacity report observed an undeclared workload path")
    if observed_isr_paths & ~required_isr_paths or observed_isr_paths & ~observed_paths:
        raise ValueError("capacity report contains impossible ISR coverage")
    expected_workload_complete = int(observed_paths & required_paths == required_paths)
    expected_isr_complete = int(
        observed_isr_paths & required_isr_paths == required_isr_paths
    )
    if workload_complete != expected_workload_complete:
        raise ValueError("capacity report workload coverage flag is inconsistent")
    if isr_paths_covered != expected_isr_complete:
        raise ValueError("capacity report ISR coverage flag is inconsistent")

    identity_offset = CAPACITY_REPORT_HEADER_WORDS * 4
    raw_build_id = data[identity_offset : identity_offset + 32]
    raw_workload_id = data[identity_offset + 32 : identity_offset + 64]
    raw_declaration_id = data[identity_offset + 64 : identity_offset + 96]
    if raw_build_id != _identity_bytes(bindings["build_id"]):
        raise ValueError("capacity report build identity does not match the build manifest")
    if raw_workload_id != _identity_bytes(bindings["workload_id"]):
        raise ValueError("capacity report workload identity does not match the workload")
    if raw_declaration_id != _identity_bytes(bindings["declaration_id"]):
        raise ValueError("capacity report declaration identity does not match the registry")

    expected_by_id = {
        _identity_bytes(resource["resource_id"]): resource
        for resource in bindings["resources"]
    }
    observed_ids: set[bytes] = set()
    resources: list[dict[str, Any]] = []
    records_offset = identity_offset + 96
    for index in range(resource_count):
        offset = records_offset + index * CAPACITY_RESOURCE_RECORD_BYTES
        resource_id = data[offset : offset + 32]
        if resource_id in observed_ids:
            raise ValueError("capacity report contains a duplicate resource identity")
        observed_ids.add(resource_id)
        expected = expected_by_id.get(resource_id)
        if expected is None:
            raise ValueError("capacity report contains a resource outside the exact registry")
        (
            kind_code,
            declared,
            observed_peak,
            granularity,
            saturated,
            dropped,
            failure_count,
        ) = struct.unpack_from("<7I", data, offset + 32)
        if kind_code not in CAPACITY_RESOURCE_KINDS:
            raise ValueError("capacity report contains an unknown resource kind")
        kind = CAPACITY_RESOURCE_KINDS[kind_code]
        if kind != expected["kind"] or declared != expected["declared"] or granularity != expected[
            "granularity"
        ]:
            raise ValueError("capacity report resource declaration does not match the registry")
        if saturated not in (0, 1) or dropped not in (0, 1):
            raise ValueError("capacity report resource flags are not boolean words")
        if kind in {"queue_slots", "pool_slots"} and dropped != int(failure_count != 0):
            raise ValueError("capacity report drop flag disagrees with its failure counter")
        if kind == "stack_bytes" and (dropped != 0 or (failure_count != 0 and saturated != 1)):
            raise ValueError("capacity report stack fault evidence is inconsistent")
        resources.append(
            {
                "name": expected["name"],
                "kind": kind,
                "declared": declared,
                "observed_peak": observed_peak,
                "granularity": granularity,
                "saturated": bool(saturated),
                "dropped": bool(dropped),
            }
        )
    if observed_ids != set(expected_by_id):
        raise ValueError("capacity report is missing a declared resource")
    unused_offset = records_offset + resource_count * CAPACITY_RESOURCE_RECORD_BYTES
    checksum_offset = len(data) - 4
    if any(data[unused_offset:checksum_offset]):
        raise ValueError("capacity report has nonzero unused resource records")

    report = {
        "schema": REPORT_SCHEMA,
        "workload_id": bindings["workload_id"],
        "declaration_id": bindings["declaration_id"],
        "margin_percent": margin_percent,
        "coverage": {
            "workload_complete": bool(workload_complete),
            "isr_paths_covered": bool(isr_paths_covered),
            "unseen_path_reserve_percent": reserve_percent,
        },
        "resources": resources,
    }
    return _validated_report(report)


def _validated_report(report: Any) -> dict[str, Any]:
    report = _exact_keys(report, _REPORT_KEYS, "report")
    if report["schema"] != REPORT_SCHEMA:
        raise ValueError(f"schema must be {REPORT_SCHEMA!r}")
    workload_id = _identity(report["workload_id"], "workload_id")
    declaration_id = _identity(report["declaration_id"], "declaration_id")
    margin = _integer(report["margin_percent"], "margin_percent", 0, MAX_PERCENT)

    coverage = _exact_keys(report["coverage"], _COVERAGE_KEYS, "coverage")
    workload_complete = _boolean(
        coverage["workload_complete"], "coverage.workload_complete"
    )
    isr_paths_covered = _boolean(
        coverage["isr_paths_covered"], "coverage.isr_paths_covered"
    )
    unseen_reserve = _integer(
        coverage["unseen_path_reserve_percent"],
        "coverage.unseen_path_reserve_percent",
        1,
        MAX_PERCENT,
    )

    raw_resources = report["resources"]
    if type(raw_resources) is not list or not raw_resources:
        raise ValueError("resources must be a non-empty list")
    if len(raw_resources) > MAX_RESOURCES:
        raise ValueError(f"resources must contain at most {MAX_RESOURCES} entries")

    resources: list[dict[str, Any]] = []
    names: set[str] = set()
    for index, raw in enumerate(raw_resources):
        location = f"resources[{index}]"
        resource = _exact_keys(raw, _RESOURCE_KEYS, location)
        name = resource["name"]
        if type(name) is not str or not _RESOURCE_NAME.fullmatch(name):
            raise ValueError(f"{location}.name has invalid characters or length")
        if name in names:
            raise ValueError(f"duplicate resource name: {name}")
        names.add(name)
        kind = resource["kind"]
        if type(kind) is not str or kind not in FLOORS:
            raise ValueError(f"{name}: unknown kind {kind!r}")
        declared = _integer(resource["declared"], f"{name}.declared", 1, MAX_CAPACITY)
        observed_peak = _integer(
            resource["observed_peak"], f"{name}.observed_peak", 0, MAX_CAPACITY
        )
        granularity = _integer(
            resource["granularity"], f"{name}.granularity", 1, MAX_CAPACITY
        )
        resources.append(
            {
                "name": name,
                "kind": kind,
                "declared": declared,
                "observed_peak": observed_peak,
                "granularity": granularity,
                "saturated": _boolean(resource["saturated"], f"{name}.saturated"),
                "dropped": _boolean(resource["dropped"], f"{name}.dropped"),
            }
        )

    actual_identity = declaration_identity(resources)
    if declaration_id != actual_identity:
        raise ValueError("declaration_id does not match the resource declarations")
    return {
        "schema": REPORT_SCHEMA,
        "workload_id": workload_id,
        "declaration_id": declaration_id,
        "margin_percent": margin,
        "coverage": {
            "workload_complete": workload_complete,
            "isr_paths_covered": isr_paths_covered,
            "unseen_path_reserve_percent": unseen_reserve,
        },
        "resources": sorted(resources, key=lambda item: item["name"]),
    }


def recommend(
    resource: dict[str, Any], margin_percent: int, unseen_path_reserve_percent: int
) -> dict[str, Any]:
    """Calculate one aligned proposal after report validation."""
    total_percent = 100 + margin_percent + unseen_path_reserve_percent
    target = (resource["observed_peak"] * total_percent + 99) // 100
    target = max(target, FLOORS[resource["kind"]], resource["observed_peak"])
    granularity = resource["granularity"]
    target = ((target + granularity - 1) // granularity) * granularity
    if target > MAX_CAPACITY:
        raise ValueError(f"{resource['name']}: recommendation exceeds supported capacity")

    unsafe_reasons: list[str] = []
    if resource["observed_peak"] >= resource["declared"]:
        unsafe_reasons.append("observed_peak_reached_declaration")
    if resource["saturated"]:
        unsafe_reasons.append("producer_counter_saturated")
    if resource["dropped"]:
        unsafe_reasons.append("events_dropped")
    if unsafe_reasons:
        status = "UNSAFE"
    elif target < resource["declared"]:
        status = "SHRINK"
    elif target > resource["declared"]:
        status = "GROW"
    else:
        status = "OK"
    return {
        "name": resource["name"],
        "kind": resource["kind"],
        "declared": resource["declared"],
        "observed_peak": resource["observed_peak"],
        "granularity": granularity,
        "recommended": target,
        "delta": target - resource["declared"],
        "status": status,
        "saturated": resource["saturated"],
        "dropped": resource["dropped"],
        "unsafe_reasons": unsafe_reasons,
    }


def analyze(report: dict[str, Any]) -> dict[str, Any]:
    """Validate a report and produce an identity-bound recommendation."""
    report = _validated_report(report)
    coverage = report["coverage"]
    rows = [
        recommend(
            resource,
            report["margin_percent"],
            coverage["unseen_path_reserve_percent"],
        )
        for resource in report["resources"]
    ]

    failures: list[str] = []
    if not coverage["workload_complete"]:
        failures.append("coverage:workload_incomplete")
    if not coverage["isr_paths_covered"]:
        failures.append("coverage:isr_paths_uncovered")
    for row in rows:
        failures.extend(f"resource:{row['name']}:{reason}" for reason in row["unsafe_reasons"])
    failures.sort()
    safe = not failures

    proposed = []
    if safe:
        proposed = [
            {
                "name": row["name"],
                "kind": row["kind"],
                "declared": row["recommended"],
                "granularity": row["granularity"],
            }
            for row in rows
        ]
    recommended_id = declaration_identity(proposed) if proposed else None
    stack_saved = (
        sum(-row["delta"] for row in rows if row["kind"] == "stack_bytes" and row["delta"] < 0)
        if safe
        else 0
    )
    return {
        "schema": RESULT_SCHEMA,
        "workload_id": report["workload_id"],
        "source_declaration_id": report["declaration_id"],
        "recommended_declaration_id": recommended_id,
        "margin_percent": report["margin_percent"],
        "unseen_path_reserve_percent": coverage["unseen_path_reserve_percent"],
        "rows": rows,
        "proposed_declarations": proposed,
        "stack_bytes_reclaimable": stack_saved,
        "failure_reasons": failures,
        "safe": safe,
        "source_rewritten": False,
    }


def render(result: dict[str, Any]) -> str:
    lines = [
        "capacity right-sizing",
        f"  workload: {result['workload_id']}",
        f"  declarations: {result['source_declaration_id']}",
        f"  margin: {result['margin_percent']}% + "
        f"{result['unseen_path_reserve_percent']}% unseen-path reserve",
        "",
        f"  {'resource':22s} {'kind':12s} {'decl':>7s} {'peak':>7s} {'reco':>7s}  status",
    ]
    for row in result["rows"]:
        lines.append(
            f"  {row['name']:22.22s} {row['kind']:12s} {row['declared']:7d} "
            f"{row['observed_peak']:7d} {row['recommended']:7d}  {row['status']}"
            + (f"  ({row['delta']:+d})" if row["delta"] else "")
        )
    lines.extend(["", f"  stack bytes reclaimable: {result['stack_bytes_reclaimable']}"])
    if result["failure_reasons"]:
        lines.append("  blocked: " + ", ".join(result["failure_reasons"]))
        lines.append("  no declarations emitted")
    else:
        lines.append(f"  recommendation: {result['recommended_declaration_id']}")
        lines.append("  proposal only; source was not rewritten")
    lines.append(f"RESULT: {'PASS' if result['safe'] else 'FAIL'}")
    return "\n".join(lines)


def result_json(result: dict[str, Any]) -> str:
    """Stable machine output used by both CLIs."""
    return json.dumps(result, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def _write_result(path: pathlib.Path, serialized: str) -> None:
    """Atomically replace an explicitly requested machine-output file."""
    temporary: pathlib.Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            newline="\n",
            dir=path.parent,
            prefix=f".{path.name}.",
            suffix=".tmp",
            delete=False,
        ) as stream:
            temporary = pathlib.Path(stream.name)
            stream.write(serialized)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        temporary = None
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def _check_output_is_distinct(
    output_path: pathlib.Path | None, input_paths: list[pathlib.Path]
) -> None:
    if output_path is None:
        return
    output_resolved = output_path.resolve()
    for input_path in input_paths:
        input_resolved = input_path.resolve(strict=True)
        same_path = output_resolved == input_resolved
        same_file = output_path.exists() and os.path.samefile(output_path, input_resolved)
        if same_path or same_file:
            raise ValueError("output path must differ from every input")


def run_bindings(
    campaign_path: pathlib.Path,
    workload_path: pathlib.Path,
    build_manifest_path: pathlib.Path,
    output_path: pathlib.Path | None,
) -> int:
    try:
        _check_output_is_distinct(
            output_path, [campaign_path, workload_path, build_manifest_path]
        )
        bindings = capacity_bindings(
            load_campaign(campaign_path),
            workload_path.read_bytes(),
            build_manifest_path.read_bytes(),
        )
        serialized = result_json(bindings)
        if output_path is None:
            print(serialized, end="")
        else:
            _write_result(output_path, serialized)
            print(f"capacity bindings: {output_path}")
        print("RESULT: PASS")
        return 0
    except (OSError, json.JSONDecodeError, TypeError, ValueError) as exc:
        print(f"nobro shrink bindings: {exc}", file=sys.stderr)
        print("RESULT: FAIL")
        return 1


def run_device_report(
    binary_path: pathlib.Path,
    campaign_path: pathlib.Path,
    workload_path: pathlib.Path,
    build_manifest_path: pathlib.Path,
    output_path: pathlib.Path | None,
) -> int:
    try:
        _check_output_is_distinct(
            output_path,
            [binary_path, campaign_path, workload_path, build_manifest_path],
        )
        report = convert_capacity_binary(
            binary_path.read_bytes(),
            load_campaign(campaign_path),
            workload_path.read_bytes(),
            build_manifest_path.read_bytes(),
        )
        serialized = result_json(report)
        if output_path is None:
            print(serialized, end="")
        else:
            _write_result(output_path, serialized)
            print(f"capacity report: {output_path}")
        print("RESULT: PASS")
        return 0
    except (OSError, json.JSONDecodeError, TypeError, ValueError) as exc:
        print(f"nobro shrink device report: {exc}", file=sys.stderr)
        print("RESULT: FAIL")
        return 1


def run(report_path: pathlib.Path, output_path: pathlib.Path | None = None) -> int:
    """Run the analyzer without ever modifying the source report."""
    try:
        report_resolved = report_path.resolve(strict=True)
        if output_path is not None:
            same_path = output_path.resolve() == report_resolved
            same_file = output_path.exists() and os.path.samefile(output_path, report_resolved)
            if same_path or same_file:
                raise ValueError("output path must differ from the source report")
        result = analyze(load_report(report_path))
        serialized = result_json(result)
        if output_path is not None:
            _write_result(output_path, serialized)
    except (OSError, json.JSONDecodeError, TypeError, ValueError) as exc:
        print(f"nobro shrink: {exc}", file=sys.stderr)
        print("RESULT: FAIL")
        return 1
    print(render(result))
    return 0 if result["safe"] else 1


def _sample_report() -> dict[str, Any]:
    resources = [
        {
            "name": "control.stack",
            "kind": "stack_bytes",
            "declared": 1024,
            "observed_peak": 240,
            "granularity": 8,
            "saturated": False,
            "dropped": False,
        },
        {
            "name": "imu-to-control",
            "kind": "queue_slots",
            "declared": 8,
            "observed_peak": 2,
            "granularity": 1,
            "saturated": False,
            "dropped": False,
        },
    ]
    return {
        "schema": REPORT_SCHEMA,
        "workload_id": "sha256:" + "a" * 64,
        "declaration_id": declaration_identity(resources),
        "margin_percent": 25,
        "coverage": {
            "workload_complete": True,
            "isr_paths_covered": True,
            "unseen_path_reserve_percent": 10,
        },
        "resources": resources,
    }


def _sample_campaign() -> dict[str, Any]:
    return {
        "schema": CAMPAIGN_SCHEMA,
        "session_id": 7,
        "margin_percent": 25,
        "unseen_path_reserve_percent": 10,
        "required_path_mask": "0x0000000000000003",
        "isr_path_mask": "0x0000000000000002",
        "resources": [
            {
                "name": "control.stack",
                "kind": "stack_bytes",
                "declared": 1024,
                "granularity": 8,
            },
            {
                "name": "kernel.mailbox",
                "kind": "queue_slots",
                "declared": 8,
                "granularity": 1,
            },
            {
                "name": "sample.pool",
                "kind": "pool_slots",
                "declared": 8,
                "granularity": 1,
            },
        ],
    }


def _sample_capacity_binary(
    campaign: dict[str, Any],
    workload_bytes: bytes,
    build_manifest_bytes: bytes,
    *,
    observed_paths: int = 0b11,
    observed_isr_paths: int = 0b10,
    observations: dict[str, tuple[int, bool, bool, int]] | None = None,
    resource_capacity_extra: int = 1,
    completed: int = 1,
    flags: int = 0,
) -> bytes:
    campaign = _validated_campaign(campaign)
    bindings = capacity_bindings(campaign, workload_bytes, build_manifest_bytes)
    if observations is None:
        observations = {
            "control.stack": (240, False, False, 0),
            "kernel.mailbox": (2, False, False, 0),
            "sample.pool": (3, False, False, 0),
        }
    resource_count = len(bindings["resources"])
    resource_capacity = resource_count + resource_capacity_extra
    report_bytes = (
        CAPACITY_REPORT_FIXED_BYTES
        + resource_capacity * CAPACITY_RESOURCE_RECORD_BYTES
    )
    required_paths = int(campaign["required_path_mask"], 16)
    required_isr_paths = int(campaign["isr_path_mask"], 16)
    header = struct.pack(
        f"<{CAPACITY_REPORT_HEADER_WORDS}I",
        CAPACITY_REPORT_MAGIC,
        CAPACITY_REPORT_VERSION,
        report_bytes,
        completed,
        flags,
        campaign["session_id"],
        resource_count,
        resource_capacity,
        campaign["margin_percent"],
        campaign["unseen_path_reserve_percent"],
        1,
        int(observed_paths & required_paths == required_paths),
        int(observed_isr_paths & required_isr_paths == required_isr_paths),
        required_paths & 0xFFFFFFFF,
        required_paths >> 32,
        observed_paths & 0xFFFFFFFF,
        observed_paths >> 32,
        required_isr_paths & 0xFFFFFFFF,
        required_isr_paths >> 32,
        observed_isr_paths & 0xFFFFFFFF,
        observed_isr_paths >> 32,
    )
    identities = b"".join(
        _identity_bytes(bindings[field])
        for field in ("build_id", "workload_id", "declaration_id")
    )
    kind_codes = {kind: code for code, kind in CAPACITY_RESOURCE_KINDS.items()}
    records = bytearray()
    for resource in bindings["resources"]:
        peak, saturated, dropped, failure_count = observations[resource["name"]]
        records.extend(_identity_bytes(resource["resource_id"]))
        records.extend(
            struct.pack(
                "<7I",
                kind_codes[resource["kind"]],
                resource["declared"],
                peak,
                resource["granularity"],
                int(saturated),
                int(dropped),
                failure_count,
            )
        )
    records.extend(b"\0" * (resource_capacity_extra * CAPACITY_RESOURCE_RECORD_BYTES))
    body = header + identities + records
    # Explicit (not `assert`) so the invariant holds under `python -O` too.
    if len(body) + 4 != report_bytes:
        raise ValueError(
            f"sample capacity binary is {len(body) + 4} bytes, declared {report_bytes}"
        )
    return body + struct.pack("<I", _fnv1a32(body))


def _resign_capacity_binary(data: bytearray) -> bytes:
    struct.pack_into("<I", data, len(data) - 4, _fnv1a32(data[:-4]))
    return bytes(data)


def selftest() -> int:
    # Safe recommendations are sorted, identity-bound, aligned, and deterministic.
    sample = _sample_report()
    first = analyze(sample)
    second = analyze(json.loads(json.dumps(sample)))
    assert first == second and result_json(first) == result_json(second)
    assert first["safe"] and first["source_rewritten"] is False
    assert [row["name"] for row in first["rows"]] == ["control.stack", "imu-to-control"]
    assert first["rows"][0]["recommended"] == 328
    assert first["rows"][0]["recommended"] % 8 == 0
    assert first["recommended_declaration_id"].startswith("sha256:")
    assert first["proposed_declarations"]

    # Every unsafe evidence flag fails closed and suppresses declaration output.
    for mutate, reason in (
        (lambda report: report["coverage"].update(workload_complete=False), "workload_incomplete"),
        (lambda report: report["coverage"].update(isr_paths_covered=False), "isr_paths_uncovered"),
        (lambda report: report["resources"][0].update(saturated=True), "counter_saturated"),
        (lambda report: report["resources"][0].update(dropped=True), "events_dropped"),
        (lambda report: report["resources"][0].update(observed_peak=1024), "reached_declaration"),
    ):
        hostile = json.loads(json.dumps(sample))
        mutate(hostile)
        result = analyze(hostile)
        assert not result["safe"] and not result["proposed_declarations"], result
        assert result["recommended_declaration_id"] is None
        assert reason in " ".join(result["failure_reasons"]), result

    # Adversarial schema mutations must be rejected rather than coerced.
    mutations = []
    mutations.append(lambda report: report.update(schema="nobro-shrink-report-v0"))
    mutations.append(lambda report: report.update(extra=True))
    mutations.append(lambda report: report.pop("coverage"))
    mutations.append(lambda report: report.update(resources=[]))
    mutations.append(lambda report: report["resources"].append(dict(report["resources"][0])))
    mutations.append(lambda report: report.update(margin_percent=True))
    mutations.append(lambda report: report.update(margin_percent="25"))
    mutations.append(lambda report: report.update(margin_percent=MAX_PERCENT + 1))
    mutations.append(lambda report: report["coverage"].update(unseen_path_reserve_percent=0))
    mutations.append(lambda report: report["coverage"].update(unseen_path_reserve_percent=True))
    mutations.append(lambda report: report["resources"][0].update(declared=0))
    mutations.append(lambda report: report["resources"][0].update(declared=True))
    mutations.append(lambda report: report["resources"][0].update(observed_peak="1"))
    mutations.append(lambda report: report["resources"][0].update(granularity=0))
    mutations.append(lambda report: report["resources"][0].update(granularity=-1))
    mutations.append(lambda report: report["resources"][0].update(saturated=0))
    mutations.append(lambda report: report.update(declaration_id="sha256:" + "0" * 64))
    for mutate in mutations:
        hostile = json.loads(json.dumps(sample))
        mutate(hostile)
        try:
            analyze(hostile)
            raise AssertionError(f"malformed report was accepted: {hostile}")
        except ValueError:
            pass

    # JSON duplicate keys and attempts to overwrite the input fail without changing it.
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        duplicate = root / "duplicate.json"
        duplicate.write_text('{"schema":"a","schema":"b"}\n', encoding="utf-8")
        try:
            load_report(duplicate)
            raise AssertionError("duplicate JSON key was accepted")
        except ValueError:
            pass
        source = root / "report.json"
        source.write_text(json.dumps(sample), encoding="utf-8")
        before = source.read_bytes()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            assert run(source, source) == 1
        assert source.read_bytes() == before

        # A target-style fixed report is identity-bound to the exact build
        # manifest, workload+coverage contract, and full resource registry.
        campaign = _sample_campaign()
        campaign_path = root / "campaign.json"
        workload_path = root / "workload.json"
        build_path = root / "build.json"
        binary_path = root / "capacity.bin"
        converted_path = root / "converted.json"
        campaign_path.write_text(json.dumps(campaign), encoding="utf-8")
        workload_path.write_bytes(b'{"tasks":["sensor","control"]}\n')
        build_path.write_bytes(b'{"toolchain":"pinned"}\n')
        binary = _sample_capacity_binary(
            campaign, workload_path.read_bytes(), build_path.read_bytes()
        )
        binary_path.write_bytes(binary)
        converted = convert_capacity_binary(
            binary, campaign, workload_path.read_bytes(), build_path.read_bytes()
        )
        converted_result = analyze(converted)
        assert converted_result["safe"] and converted_result["proposed_declarations"]
        with contextlib.redirect_stdout(io.StringIO()):
            assert run_device_report(
                binary_path,
                campaign_path,
                workload_path,
                build_path,
                converted_path,
            ) == 0
        assert load_report(converted_path) == converted

        project_cli = pathlib.Path(__file__).with_name("nobro_project.py")
        project_bindings = root / "project-bindings.json"
        project_decoded = root / "project-decoded.json"
        for command, expected_path, expected_value in (
            (
                [
                    sys.executable,
                    str(project_cli),
                    "shrink",
                    "--bindings",
                    "--campaign",
                    str(campaign_path),
                    "--workload",
                    str(workload_path),
                    "--build-manifest",
                    str(build_path),
                    "--json",
                    str(project_bindings),
                ],
                project_bindings,
                capacity_bindings(
                    campaign, workload_path.read_bytes(), build_path.read_bytes()
                ),
            ),
            (
                [
                    sys.executable,
                    str(project_cli),
                    "shrink",
                    "--device-report",
                    str(binary_path),
                    "--campaign",
                    str(campaign_path),
                    "--workload",
                    str(workload_path),
                    "--build-manifest",
                    str(build_path),
                    "--json",
                    str(project_decoded),
                ],
                project_decoded,
                converted,
            ),
        ):
            completed = subprocess.run(command, capture_output=True, text=True, check=False)
            assert completed.returncode == 0, completed.stdout + completed.stderr
            assert load_report(expected_path) == expected_value

        # Unsafe target evidence still converts, then the existing analyzer
        # suppresses every proposed declaration.
        unsafe_observations = {
            "control.stack": (240, True, False, 0),
            "kernel.mailbox": (8, False, True, 1),
            "sample.pool": (3, False, False, 0),
        }
        for unsafe_binary in (
            _sample_capacity_binary(
                campaign,
                workload_path.read_bytes(),
                build_path.read_bytes(),
                observations=unsafe_observations,
            ),
            _sample_capacity_binary(
                campaign,
                workload_path.read_bytes(),
                build_path.read_bytes(),
                observed_paths=0b01,
                observed_isr_paths=0,
            ),
        ):
            unsafe_report = convert_capacity_binary(
                unsafe_binary,
                campaign,
                workload_path.read_bytes(),
                build_path.read_bytes(),
            )
            unsafe_result = analyze(unsafe_report)
            assert not unsafe_result["safe"]
            assert unsafe_result["proposed_declarations"] == []
            assert unsafe_result["recommended_declaration_id"] is None

        # Corruption, an incomplete producer, identity/declaration drift, and
        # hidden nonzero records all fail before a v1 report can be emitted.
        hostile_binaries: list[bytes] = []
        corrupt = bytearray(binary)
        corrupt[0] ^= 1
        hostile_binaries.append(bytes(corrupt))
        incomplete = bytearray(binary)
        struct.pack_into("<I", incomplete, 3 * 4, 0)
        hostile_binaries.append(_resign_capacity_binary(incomplete))
        wrong_identity = bytearray(binary)
        wrong_identity[CAPACITY_REPORT_HEADER_WORDS * 4 + 64] ^= 1
        hostile_binaries.append(_resign_capacity_binary(wrong_identity))
        wrong_declaration = bytearray(binary)
        first_record = CAPACITY_REPORT_HEADER_WORDS * 4 + 96
        declared_offset = first_record + 32 + 4
        struct.pack_into(
            "<I",
            wrong_declaration,
            declared_offset,
            struct.unpack_from("<I", wrong_declaration, declared_offset)[0] + 1,
        )
        hostile_binaries.append(_resign_capacity_binary(wrong_declaration))
        hidden = bytearray(binary)
        unused = first_record + len(campaign["resources"]) * CAPACITY_RESOURCE_RECORD_BYTES
        hidden[unused] = 1
        hostile_binaries.append(_resign_capacity_binary(hidden))
        for hostile_binary in hostile_binaries:
            try:
                convert_capacity_binary(
                    hostile_binary,
                    campaign,
                    workload_path.read_bytes(),
                    build_path.read_bytes(),
                )
                raise AssertionError("hostile capacity report was accepted")
            except ValueError:
                pass

        # Producer output cannot alias any source, including through a hardlink.
        binary_alias = root / "capacity-alias.bin"
        os.link(binary_path, binary_alias)
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            assert run_device_report(
                binary_path,
                campaign_path,
                workload_path,
                build_path,
                binary_alias,
            ) == 1
        assert binary_path.read_bytes() == binary

        duplicate_campaign = root / "duplicate-campaign.json"
        duplicate_campaign.write_text(
            '{"schema":"a","schema":"b"}\n', encoding="utf-8"
        )
        try:
            load_campaign(duplicate_campaign)
            raise AssertionError("duplicate campaign key was accepted")
        except ValueError:
            pass
        hardlink = root / "report-hardlink.json"
        os.link(source, hardlink)
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            assert run(source, hardlink) == 1
        assert source.read_bytes() == before

        # The public `nobro project shrink` dispatcher produces the same bound
        # proposal without touching its source report.
        output = root / "proposal.json"
        command = [
            sys.executable,
            str(pathlib.Path(__file__).with_name("nobro_project.py")),
            "shrink",
            str(source),
            "--json",
            str(output),
        ]
        completed = subprocess.run(command, capture_output=True, text=True, check=False)
        assert completed.returncode == 0, completed.stdout + completed.stderr
        assert output.read_text(encoding="utf-8") == result_json(analyze(sample))
        assert source.read_bytes() == before

    print(
        "NOBRO SHRINK SELFTEST: PASS "
        "(schema/types/identity/real-producer/coverage/saturation/drop/reserve/output)"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("report", nargs="?", type=pathlib.Path, help="occupancy report JSON")
    parser.add_argument("--json", type=pathlib.Path, metavar="FILE", help="write proposal JSON")
    parser.add_argument(
        "--bindings", action="store_true", help="derive firmware campaign identities"
    )
    parser.add_argument(
        "--device-report",
        type=pathlib.Path,
        metavar="REPORT.BIN",
        help="decode a report captured from firmware",
    )
    parser.add_argument("--campaign", type=pathlib.Path, metavar="FILE")
    parser.add_argument("--workload", type=pathlib.Path, metavar="FILE")
    parser.add_argument("--build-manifest", type=pathlib.Path, metavar="FILE")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()
    producer_inputs = (args.campaign, args.workload, args.build_manifest)
    if args.bindings or args.device_report is not None:
        if args.report is not None:
            parser.error("the positional report cannot be combined with producer modes")
        if args.bindings and args.device_report is not None:
            parser.error("choose either --bindings or --device-report")
        if any(path is None for path in producer_inputs):
            parser.error("producer modes require --campaign, --workload, and --build-manifest")
        if args.bindings:
            return run_bindings(
                args.campaign, args.workload, args.build_manifest, args.json
            )
        return run_device_report(
            args.device_report,
            args.campaign,
            args.workload,
            args.build_manifest,
            args.json,
        )
    if any(path is not None for path in producer_inputs):
        parser.error("--campaign/--workload/--build-manifest require a producer mode")
    if args.report is None:
        parser.error("a report file is required (or use --selftest)")
    return run(args.report, args.json)


if __name__ == "__main__":
    sys.exit(main())
