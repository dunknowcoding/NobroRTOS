#!/usr/bin/env python3
"""Validate catalog-v2 ownership, provenance, and domain relationships."""

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
CATALOG = ROOT / "core" / "adapters" / "catalog.json"
LAYOUT = ROOT / "core" / "layout.json"

DEPLOYMENTS = {"firmware", "host"}
MATURITY = {"absent", "stub", "compile-only", "implemented"}
EVIDENCE = {"host-test", "target-build", "physical"}
KINDS = {"contract", "adapter", "library", "host-product"}
IDENTIFIER = re.compile(r"^[a-z][a-z0-9-]*$")
REVISION = re.compile(r"^[0-9a-f]{40}$")
SUPPORT_STATES = [
    "cataloged",
    "compiled",
    "implemented",
    "physical",
    "promoted",
    "released",
]
SUPPORT_TRANSITIONS = [
    "cataloged->compiled",
    "compiled->implemented",
    "implemented->physical",
    "physical->promoted",
    "promoted->released",
]
SUPPORT_STATE_OWNERS = {
    "cataloged": "core/adapters/catalog.json",
    "compiled": "core/adapters/catalog.json",
    "implemented": "core/adapters/catalog.json",
    "physical": "core/boards/feature_providers.json exact bindings",
    "promoted": "core/boards/feature_providers.json exact bindings",
    "released": "versioned package and release manifests",
}
COMPONENT_STAGE_MAP = {
    "absent": "cataloged",
    "stub": "cataloged",
    "compile-only": "compiled",
    "implemented": "implemented",
}


def _sorted_unique_strings(value: object, *, allow_empty: bool = True) -> bool:
    return (
        isinstance(value, list)
        and (allow_empty or bool(value))
        and all(isinstance(item, str) and item for item in value)
        and value == sorted(set(value), key=str.casefold)
    )


def _support_model_errors(value: object) -> list[str]:
    if not isinstance(value, dict):
        return ["support_model must be an object"]
    errors: list[str] = []
    if value.get("schema") != "nobro-support-state-v1":
        errors.append("support_model has an unknown schema")
    if value.get("states") != SUPPORT_STATES:
        errors.append("support_model states must preserve the canonical order")
    if value.get("transitions") != SUPPORT_TRANSITIONS:
        errors.append("support_model transitions must preserve the canonical chain")
    if value.get("state_owners") != SUPPORT_STATE_OWNERS:
        errors.append("support_model state owners must remain explicit")
    if value.get("component_stage_map") != COMPONENT_STAGE_MAP:
        errors.append("support_model component stage map must remain explicit")
    if value.get("implicit_promotion") is not False:
        errors.append("support_model must reject implicit promotion")
    return errors


def _primary_domain_errors(
    components: dict[str, dict[str, object]],
    memberships: dict[str, set[str]],
    domain_ids: set[str],
) -> list[str]:
    errors: list[str] = []
    for component_id, component in components.items():
        primary = component.get("primary_domain")
        if not isinstance(primary, str) or primary not in domain_ids:
            errors.append(f"{component_id}: primary_domain is missing or unknown")
            continue
        if primary not in memberships.get(component_id, set()):
            errors.append(
                f"{component_id}: primary_domain {primary} lacks a domain relationship"
            )
    return errors


