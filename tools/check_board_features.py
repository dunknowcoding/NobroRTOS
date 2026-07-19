#!/usr/bin/env python3
"""Validate the data-driven board-feature provider extension registry."""

from __future__ import annotations

import argparse
import copy
import json
import pathlib
import re
import sys


ROOT = pathlib.Path(__file__).resolve().parents[1]
REGISTRY = ROOT / "core" / "boards" / "feature_providers.json"
CATALOG = ROOT / "core" / "adapters" / "catalog.json"
SCHEMA = "nobro-board-feature-registry-v2"
NAME = re.compile(r"^[a-z][a-z0-9]*(?:[-_][a-z0-9]+)*$")
FINGERPRINT = re.compile(r"^[0-9a-f]{16}$")
CLASSES = {"peripheral", "connectivity"}
FIXED_PRICE_FIELDS = {
    "flash_bytes",
    "static_ram_bytes",
    "retained_heap_bytes",
    "stack_bytes",
    "vendor_reserved_ram_bytes",
    "worker_threads",
    "interrupt_slots",
    "dma_channels",
    "controller_firmware_bytes",
    "peripheral_channels",
}
RUNTIME_PRICE_FIELDS = {
    "transient_heap_peak_bytes",
    "stack_high_water_bytes",
    "cpu_cycles_per_second",
    "latency_p99_cycles",
    "latency_max_cycles",
}
COEXISTENCE_FIELDS = {
    "leases",
    "exclusive_resources",
    "compatible_instances",
    "core_affinity",
}


def _duplicates(values: list[str]) -> set[str]:
    seen: set[str] = set()
    return {value for value in values if value in seen or seen.add(value)}


def _names(records: object, label: str, errors: list[str]) -> dict[str, dict]:
    if not isinstance(records, list):
        errors.append(f"{label}: expected a list")
        return {}
    result: dict[str, dict] = {}
    for index, record in enumerate(records):
        if not isinstance(record, dict):
            errors.append(f"{label}[{index}]: expected an object")
            continue
        identifier = record.get("id")
        if not isinstance(identifier, str) or not NAME.fullmatch(identifier):
            errors.append(f"{label}[{index}]: invalid id")
            continue
        if identifier in result:
            errors.append(f"{label}: duplicate id {identifier!r}")
        result[identifier] = record
    return result


