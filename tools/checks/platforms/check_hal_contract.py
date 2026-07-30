#!/usr/bin/env python3
"""Validate HAL contract v2 against Rust witnesses and platform claims."""

from __future__ import annotations

import argparse
import copy
import json
import pathlib
import re
import sys


ROOT = pathlib.Path(__file__).resolve().parents[3]
CONTRACT = ROOT / "core" / "boards" / "hal_contract_v2.json"
MATRIX = ROOT / "core" / "boards" / "platform_tiers.json"
TRAITS = ROOT / "core" / "crates" / "nobro_hal" / "src" / "traits.rs"
SCHEMA = "nobro-hal-contract-v2"
STATES = {
    "required",
    "supported",
    "hardware-inapplicable",
    "unimplemented",
}
KINDS = {"deep", "constrained"}
RUST_VARIANT = {
    "timebase": "Timebase",
    "deadline": "Deadline",
    "event": "Event",
    "dma_completion": "DmaCompletion",
    "gpio": "Gpio",
    "irq": "Irq",
    "uart": "Uart",
    "byte_io": "ByteIo",
    "adc": "Adc",
    "pwm": "Pwm",
    "servo": "Servo",
    "pulse": "Pulse",
    "i2c": "I2c",
    "spi": "Spi",
    "usb": "Usb",
    "watchdog": "Watchdog",
    "rtc": "Rtc",
    "flash": "Flash",
    "reset": "Reset",
    "power": "Power",
    "cache": "Cache",
    "multicore": "Multicore",
    "lease": "Lease",
}


def _duplicates(values: list[str]) -> set[str]:
    seen: set[str] = set()
    return {value for value in values if value in seen or seen.add(value)}


def _records(records: object, key: str, label: str, errors: list[str]) -> dict:
    if not isinstance(records, list):
        errors.append(f"{label}: expected a list")
        return {}
    result = {}
    for index, record in enumerate(records):
        if not isinstance(record, dict):
            errors.append(f"{label}[{index}]: expected an object")
            continue
        identifier = record.get(key)
        if not isinstance(identifier, str) or not identifier:
            errors.append(f"{label}[{index}]: missing {key}")
            continue
        if identifier in result:
            errors.append(f"{label}: duplicate {key} {identifier!r}")
        result[identifier] = record
    return result


def _rust_capabilities(traits_text: str, errors: list[str]) -> list[str]:
    version = re.search(
        r"HARDWARE_CAPABILITY_CONTRACT_VERSION:\s*u16\s*=\s*(\d+)", traits_text
    )
    count = re.search(r"HARDWARE_CAPABILITY_COUNT:\s*usize\s*=\s*(\d+)", traits_text)
    if version is None or int(version.group(1)) != 2:
        errors.append("Rust capability contract version is not 2")
    if count is None:
        errors.append("Rust capability count is missing")
    pairs = re.findall(r'Self::([A-Za-z0-9_]+)\s*=>\s*"([a-z0-9_]+)"', traits_text)
    ids = [identifier for variant, identifier in pairs if RUST_VARIANT.get(identifier) == variant]
    if len(ids) != len(RUST_VARIANT) or set(ids) != set(RUST_VARIANT):
        errors.append("Rust HardwareCapability IDs differ from the canonical vocabulary")
    if count is not None and int(count.group(1)) != len(ids):
        errors.append("Rust capability count differs from its ID mapping")
    return ids


