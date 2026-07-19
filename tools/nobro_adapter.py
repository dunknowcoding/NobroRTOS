#!/usr/bin/env python3
"""Scaffold one bounded adapter and register it in catalog v2.

Usage:
    python sdk/cli/nobro.py adapter new <domain> <name>
        [--backend native|embedded-hal|c-module|arduino-shim]
"""

from __future__ import annotations

import argparse
import itertools
import json
import os
import pathlib
import re
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
BACKENDS = ("native", "embedded-hal", "c-module", "arduino-shim")
NAME = re.compile(r"^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$")


class ScaffoldError(ValueError):
    pass


def _backend_module(backend: str) -> str:
    return backend.replace("-", "_")


def _render_manifest(
    domain: str,
    name: str,
    contract: dict[str, object] | None,
    backends: tuple[str, ...],
) -> str:
    dependency = ""
    if contract is not None:
        package = str(contract["name"])
        crate_path = pathlib.PurePosixPath(str(contract["path"])).name
        dependency = f'\n[dependencies]\n{package} = {{ path = "../../../crates/{crate_path}" }}\n'
    features = "\n".join(f'backend-{backend} = []' for backend in backends)
    return (
        "[package]\n"
        f'name = "nobro-adapter-{domain}-{name}"\n'
        "version.workspace = true\n"
        "edition.workspace = true\n"
        "license.workspace = true\n"
        f"{dependency}\n"
        "[features]\n"
        "default = []\n"
        f"{features}\n"
    )


def _render_source(backends: tuple[str, ...]) -> str:
    conflicts = "\n".join(
        "#[cfg(all(feature = "
        f'"backend-{first}", feature = "backend-{second}"))]\n'
        f'compile_error!("select only one adapter backend: {first} or {second}");'
        for first, second in itertools.combinations(backends, 2)
    )
    modules = "\n\n".join(
        f'''#[cfg(feature = "backend-{backend}")]
pub mod {_backend_module(backend)} {{
    pub struct Backend;

    impl super::AdapterBackend for Backend {{
        const BACKEND_ID: &'static str = "{backend}";
    }}
}}'''
        for backend in backends
    )
    return f"""//! Generated bounded adapter scaffold.
#![no_std]

{conflicts}

pub trait AdapterBackend {{
    const BACKEND_ID: &'static str;
}}

{modules}
"""


def _render_readme(domain: str, name: str, backends: tuple[str, ...]) -> str:
    backend_list = ", ".join(f"`backend-{backend}`" for backend in backends)
    return f"""# {name}

Generated `{domain}` adapter scaffold.

- Backends: {backend_list}
- Catalog maturity: `stub`
- Evidence and supported targets: none until promoted by reproducible gates

Implement the domain contract behind one backend at a time. Keep bus, board,
credentials, and vendor runtime ownership outside the portable application API.
"""


def _insert_workspace_member(workspace: str, member: str) -> str:
    if f'"{member}"' in workspace:
        raise ScaffoldError(f"workspace member already exists: {member}")
    lines = workspace.splitlines()
    try:
        start = next(index for index, line in enumerate(lines) if line.strip() == "members = [")
        end = next(index for index in range(start + 1, len(lines)) if lines[index].strip() == "]")
    except StopIteration as error:
        raise ScaffoldError("core/Cargo.toml has no simple workspace members array") from error
    lines.insert(end, f'    "{member}",')
    return "\n".join(lines) + "\n"


