#!/usr/bin/env python3
"""Gate the scalable, non-overlapping core source layout."""

import json
import pathlib
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]
CORE = ROOT / "core"
POLICY = CORE / "layout.json"


def directories(path: pathlib.Path) -> set[str]:
    return {item.name for item in path.iterdir() if item.is_dir() and not item.name.startswith("_") and item.name != "target"}


def validate() -> list[str]:
    errors: list[str] = []
    policy = json.loads(POLICY.read_text(encoding="utf-8"))
    if policy.get("schema") != "nobro-core-layout-v1":
        errors.append("wrong layout schema")
    for collection, key in (
        ("adapters", "adapter_categories"),
        ("apps", "app_categories"),
        ("boards", "board_categories"),
    ):
        expected = set(policy[key])
        actual = directories(CORE / collection)
        if actual != expected:
            errors.append(f"{collection}: categories {sorted(actual)} != policy {sorted(expected)}")
    packages: list[pathlib.Path] = []
    for category in policy["adapter_categories"]:
        category_path = CORE / "adapters" / category
        if (category_path / "Cargo.toml").exists():
            errors.append(f"adapters/{category}: uncategorized crate at category root")
        for implementation in directories(category_path):
            implementation_path = category_path / implementation
            manifest = implementation_path / "Cargo.toml"
            if manifest.is_file():
                packages.append(manifest.parent)
                continue
            if any(
                item.is_file() and item.name != "README.md"
                for item in implementation_path.iterdir()
            ):
                errors.append(
                    f"adapters/{category}/{implementation}: family may contain only "
                    "implementation directories and an optional README.md"
                )
            nested = directories(implementation_path)
            if not nested:
                errors.append(
                    f"adapters/{category}/{implementation}: empty adapter family"
                )
            for name in nested:
                nested_manifest = implementation_path / name / "Cargo.toml"
                if not nested_manifest.is_file():
                    errors.append(
                        f"adapters/{category}/{implementation}/{name}: "
                        "missing Cargo.toml"
                    )
                else:
                    packages.append(nested_manifest.parent)
    for category in policy["app_categories"]:
        category_path = CORE / "apps" / category
        if (category_path / "Cargo.toml").exists():
            errors.append(f"apps/{category}: composition must be below its category")
        for app in directories(category_path):
            manifest = category_path / app / "Cargo.toml"
            if not manifest.is_file():
                errors.append(f"apps/{category}/{app}: missing Cargo.toml")
            else:
                packages.append(manifest.parent)
    board_ids: list[str] = []
    for category in policy["board_categories"]:
        for board in directories(CORE / "boards" / category):
            profile = CORE / "boards" / category / board / "board.json"
            if not profile.is_file():
                errors.append(f"boards/{category}/{board}: missing board.json")
                continue
            board_ids.append(json.loads(profile.read_text(encoding="utf-8")).get("board_id"))
    if len(board_ids) != len(set(board_ids)) or any(not item for item in board_ids):
        errors.append("board_id values must be non-empty and globally unique")
    for crate in directories(CORE / "crates"):
        manifest = CORE / "crates" / crate / "Cargo.toml"
        if not manifest.is_file():
            errors.append(f"crates/{crate}: flat capability package lacks Cargo.toml")
        else:
            packages.append(manifest.parent)
    workspace = tomllib.loads((CORE / "Cargo.toml").read_text(encoding="utf-8"))
    declared = set(workspace["workspace"]["members"])
    expected_members = {path.relative_to(CORE).as_posix() for path in packages}
    if declared != expected_members:
        errors.append(
            f"workspace membership drift: missing={sorted(expected_members-declared)}, extra={sorted(declared-expected_members)}"
        )
    for duplicate in ("ecosystem", "ecosystems"):
        if (CORE / duplicate).exists():
            errors.append(f"core/{duplicate} is a duplicate ownership hierarchy")
    return errors


def main() -> int:
    errors = validate()
    for error in errors:
        print(f"CORE LAYOUT: {error}")
    print(f"CORE LAYOUT: {'PASS' if not errors else 'FAIL'}")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