def validate(contract: dict, matrix: dict, *, root: pathlib.Path = ROOT) -> list[str]:
    errors: list[str] = []
    if contract.get("schema") != SCHEMA:
        errors.append(f"schema must be {SCHEMA!r}")
    if contract.get("contract_version") != 2:
        errors.append("contract_version must be 2")
    if set(contract.get("declaration_states", [])) != STATES:
        errors.append("declaration_states must contain the four canonical states")

    capabilities = contract.get("capabilities")
    if (
        not isinstance(capabilities, list)
        or not all(isinstance(item, str) and item for item in capabilities)
        or _duplicates(capabilities)
    ):
        errors.append("capabilities must be a unique non-empty string list")
        capabilities = []
    capability_set = set(capabilities)

    traits_path = root / "core" / "crates" / "nobro_hal" / "src" / "traits.rs"
    try:
        rust_ids = _rust_capabilities(traits_path.read_text(encoding="utf-8"), errors)
    except OSError as exc:
        errors.append(f"cannot read Rust capability source: {exc}")
        rust_ids = []
    if capabilities != rust_ids:
        errors.append("registry capability order differs from Rust HardwareCapability IDs")
    if capabilities != list(RUST_VARIANT):
        errors.append("registry capability order differs from checker contract")

    if matrix.get("hal_contract") != "core/boards/hal_contract_v2.json":
        errors.append("platform matrix does not reference the canonical HAL contract")
    if matrix.get("providers") != capabilities:
        errors.append("platform provider vocabulary differs from the HAL contract")

    profiles = _records(contract.get("profiles"), "id", "profiles", errors)
    for profile_id, profile in profiles.items():
        if set(profile) != {"id", "kind", "required"}:
            errors.append(f"profiles.{profile_id}: form is not exact")
        if profile.get("kind") not in KINDS:
            errors.append(f"profiles.{profile_id}: unknown kind")
        required = profile.get("required")
        if (
            not isinstance(required, list)
            or not required
            or _duplicates(required)
            or not set(required).issubset(capability_set)
        ):
            errors.append(f"profiles.{profile_id}: invalid required capabilities")

    declarations = {}
    records = contract.get("declarations")
    if not isinstance(records, list):
        errors.append("declarations: expected a list")
        records = []
    for index, declaration in enumerate(records):
        prefix = f"declarations[{index}]"
        if not isinstance(declaration, dict):
            errors.append(f"{prefix}: expected an object")
            continue
        expected_fields = {
            "platform",
            "composition",
            "profile",
            "rust_type",
            "source",
            *STATES,
            "trait_witnesses",
        }
        if set(declaration) != expected_fields:
            errors.append(f"{prefix}: declaration form is not exact")
        scope = (declaration.get("platform"), declaration.get("composition"))
        if scope in declarations:
            errors.append(f"{prefix}: duplicate platform/composition scope")
        declarations[scope] = declaration

    platforms = matrix.get("platforms", {})
    native_scopes = {
        (platform_id, composition_id)
        for platform_id, platform in platforms.items()
        if isinstance(platform, dict)
        for composition_id, composition in platform.get("compositions", {}).items()
        if isinstance(composition, dict) and composition.get("surface") == "native"
    }
    if set(declarations) != native_scopes:
        errors.append("HAL declarations do not exactly cover native platform compositions")

    for scope, declaration in declarations.items():
        platform_id, composition_id = scope
        prefix = f"declarations.{platform_id}.{composition_id}"
        profile = profiles.get(declaration.get("profile"))
        if profile is None:
            errors.append(f"{prefix}: unknown profile")
            continue
        state_sets: dict[str, set[str]] = {}
        for state in STATES:
            values = declaration.get(state)
            if (
                not isinstance(values, list)
                or _duplicates(values)
                or not set(values).issubset(capability_set)
            ):
                errors.append(f"{prefix}.{state}: invalid capability list")
                values = []
            state_sets[state] = set(values)
        classified: set[str] = set()
        for state, values in state_sets.items():
            overlap = classified & values
            if overlap:
                errors.append(f"{prefix}.{state}: overlaps {sorted(overlap)}")
            classified |= values
        if classified != capability_set:
            errors.append(f"{prefix}: four states do not classify the full vocabulary")

        supported = state_sets["supported"]
        required = state_sets["required"]
        profile_required = set(profile.get("required", []))
        if required != profile_required - supported:
            errors.append(f"{prefix}: required state differs from unsatisfied profile requirements")
        if not profile_required.issubset(supported | required):
            errors.append(f"{prefix}: selected profile is not represented by its states")
        witnesses = declaration.get("trait_witnesses")
        if not isinstance(witnesses, list) or _duplicates(witnesses):
            errors.append(f"{prefix}: invalid trait witnesses")
            witnesses = []
        if set(witnesses) != supported:
            errors.append(f"{prefix}: trait witnesses differ from supported capabilities")

        platform = platforms.get(platform_id, {})
        composition = platform.get("compositions", {}).get(composition_id, {})
        claims = composition.get("claims", {})
        if set(claims) & capability_set != supported:
            errors.append(f"{prefix}: platform claims differ from supported capabilities")
        expected_kind = "deep" if platform.get("tier") == "deep" else "constrained"
        if profile.get("kind") != expected_kind:
            errors.append(f"{prefix}: profile kind does not match the platform tier")
        if expected_kind == "deep" and required:
            errors.append(f"{prefix}: deep profile has unsatisfied requirements")

        source_text = declaration.get("source")
        rust_type = declaration.get("rust_type")
        if (
            not isinstance(source_text, str)
            or pathlib.PurePath(source_text).is_absolute()
            or ".." in pathlib.PurePath(source_text).parts
        ):
            errors.append(f"{prefix}: source must be repository-relative")
            continue
        source = (root / source_text).resolve()
        if not source.is_relative_to(root.resolve()) or not source.is_file():
            errors.append(f"{prefix}: source does not exist")
            continue
        text = source.read_text(encoding="utf-8")
        if not isinstance(rust_type, str) or f"impl HalCompatibility for {rust_type}" not in text:
            errors.append(f"{prefix}: Rust compatibility implementation is missing")
        if f'"{declaration.get("profile")}"' not in text:
            errors.append(f"{prefix}: selected profile is absent from Rust source")
        for capability in supported:
            variant = RUST_VARIANT[capability]
            marker = re.compile(
                rf"impl\s+HardwareCapabilityWitness\s*<\s*\{{\s*"
                rf"HardwareCapability::{variant}\s+as\s+u8\s*\}}\s*>\s+"
                rf"for\s+{re.escape(rust_type)}\b"
            )
            witness_bit = re.compile(
                rf"\.witnessed\s*::\s*<\s*Self\s*,\s*\{{\s*"
                rf"HardwareCapability::{variant}\s+as\s+u8\s*\}}\s*>"
            )
            if marker.search(text) is None or witness_bit.search(text) is None:
                errors.append(
                    f"{prefix}: supported {capability!r} lacks a compiled Rust witness"
                )
        if "DECLARATION.is_valid()" not in text or "DECLARATION.is_exact_profile()" not in text:
            errors.append(f"{prefix}: compile-time declaration assertions are missing")
    return errors