def _atomic_write(path: pathlib.Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    handle, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(handle, "w", encoding="utf-8", newline="\n") as stream:
            stream.write(content)
        os.replace(temporary, path)
    except BaseException:
        pathlib.Path(temporary).unlink(missing_ok=True)
        raise


def scaffold(
    root: pathlib.Path,
    domain: str,
    name: str,
    backends: tuple[str, ...],
    capability_kind: str | None = None,
) -> pathlib.Path:
    if not NAME.fullmatch(domain) or not NAME.fullmatch(name):
        raise ScaffoldError("domain and name must be lowercase kebab-case identifiers")
    if not backends or len(set(backends)) != len(backends) or not set(backends).issubset(BACKENDS):
        raise ScaffoldError("select one or more unique supported backends")

    catalog_path = root / "core" / "adapters" / "catalog.json"
    workspace_path = root / "core" / "Cargo.toml"
    feature_path = root / "core" / "boards" / "feature_providers.json"
    catalog = json.loads(catalog_path.read_text(encoding="utf-8"))
    if catalog.get("schema") != "nobro-adapter-catalog-v2":
        raise ScaffoldError("adapter scaffolding requires catalog v2")
    domain_record = next(
        (record for record in catalog["domains"] if record["id"] == domain),
        None,
    )
    if domain_record is None:
        raise ScaffoldError(f"unknown adapter domain: {domain}")

    component_id = f"adapter-{domain}-{name}"
    if any(component["id"] == component_id for component in catalog["components"]):
        raise ScaffoldError(f"catalog component already exists: {component_id}")
    relative = pathlib.PurePosixPath("core", "adapters", domain, name)
    destination = root.joinpath(*relative.parts)
    if destination.exists():
        raise ScaffoldError(f"adapter path already exists: {relative}")

    contracts = {
        component["id"]: component
        for component in catalog["components"]
        if component["kind"] == "contract"
    }
    contract_ids = domain_record["contract_ids"]
    contract = contracts.get(contract_ids[0]) if contract_ids else None
    component = {
        "id": component_id,
        "name": name,
        "kind": "adapter",
        "path": relative.as_posix(),
        "deployment": "firmware",
        "maturity": "stub",
        "evidence": [],
        "supported_targets": [],
        "limitations": ["Generated scaffold; no target support is claimed."],
    }
    catalog["components"].append(component)
    catalog["components"].sort(key=lambda item: item["id"].casefold())
    domain_record["component_ids"].append(component_id)
    domain_record["component_ids"].sort(key=str.casefold)
    feature_registry = None
    feature_text = None
    if capability_kind is not None:
        feature_text = feature_path.read_text(encoding="utf-8")
        feature_registry = json.loads(feature_text)
        if feature_registry.get("schema") != "nobro-board-feature-registry-v2":
            raise ScaffoldError("board-feature scaffolding requires feature registry v2")
        kind = next(
            (
                record
                for record in feature_registry.get("capability_kinds", [])
                if record.get("id") == capability_kind
            ),
            None,
        )
        if kind is None:
            raise ScaffoldError(f"unknown board-feature capability kind: {capability_kind}")
        backend_id = f"backend-{domain}-{name}"
        if any(
            record.get("id") == backend_id
            or record.get("adapter_component_id") == component_id
            for record in feature_registry.get("backends", [])
        ):
            raise ScaffoldError(f"board-feature backend already exists: {backend_id}")
        feature_registry["backends"].append(
            {
                "id": backend_id,
                "capability_kind": capability_kind,
                "stack_family": kind["stack_family"],
                "adapter_component_id": component_id,
                "deployment": "firmware",
                "maturity": "stub",
                "evidence": [],
                "provenance_id": None,
                "supported_targets": [],
                "limitations": [
                    "Generated scaffold; no board binding or target support is claimed."
                ],
            }
        )
        feature_registry["backends"].sort(key=lambda item: item["id"].casefold())
    original_catalog = catalog_path.read_text(encoding="utf-8")
    original_workspace = workspace_path.read_text(encoding="utf-8")
    catalog_text = json.dumps(catalog, indent=2, ensure_ascii=False) + "\n"
    member = relative.relative_to("core").as_posix()
    workspace_text = _insert_workspace_member(
        original_workspace,
        member,
    )

    destination.mkdir(parents=True)
    try:
        _atomic_write(
            destination / "Cargo.toml",
            _render_manifest(domain, name, contract, backends),
        )
        _atomic_write(destination / "src" / "lib.rs", _render_source(backends))
        _atomic_write(destination / "README.md", _render_readme(domain, name, backends))
        _atomic_write(catalog_path, catalog_text)
        _atomic_write(workspace_path, workspace_text)
        if feature_registry is not None:
            _atomic_write(
                feature_path,
                json.dumps(feature_registry, indent=2, ensure_ascii=False) + "\n",
            )
    except BaseException:
        _atomic_write(catalog_path, original_catalog)
        _atomic_write(workspace_path, original_workspace)
        if feature_text is not None:
            _atomic_write(feature_path, feature_text)
        for file in sorted(destination.rglob("*"), reverse=True):
            if file.is_file():
                file.unlink()
            elif file.is_dir():
                file.rmdir()
        destination.rmdir()
        raise
    return destination


def selftest() -> int:
    try:
        with tempfile.TemporaryDirectory(prefix="nobro-adapter-") as temporary:
            root = pathlib.Path(temporary)
            contract = root / "core" / "crates" / "nobro_sensor"
            contract.mkdir(parents=True)
            (contract / "Cargo.toml").write_text(
                '[package]\nname="nobro-sensor"\nversion="0.0.0"\n',
                encoding="utf-8",
            )
            adapters = root / "core" / "adapters"
            adapters.mkdir(parents=True)
            catalog = {
                "schema": "nobro-adapter-catalog-v2",
                "provenance": [],
                "components": [{
                    "id": "contract-sensors",
                    "name": "nobro-sensor",
                    "kind": "contract",
                    "path": "core/crates/nobro_sensor",
                    "deployment": "firmware",
                    "maturity": "implemented",
                    "evidence": ["host-test"],
                    "supported_targets": ["portable"],
                    "limitations": [],
                }],
                "domains": [{
                    "id": "sensors",
                    "ecosystem": "nobro-sensors",
                    "aliases": ["environment"],
                    "contract_ids": ["contract-sensors"],
                    "component_ids": [],
                }],
            }
            (adapters / "catalog.json").write_text(
                json.dumps(catalog), encoding="utf-8"
            )
            workspace = root / "core" / "Cargo.toml"
            workspace.write_text(
                "[workspace]\nmembers = [\n]\n", encoding="utf-8"
            )
            boards = root / "core" / "boards"
            boards.mkdir()
            (boards / "feature_providers.json").write_text(
                json.dumps(
                    {
                        "schema": "nobro-board-feature-registry-v2",
                        "capability_kinds": [
                            {
                                "id": "audio_i2s",
                                "portable_contract_id": "contract-sensors",
                                "stack_family": "audio-i2s",
                            }
                        ],
                        "provenance": [],
                        "backends": [],
                        "bindings": [],
                    }
                ),
                encoding="utf-8",
            )
            destination = scaffold(
                root,
                "sensors",
                "demo-part",
                ("native", "embedded-hal"),
                "audio_i2s",
            )
            result = json.loads((adapters / "catalog.json").read_text(encoding="utf-8"))
            assert (destination / "src" / "lib.rs").is_file()
            manifest = (destination / "Cargo.toml").read_text(encoding="utf-8")
            assert "backend-native = []" in manifest
            assert "backend-embedded-hal = []" in manifest
            assert "backend-c-module = []" not in manifest
            assert result["domains"][0]["component_ids"] == [
                "adapter-sensors-demo-part"
            ]
            feature = json.loads(
                (boards / "feature_providers.json").read_text(encoding="utf-8")
            )
            assert feature["backends"][0]["capability_kind"] == "audio_i2s"
            assert feature["backends"][0]["adapter_component_id"] == (
                "adapter-sensors-demo-part"
            )
            assert "adapters/sensors/demo-part" in workspace.read_text(encoding="utf-8")
            try:
                scaffold(root, "sensors", "demo-part", ("native",))
            except ScaffoldError:
                pass
            else:
                raise AssertionError("duplicate scaffold did not fail closed")
    except (AssertionError, OSError, ScaffoldError, ValueError) as error:
        print(f"ADAPTER SCAFFOLD SELFTEST: FAIL ({error})")
        return 1
    print("ADAPTER SCAFFOLD SELFTEST: PASS")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    subparsers = parser.add_subparsers(dest="command")
    new = subparsers.add_parser("new", help="create and register one adapter")
    new.add_argument("domain")
    new.add_argument("name")
    new.add_argument(
        "--backend",
        action="append",
        choices=BACKENDS,
        help="backend module to include; repeat as needed (default: all)",
    )
    new.add_argument(
        "--capability-kind",
        help="also register this adapter as a stub backend for a board-feature kind",
    )
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if args.command != "new":
        parser.print_help()
        return 2
    try:
        destination = scaffold(
            ROOT,
            args.domain,
            args.name,
            tuple(args.backend or BACKENDS),
            args.capability_kind,
        )
    except (OSError, ScaffoldError, ValueError) as error:
        print(f"nobro adapter: {error}", file=sys.stderr)
        return 2
    print(destination.relative_to(ROOT).as_posix())
    return 0


if __name__ == "__main__":
    sys.exit(main())