def validate(registry: dict, catalog: dict) -> list[str]:
    errors: list[str] = []
    if registry.get("schema") != SCHEMA:
        errors.append(f"schema must be {SCHEMA!r}")
    deployment = registry.get("deployment_values")
    maturities = registry.get("maturity_values")
    evidence_values = registry.get("evidence_values")
    for label, actual, expected in (
        ("deployment_values", deployment, ["firmware", "host"]),
        ("maturity_values", maturities, ["absent", "stub", "compile-only", "implemented"]),
        ("evidence_values", evidence_values, ["host-test", "target-build", "physical"]),
    ):
        if actual != expected:
            errors.append(f"{label}: contract differs from {expected!r}")
    deployment_set = set(deployment) if isinstance(deployment, list) else set()
    maturity_set = set(maturities) if isinstance(maturities, list) else set()
    evidence_set = set(evidence_values) if isinstance(evidence_values, list) else set()
    if set(registry.get("fixed_price_dimensions", [])) != FIXED_PRICE_FIELDS:
        errors.append("fixed_price_dimensions: incomplete or unknown dimensions")
    if set(registry.get("runtime_price_dimensions", [])) != RUNTIME_PRICE_FIELDS:
        errors.append("runtime_price_dimensions: incomplete or unknown dimensions")
    if set(registry.get("coexistence_dimensions", [])) != COEXISTENCE_FIELDS:
        errors.append("coexistence_dimensions: incomplete or unknown dimensions")

    kinds = _names(registry.get("capability_kinds"), "capability_kinds", errors)
    provenance = _names(registry.get("provenance"), "provenance", errors)
    backends = _names(registry.get("backends"), "backends", errors)
    bindings = _names(registry.get("bindings"), "bindings", errors)
    contracts = {
        item.get("id")
        for item in catalog.get("components", [])
        if isinstance(item, dict) and item.get("kind") == "contract"
    }
    components = {
        item.get("id"): item
        for item in catalog.get("components", [])
        if isinstance(item, dict) and isinstance(item.get("id"), str)
    }
    for identifier, kind in kinds.items():
        prefix = f"capability_kinds.{identifier}"
        if kind.get("class") not in CLASSES:
            errors.append(f"{prefix}: invalid class")
        if kind.get("portable_contract_id") not in contracts:
            errors.append(f"{prefix}: portable contract is not in adapter catalog")
        if not isinstance(kind.get("stack_family"), str) or not NAME.fullmatch(
            kind["stack_family"]
        ):
            errors.append(f"{prefix}: invalid stack family")
        limitations = kind.get("limitations")
        if not isinstance(limitations, list) or not limitations or not all(
            isinstance(value, str) and value for value in limitations
        ):
            errors.append(f"{prefix}: limitations must be a non-empty string list")

    for identifier, backend in backends.items():
        prefix = f"backends.{identifier}"
        kind = kinds.get(backend.get("capability_kind"))
        if kind is None:
            errors.append(f"{prefix}: unknown capability kind")
        elif backend.get("stack_family") != kind.get("stack_family"):
            errors.append(f"{prefix}: stack family differs from capability kind")
        component = components.get(backend.get("adapter_component_id"))
        if not isinstance(component, dict) or component.get("kind") != "adapter":
            errors.append(f"{prefix}: adapter component is not in catalog")
        if backend.get("deployment") not in deployment_set:
            errors.append(f"{prefix}: invalid deployment")
        if backend.get("maturity") not in maturity_set:
            errors.append(f"{prefix}: invalid maturity")
        provenance_id = backend.get("provenance_id")
        if provenance_id is not None and provenance_id not in provenance:
            errors.append(f"{prefix}: unknown provenance")
        targets = backend.get("supported_targets")
        if not isinstance(targets, list) or _duplicates(targets) or not all(
            isinstance(value, str) and value for value in targets
        ):
            errors.append(f"{prefix}: supported_targets must be a unique string list")
        evidence = backend.get("evidence")
        if not isinstance(evidence, list) or _duplicates(evidence):
            errors.append(f"{prefix}: evidence must be a unique list")
        elif any(value not in evidence_set for value in evidence):
            errors.append(f"{prefix}: invalid evidence kind")
        limitations = backend.get("limitations")
        if not isinstance(limitations, list) or not limitations:
            errors.append(f"{prefix}: limitations are required")

    seen_instances: set[tuple[str, str, str]] = set()
    for identifier, binding in bindings.items():
        prefix = f"bindings.{identifier}"
        backend = backends.get(binding.get("backend_id"))
        if backend is None:
            errors.append(f"{prefix}: unknown backend")
            continue
        if binding.get("capability_kind") != backend.get("capability_kind"):
            errors.append(f"{prefix}: capability kind differs from backend")
        platform = binding.get("platform")
        composition = binding.get("composition")
        instance = binding.get("instance")
        if not all(isinstance(value, str) and NAME.fullmatch(value) for value in (
            platform, composition, instance
        )):
            errors.append(f"{prefix}: invalid platform/composition/instance")
        else:
            key = (platform, composition, instance)
            if key in seen_instances:
                errors.append(f"{prefix}: duplicate logical instance")
            seen_instances.add(key)
        if binding.get("maturity") not in maturity_set:
            errors.append(f"{prefix}: invalid maturity")
        evidence_gates = binding.get("evidence_gates")
        if not isinstance(evidence_gates, list) or _duplicates(evidence_gates) or not all(
            isinstance(value, str) and value for value in evidence_gates
        ):
            errors.append(f"{prefix}: evidence_gates must be a unique string list")
        workload = binding.get("workload")
        if (
            not isinstance(workload, dict)
            or set(workload) != {"configuration_fingerprint", "operations_per_second"}
            or not isinstance(workload.get("configuration_fingerprint"), str)
            or not FINGERPRINT.fullmatch(workload["configuration_fingerprint"])
            or not isinstance(workload.get("operations_per_second"), int)
            or workload["operations_per_second"] <= 0
        ):
            errors.append(f"{prefix}: workload identity and positive operation rate are required")
        fixed_price = binding.get("measured_fixed_price")
        if (
            not isinstance(fixed_price, dict)
            or set(fixed_price) != FIXED_PRICE_FIELDS
            or any(not isinstance(value, int) or value < 0 for value in fixed_price.values())
        ):
            errors.append(
                f"{prefix}: measured_fixed_price must contain every non-negative dimension"
            )
        fixed_provenance = binding.get("fixed_price_provenance")
        if (
            not isinstance(fixed_provenance, dict)
            or set(fixed_provenance) != FIXED_PRICE_FIELDS
            or any(
                value not in {"measured", "declared-zero"}
                for value in fixed_provenance.values()
            )
        ):
            errors.append(
                f"{prefix}: fixed_price_provenance must classify every dimension as "
                "measured or declared-zero"
            )
        elif isinstance(fixed_price, dict) and any(
            fixed_price.get(field) != 0
            and fixed_provenance[field] == "declared-zero"
            for field in FIXED_PRICE_FIELDS
        ):
            errors.append(f"{prefix}: declared-zero fixed price has a non-zero value")
        runtime_price = binding.get("measured_runtime_price")
        if (
            not isinstance(runtime_price, dict)
            or set(runtime_price) != RUNTIME_PRICE_FIELDS
            or any(
                not isinstance(value, int) or value < 0
                for value in runtime_price.values()
            )
        ):
            errors.append(
                f"{prefix}: measured_runtime_price must contain every non-negative dimension"
            )
        elif runtime_price["latency_p99_cycles"] > runtime_price["latency_max_cycles"]:
            errors.append(f"{prefix}: p99 latency exceeds maximum latency")
        runtime_provenance = binding.get("runtime_price_provenance")
        if (
            not isinstance(runtime_provenance, dict)
            or set(runtime_provenance) != RUNTIME_PRICE_FIELDS
            or any(
                value not in {"measured", "declared-zero"}
                for value in runtime_provenance.values()
            )
        ):
            errors.append(
                f"{prefix}: runtime_price_provenance must classify every dimension as "
                "measured or declared-zero"
            )
        elif isinstance(runtime_price, dict) and any(
            runtime_price.get(field) != 0
            and runtime_provenance[field] == "declared-zero"
            for field in RUNTIME_PRICE_FIELDS
        ):
            errors.append(f"{prefix}: declared-zero runtime price has a non-zero value")
        coexistence = binding.get("coexistence")
        if not isinstance(coexistence, dict) or set(coexistence) != COEXISTENCE_FIELDS or any(
            not isinstance(value, list)
            or not all(isinstance(item, str) and item for item in value)
            for value in coexistence.values()
        ):
            errors.append(f"{prefix}: coexistence must contain every string-list dimension")
        gate = binding.get("disabled_symbol_gate")
        if not isinstance(gate, dict) or set(gate) != {
            "baseline",
            "feature",
            "forbidden_symbols",
            "max_flash_delta_bytes",
            "max_ram_delta_bytes",
        }:
            errors.append(f"{prefix}: disabled_symbol_gate is incomplete")
        elif (
            not isinstance(gate.get("baseline"), str)
            or not isinstance(gate.get("feature"), str)
            or not isinstance(gate.get("forbidden_symbols"), list)
            or not all(isinstance(value, str) and value for value in gate["forbidden_symbols"])
            or gate.get("max_flash_delta_bytes") != 0
            or gate.get("max_ram_delta_bytes") != 0
        ):
            errors.append(f"{prefix}: disabled gate must prove zero delta and forbidden symbols")
        report = binding.get("report_wiring")
        if not isinstance(report, dict) or not all(
            isinstance(report.get(field), str) and report[field]
            for field in ("provider_id", "status_field", "evidence_gate")
        ):
            errors.append(f"{prefix}: report wiring is incomplete")
        elif (
            report["provider_id"] != binding.get("capability_kind")
            or report["evidence_gate"] not in (evidence_gates or [])
        ):
            errors.append(f"{prefix}: report wiring differs from capability/evidence")
        if binding.get("maturity") == "implemented" and not evidence_gates:
            errors.append(f"{prefix}: implemented binding needs evidence gates")
    return errors