def _policy_selftest() -> list[str]:
    errors: list[str] = []
    good_model = {
        "schema": "nobro-support-state-v1",
        "states": list(SUPPORT_STATES),
        "transitions": list(SUPPORT_TRANSITIONS),
        "state_owners": dict(SUPPORT_STATE_OWNERS),
        "component_stage_map": dict(COMPONENT_STAGE_MAP),
        "implicit_promotion": False,
    }
    if _support_model_errors(good_model):
        errors.append("support-model self-test rejected the canonical model")
    bad_model = dict(good_model)
    bad_model["implicit_promotion"] = True
    if "support_model must reject implicit promotion" not in _support_model_errors(
        bad_model
    ):
        errors.append("support-model self-test accepted implicit promotion")
    bad_model = dict(good_model)
    bad_model["transitions"] = SUPPORT_TRANSITIONS[:-1]
    if not any(
        "canonical chain" in error for error in _support_model_errors(bad_model)
    ):
        errors.append("support-model self-test accepted an incomplete transition chain")

    components = {
        "adapter-demo": {"primary_domain": "sensors"},
        "library-demo": {"primary_domain": "wireless"},
    }
    memberships = {
        "adapter-demo": {"sensors"},
        "library-demo": {"sensors"},
    }
    domain_errors = _primary_domain_errors(
        components, memberships, {"sensors", "wireless"}
    )
    if not any("library-demo" in error for error in domain_errors):
        errors.append("primary-domain self-test accepted an unrelated owner")
    components["library-demo"].pop("primary_domain")
    domain_errors = _primary_domain_errors(
        components, memberships, {"sensors", "wireless"}
    )
    if not any("missing or unknown" in error for error in domain_errors):
        errors.append("primary-domain self-test accepted a missing owner")
    return errors