def selftest() -> int:
    contract = json.loads(CONTRACT.read_text(encoding="utf-8"))
    matrix = json.loads(MATRIX.read_text(encoding="utf-8"))
    errors = validate(contract, matrix)
    if errors:
        raise RuntimeError(f"real HAL contract is invalid: {errors}")

    broken = copy.deepcopy(contract)
    broken["declarations"][0]["trait_witnesses"].pop()
    if not any("trait witnesses differ" in item for item in validate(broken, matrix)):
        raise RuntimeError("unwitnessed support negative did not fail")

    broken = copy.deepcopy(contract)
    capability = broken["declarations"][0]["unimplemented"].pop()
    if not any("do not classify" in item for item in validate(broken, matrix)):
        raise RuntimeError(f"missing classification negative did not fail: {capability}")

    broken = copy.deepcopy(contract)
    selected_profile = broken["declarations"][0]["profile"]
    next(
        profile
        for profile in broken["profiles"]
        if profile["id"] == selected_profile
    )["kind"] = "constrained"
    if not any("profile kind" in item for item in validate(broken, matrix)):
        raise RuntimeError("false-deep negative did not fail")

    broken_matrix = copy.deepcopy(matrix)
    broken_matrix["platforms"]["nrf52840"]["compositions"]["native-nosd"][
        "claims"
    ].pop("timebase")
    if not any(
        "platform claims differ" in item
        for item in validate(contract, broken_matrix)
    ):
        raise RuntimeError("claim/registry mismatch negative did not fail")
    print("HAL CONTRACT SELFTEST: PASS")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    errors = validate(
        json.loads(CONTRACT.read_text(encoding="utf-8")),
        json.loads(MATRIX.read_text(encoding="utf-8")),
    )
    for error in errors:
        print(f"HAL CONTRACT: {error}")
    print("RESULT:", "FAIL" if errors else "PASS")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