def capability_ids(registry: dict) -> set[str]:
    return {
        item["id"]
        for item in registry.get("capability_kinds", [])
        if isinstance(item, dict) and isinstance(item.get("id"), str)
    }


def selftest() -> int:
    registry = json.loads(REGISTRY.read_text(encoding="utf-8"))
    catalog = json.loads(CATALOG.read_text(encoding="utf-8"))
    assert not validate(registry, catalog)
    broken = copy.deepcopy(registry)
    broken["fixed_price_dimensions"].remove("retained_heap_bytes")
    assert any("fixed_price_dimensions" in error for error in validate(broken, catalog))
    broken = copy.deepcopy(registry)
    broken["runtime_price_dimensions"].remove("transient_heap_peak_bytes")
    assert any("runtime_price_dimensions" in error for error in validate(broken, catalog))
    broken = copy.deepcopy(registry)
    broken["capability_kinds"][0]["portable_contract_id"] = "missing"
    assert any("portable contract" in error for error in validate(broken, catalog))
    binding = {
        "id": "selftest-binding",
        "backend_id": registry["backends"][0]["id"],
        "capability_kind": registry["backends"][0]["capability_kind"],
        "platform": "esp32s3",
        "composition": "arduino",
        "instance": "audio0",
        "maturity": "compile-only",
        "evidence_gates": ["selftest-gate"],
        "workload": {
            "configuration_fingerprint": "0123456789abcdef",
            "operations_per_second": 100,
        },
        "measured_fixed_price": {field: 0 for field in FIXED_PRICE_FIELDS},
        "fixed_price_provenance": {
            field: "declared-zero" for field in FIXED_PRICE_FIELDS
        },
        "measured_runtime_price": {field: 0 for field in RUNTIME_PRICE_FIELDS},
        "runtime_price_provenance": {
            field: "declared-zero" for field in RUNTIME_PRICE_FIELDS
        },
        "coexistence": {field: [] for field in COEXISTENCE_FIELDS},
        "disabled_symbol_gate": {
            "baseline": "selftest-baseline",
            "feature": "audio_i2s",
            "forbidden_symbols": ["selftest_backend"],
            "max_flash_delta_bytes": 0,
            "max_ram_delta_bytes": 0,
        },
        "report_wiring": {
            "provider_id": registry["backends"][0]["capability_kind"],
            "status_field": "audio0",
            "evidence_gate": "selftest-gate",
        },
    }
    priced = copy.deepcopy(registry)
    priced["bindings"].append(binding)
    assert not validate(priced, catalog)
    broken = copy.deepcopy(priced)
    broken["bindings"][0]["fixed_price_provenance"].pop("retained_heap_bytes")
    assert any("fixed_price_provenance" in error for error in validate(broken, catalog))
    broken = copy.deepcopy(priced)
    broken["bindings"][0]["measured_runtime_price"]["transient_heap_peak_bytes"] = 1
    assert any("declared-zero runtime" in error for error in validate(broken, catalog))
    broken = copy.deepcopy(priced)
    broken["bindings"][0]["workload"]["operations_per_second"] = 0
    assert any("operation rate" in error for error in validate(broken, catalog))
    broken = copy.deepcopy(priced)
    broken["bindings"][0]["measured_runtime_price"]["latency_p99_cycles"] = 2
    broken["bindings"][0]["measured_runtime_price"]["latency_max_cycles"] = 1
    broken["bindings"][0]["runtime_price_provenance"]["latency_p99_cycles"] = "measured"
    broken["bindings"][0]["runtime_price_provenance"]["latency_max_cycles"] = "measured"
    assert any("p99 latency" in error for error in validate(broken, catalog))
    print(
        "BOARD FEATURES SELFTEST: PASS "
        "(vocabulary, backend, workload, fixed/runtime price, zero-delta)"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    registry = json.loads(REGISTRY.read_text(encoding="utf-8"))
    catalog = json.loads(CATALOG.read_text(encoding="utf-8"))
    errors = validate(registry, catalog)
    if errors:
        for error in errors:
            print(f"BOARD FEATURES: {error}")
        print("RESULT: FAIL")
        return 1
    print(
        "BOARD FEATURES: PASS "
        f"({len(capability_ids(registry))} kinds, "
        f"{len(registry['backends'])} backends, {len(registry['bindings'])} bindings)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