def validate() -> list[str]:
    errors = _policy_selftest()
    catalog = json.loads(CATALOG.read_text(encoding="utf-8"))
    layout = json.loads(LAYOUT.read_text(encoding="utf-8"))
    allowed_categories = set(layout["adapter_categories"])

    if catalog.get("schema") != "nobro-adapter-catalog-v2":
        errors.append("unexpected catalog schema")
    errors.extend(_support_model_errors(catalog.get("support_model")))

    provenance: dict[str, dict[str, object]] = {}
    for record in catalog.get("provenance", []):
        record_id = record.get("id")
        if not isinstance(record_id, str) or not IDENTIFIER.fullmatch(record_id):
            errors.append(f"invalid provenance id: {record_id!r}")
            continue
        if record_id in provenance:
            errors.append(f"duplicate provenance id: {record_id}")
            continue
        provenance[record_id] = record
        if not str(record.get("source", "")).startswith("https://"):
            errors.append(f"{record_id}: source must be a public HTTPS URL")
        if not REVISION.fullmatch(str(record.get("revision", ""))):
            errors.append(f"{record_id}: revision must be an immutable 40-hex commit")
        if not isinstance(record.get("version"), str) or not record["version"]:
            errors.append(f"{record_id}: version is required")
        if not isinstance(record.get("license"), str) or not record["license"]:
            errors.append(f"{record_id}: license is required")
        if record.get("pinned") is not True:
            errors.append(f"{record_id}: provenance must be pinned")
        if not isinstance(record.get("clean"), bool):
            errors.append(f"{record_id}: clean must be boolean")

    components: dict[str, dict[str, object]] = {}
    adapter_paths: dict[str, str] = {}
    provenance_users: set[str] = set()
    for component in catalog.get("components", []):
        component_id = component.get("id")
        if not isinstance(component_id, str) or not IDENTIFIER.fullmatch(component_id):
            errors.append(f"invalid component id: {component_id!r}")
            continue
        if component_id in components:
            errors.append(f"duplicate component id: {component_id}")
            continue
        components[component_id] = component
        kind = component.get("kind")
        if kind not in KINDS:
            errors.append(f"{component_id}: invalid kind {kind!r}")
        if component.get("deployment") not in DEPLOYMENTS:
            errors.append(f"{component_id}: invalid deployment")
        if component.get("maturity") not in MATURITY:
            errors.append(f"{component_id}: invalid maturity")
        evidence = component.get("evidence")
        if not _sorted_unique_strings(evidence) or not set(evidence).issubset(EVIDENCE):
            errors.append(f"{component_id}: invalid evidence")
        if not _sorted_unique_strings(component.get("supported_targets")):
            errors.append(f"{component_id}: invalid supported_targets")
        if not _sorted_unique_strings(component.get("limitations")):
            errors.append(f"{component_id}: invalid limitations")
        maturity = component.get("maturity")
        targets = component.get("supported_targets", [])
        if maturity == "absent" and (evidence or targets):
            errors.append(f"{component_id}: absent components cannot carry evidence or targets")
        if maturity == "compile-only" and (
            "target-build" not in evidence or not targets
        ):
            errors.append(f"{component_id}: compile-only requires target-build evidence and scope")
        if "physical" in evidence and maturity != "implemented":
            errors.append(f"{component_id}: physical evidence requires implemented maturity")

        path_value = component.get("path")
        if kind in {"contract", "adapter"}:
            if not isinstance(path_value, str) or not (ROOT / path_value / "Cargo.toml").is_file():
                errors.append(f"{component_id}: missing crate path {path_value!r}")
        elif path_value is not None:
            errors.append(f"{component_id}: external member must not own a source path")

        provenance_id = component.get("provenance_id")
        if kind in {"library", "host-product"}:
            if provenance_id not in provenance:
                errors.append(f"{component_id}: unresolved provenance {provenance_id!r}")
            else:
                provenance_users.add(str(provenance_id))
        elif provenance_id is not None:
            errors.append(f"{component_id}: internal component must not duplicate provenance")

        facade = component.get("facade")
        if facade and not (ROOT / str(facade)).is_file():
            errors.append(f"{component_id}: missing facade {facade}")
        for inventory in ("sensor_drivers", "board_modules"):
            values = component.get(inventory)
            if values is not None and not _sorted_unique_strings(values, allow_empty=False):
                errors.append(f"{component_id}: invalid {inventory}")

        if kind == "adapter" and isinstance(path_value, str):
            if path_value in adapter_paths:
                errors.append(f"duplicate adapter path: {path_value}")
            adapter_paths[path_value] = component_id

    unused_provenance = set(provenance) - provenance_users
    if unused_provenance:
        errors.append(f"unused provenance records: {sorted(unused_provenance)}")

    domains: dict[str, dict[str, object]] = {}
    memberships: dict[str, set[str]] = {
        component_id: set() for component_id in components
    }
    related_components: set[str] = set()
    related_adapters: set[str] = set()
    aliases: set[str] = set()
    for domain in catalog.get("domains", []):
        domain_id = domain.get("id")
        if not isinstance(domain_id, str) or not IDENTIFIER.fullmatch(domain_id):
            errors.append(f"invalid domain id: {domain_id!r}")
            continue
        if domain_id in domains:
            errors.append(f"duplicate domain id: {domain_id}")
            continue
        domains[domain_id] = domain
        if domain_id in aliases:
            errors.append(f"{domain_id}: domain id collides with an earlier alias")
        if domain.get("ecosystem") != f"nobro-{domain_id}":
            errors.append(f"{domain_id}: ecosystem name must be nobro-{domain_id}")
        domain_aliases = domain.get("aliases")
        if not _sorted_unique_strings(domain_aliases):
            errors.append(f"{domain_id}: invalid aliases")
            domain_aliases = []
        for alias in domain_aliases:
            if alias in aliases or alias in domains:
                errors.append(f"{domain_id}: duplicate alias {alias}")
            aliases.add(alias)
        for field, expected_kind in (("contract_ids", "contract"), ("component_ids", None)):
            identifiers = domain.get(field)
            if not _sorted_unique_strings(identifiers):
                errors.append(f"{domain_id}: invalid {field}")
                continue
            for component_id in identifiers:
                component = components.get(component_id)
                if component is None:
                    errors.append(f"{domain_id}: unknown component {component_id}")
                    continue
                if expected_kind and component.get("kind") != expected_kind:
                    errors.append(f"{domain_id}: {component_id} is not a {expected_kind}")
                related_components.add(component_id)
                memberships[component_id].add(str(domain_id))
                if component.get("kind") == "adapter":
                    path_value = str(component["path"])
                    path_domain = pathlib.PurePosixPath(path_value).parts[2]
                    if path_domain != domain_id:
                        errors.append(
                            f"{component_id}: path domain {path_domain} != relationship {domain_id}"
                        )
                    related_adapters.add(path_value)

    if "boards" in domains:
        errors.append("boards are profiles/tiers, never an ecosystem domain")
    if "motor" in domains:
        errors.append("motor has no admitted contract/member and must remain absent")
    expected_aliases = {
        "sensors": ["environment"],
        "servo": ["actuator"],
    }
    for domain_id, expected in expected_aliases.items():
        if domains.get(domain_id, {}).get("aliases") != expected:
            errors.append(f"{domain_id}: migration aliases must be {expected}")
        for alias in expected:
            if (ROOT / "core" / "adapters" / alias).exists():
                errors.append(f"{domain_id}: alias {alias} must not own source")
    if not allowed_categories.issubset(domains):
        errors.append(
            f"adapter categories without domain relationships: {sorted(allowed_categories-set(domains))}"
        )
    errors.extend(_primary_domain_errors(components, memberships, set(domains)))

    actual_adapters = {
        path.parent.relative_to(ROOT).as_posix()
        for path in (ROOT / "core" / "adapters").glob("**/Cargo.toml")
        if len(path.parent.relative_to(ROOT / "core" / "adapters").parts) in (2, 3)
    }
    for path_value in sorted(actual_adapters - related_adapters):
        errors.append(f"uncatalogued adapter: {path_value}")
    for path_value in sorted(related_adapters - actual_adapters):
        errors.append(f"catalog adapter path is not a crate: {path_value}")

    for component_id, component in components.items():
        if component.get("kind") != "contract" and component_id not in related_components:
            errors.append(f"unrelated component: {component_id}")

    candidate_ids: set[str] = set()
    for candidate in catalog.get("intake_candidates", []):
        candidate_id = candidate.get("id")
        if (
            not isinstance(candidate_id, str)
            or not IDENTIFIER.fullmatch(candidate_id)
            or candidate_id in candidate_ids
            or candidate_id in components
        ):
            errors.append(f"invalid or duplicate candidate id: {candidate_id!r}")
            continue
        candidate_ids.add(candidate_id)
        if candidate.get("status") != "blocked":
            errors.append(f"{candidate_id}: candidate must remain blocked until admitted")
        if candidate.get("desired_domain") not in domains:
            errors.append(f"{candidate_id}: desired_domain is unknown")
        if not str(candidate.get("source", "")).startswith("https://"):
            errors.append(f"{candidate_id}: candidate source must be public HTTPS")
        if not _sorted_unique_strings(candidate.get("limitations"), allow_empty=False):
            errors.append(f"{candidate_id}: blocked candidate needs sorted limitations")

    exclusion_ids: set[str] = set()
    for exclusion in catalog.get("exclusions", []):
        exclusion_id = exclusion.get("id")
        if (
            not isinstance(exclusion_id, str)
            or not IDENTIFIER.fullmatch(exclusion_id)
            or exclusion_id in exclusion_ids
        ):
            errors.append(f"invalid or duplicate exclusion id: {exclusion_id!r}")
            continue
        exclusion_ids.add(exclusion_id)
        if not isinstance(exclusion.get("reason"), str) or not exclusion["reason"]:
            errors.append(f"{exclusion_id}: exclusion reason is required")

    for duplicate in ("ecosystem", "ecosystems"):
        if (ROOT / "core" / duplicate).exists():
            errors.append(f"core/{duplicate} must not exist")
    return errors


def main() -> int:
    errors = validate()
    for error in errors:
        print(f"ADAPTER CATALOG: {error}")
    print(f"ADAPTER CATALOG: {'PASS' if not errors else 'FAIL'}")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
