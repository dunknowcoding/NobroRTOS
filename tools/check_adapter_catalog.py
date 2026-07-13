#!/usr/bin/env python3
"""Validate the public adapter tree and its concise catalog."""

import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
CATALOG = ROOT / "core" / "adapters" / "catalog.json"
LAYOUT = ROOT / "core" / "layout.json"


def validate() -> list[str]:
    errors: list[str] = []
    catalog = json.loads(CATALOG.read_text(encoding="utf-8"))
    layout = json.loads(LAYOUT.read_text(encoding="utf-8"))
    allowed = set(layout["adapter_categories"])
    seen_domains: set[str] = set()
    seen_adapters: set[str] = set()

    if catalog.get("schema") != "nobro-adapter-catalog-v1":
        errors.append("unexpected catalog schema")

    for domain in catalog.get("domains", []):
        name = domain.get("id")
        if not isinstance(name, str) or name in seen_domains:
            errors.append(f"invalid or duplicate domain: {name!r}")
            continue
        seen_domains.add(name)
        if domain.get("adapters") and name not in allowed:
            errors.append(f"uncategorized adapter domain: {name}")
        contract = domain.get("contract")
        if contract and not (ROOT / contract).is_dir():
            errors.append(f"missing contract: {contract}")
        for adapter in domain.get("adapters", []):
            if adapter in seen_adapters:
                errors.append(f"duplicate adapter: {adapter}")
            seen_adapters.add(adapter)
            path = ROOT / adapter
            if not path.is_dir() or not (path / "Cargo.toml").is_file():
                errors.append(f"missing adapter crate: {adapter}")
        for member in domain.get("library_members", []):
            facade = member.get("facade")
            if facade and not (ROOT / facade).is_file():
                errors.append(f"missing library facade: {facade}")
            for inventory in ("sensor_drivers", "board_modules"):
                values = member.get(inventory)
                if values is not None and (
                    not isinstance(values, list)
                    or not values
                    or values != sorted(set(values), key=str.casefold)
                ):
                    errors.append(f"{name}/{member.get('name')}: invalid {inventory}")

    actual = {
        path.parent.relative_to(ROOT).as_posix()
        for path in (ROOT / "core" / "adapters").glob("*/*/Cargo.toml")
    }
    for adapter in sorted(actual - seen_adapters):
        errors.append(f"uncatalogued adapter: {adapter}")
    for adapter in sorted(seen_adapters - actual):
        errors.append(f"catalog entry is not an adapter crate: {adapter}")
    return errors


def main() -> int:
    errors = validate()
    for error in errors:
        print(f"ADAPTER CATALOG: {error}")
    print(f"ADAPTER CATALOG: {'PASS' if not errors else 'FAIL'}")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
