#!/usr/bin/env python3
"""Validate the extensible public platform capability/evidence matrix.

The v2 matrix models independent compositions (for example native Rust and an
Arduino facade) and binds every claim to named executable evidence gates.  This
validator deliberately does not search implementation source for magic tokens:
an implementation path must exist, evidence must be scoped to the platform and
composition, and CI runners must submit all of their required gate receipts.

    python tools/check_platform_tiers.py
    python tools/check_platform_tiers.py --selftest
    python tools/check_platform_tiers.py --begin-receipts RUNNER
    python tools/check_platform_tiers.py --run-gate GATE
    python tools/check_platform_tiers.py --assert-receipts RUNNER

A target build proves compilation only.  The public matrix does not accept a
"physical" evidence kind, so source text or a successful build cannot be
mistaken for HIL evidence.
"""

from __future__ import annotations

import argparse
import contextlib
import copy
import hashlib
import io
import json
import os
import pathlib
import re
import secrets
import shutil
import stat
import subprocess
import sys
import tempfile
from unittest import mock

import check_board_features


ROOT = pathlib.Path(__file__).resolve().parents[1]
MATRIX = ROOT / "core" / "boards" / "platform_tiers.json"
FEATURE_REGISTRY = ROOT / "core" / "boards" / "feature_providers.json"
ADAPTER_CATALOG = ROOT / "core" / "adapters" / "catalog.json"
SCHEMA = "nobro-platform-support-v2"
SURFACE_VOCABULARY = {"native": "providers", "arduino": "facade_offers"}
HOST_EVIDENCE = "host-test"
TARGET_EVIDENCE = "target-build"
SAFE_PLACEHOLDERS = {"host_target"}
PLACEHOLDER = re.compile(r"\{([^{}]+)\}")
SAFE_COMPONENT = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]*$")
SAFE_ENVIRONMENT_NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
WORKFLOW = ROOT / ".github" / "workflows" / "gates.yml"


def _duplicates(values: list[str]) -> set[str]:
    seen: set[str] = set()
    return {value for value in values if value in seen or seen.add(value)}


def _resolved_repository_path(path_text: object) -> pathlib.Path | None:
    if not isinstance(path_text, str) or not path_text:
        return None
    path = pathlib.PurePath(path_text)
    if path.is_absolute() or ".." in path.parts:
        return None
    root = ROOT.resolve()
    candidate = (root / path_text).resolve()
    if not candidate.is_relative_to(root):
        return None
    return candidate


def _relative_existing_directory(path_text: object) -> bool:
    path = _resolved_repository_path(path_text)
    return path is not None and path.is_dir()


def _relative_existing_file(path_text: object) -> bool:
    path = _resolved_repository_path(path_text)
    return path is not None and path.is_file()


def _workflow_job_block(workflow_text: str, job: str) -> str | None:
    match = re.search(
        rf"(?ms)^  {re.escape(job)}:\s*\n(?P<body>.*?)(?=^  [A-Za-z0-9_-]+:\s*$|\Z)",
        workflow_text,
    )
    return match.group(0) if match else None


def validate(
    matrix: dict,
    *,
    check_runner_bindings: bool = True,
    feature_registry: dict | None = None,
) -> list[str]:
    """Return every semantic error; do not depend on a fixed board or claim set."""
    errors: list[str] = []
    if feature_registry is None:
        feature_registry = json.loads(FEATURE_REGISTRY.read_text(encoding="utf-8"))
    adapter_catalog = json.loads(ADAPTER_CATALOG.read_text(encoding="utf-8"))
    errors.extend(check_board_features.validate(feature_registry, adapter_catalog))
    if matrix.get("feature_registry") != "core/boards/feature_providers.json":
        errors.append("feature_registry must name the public board-feature registry")
    feature_capabilities = check_board_features.capability_ids(feature_registry)
    feature_bindings = {
        (
            binding.get("platform"),
            binding.get("composition"),
            binding.get("capability_kind"),
        )
        for binding in feature_registry.get("bindings", [])
        if isinstance(binding, dict)
    }
    if matrix.get("schema") != SCHEMA:
        errors.append(f"schema must be {SCHEMA!r}")

    tiers = matrix.get("tiers", {})
    providers_list = matrix.get("providers", [])
    facade_list = matrix.get("facade_offers", [])
    evidence_kinds = matrix.get("evidence_kinds", {})
    maturities = matrix.get("maturities", {})
    runners = matrix.get("runners", {})
    gates = matrix.get("evidence_gates", {})
    platforms = matrix.get("platforms", {})
    providers = set(providers_list)
    native_vocabulary = providers | feature_capabilities
    facade_offers = set(facade_list)

    unsupported_evidence = set(evidence_kinds) - {HOST_EVIDENCE, TARGET_EVIDENCE}
    if unsupported_evidence:
        errors.append(
            "evidence_kinds: public matrix accepts only host-test and target-build; "
            f"unsupported {sorted(unsupported_evidence)}"
        )

    for label, values in (("providers", providers_list), ("facade_offers", facade_list)):
        if not isinstance(values, list) or not all(isinstance(value, str) and value for value in values):
            errors.append(f"{label}: expected a list of non-empty names")
        elif _duplicates(values):
            errors.append(f"{label}: duplicate names {sorted(_duplicates(values))}")

    for label, mapping in (
        ("tiers", tiers),
        ("evidence_kinds", evidence_kinds),
        ("maturities", maturities),
        ("runners", runners),
        ("evidence_gates", gates),
        ("platforms", platforms),
    ):
        if not isinstance(mapping, dict) or not mapping:
            errors.append(f"{label}: expected a non-empty object")

    reference = matrix.get("reference_platform")
    if reference not in platforms:
        errors.append(f"reference_platform {reference!r} is not declared")
    elif platforms[reference].get("tier") != "deep":
        errors.append("reference_platform must currently have the deep tier")

    for runner_id, runner in runners.items():
        prefix = f"runners.{runner_id}"
        if not isinstance(runner_id, str) or not SAFE_COMPONENT.fullmatch(runner_id):
            errors.append("runners: runner IDs must be safe path components")
            continue
        if not isinstance(runner, dict):
            errors.append(f"{prefix}: expected an object")
            continue
        workflow_job = runner.get("workflow_job")
        if not isinstance(workflow_job, str) or not SAFE_COMPONENT.fullmatch(workflow_job):
            errors.append(f"{prefix}: workflow_job must be a safe job ID")
        if not _relative_existing_file(runner.get("receipt_driver")):
            errors.append(f"{prefix}: receipt_driver must be a repository-contained file")

    gate_references: set[str] = set()
    gate_claim_scopes: dict[str, set[tuple[str, str, str]]] = {}
    for gate_id, gate in gates.items():
        prefix = f"evidence_gates.{gate_id}"
        if not isinstance(gate_id, str) or not SAFE_COMPONENT.fullmatch(gate_id):
            errors.append("evidence_gates: gate IDs must be safe path components")
            continue
        if not isinstance(gate, dict):
            errors.append(f"{prefix}: expected an object")
            continue
        if gate.get("kind") not in evidence_kinds:
            errors.append(f"{prefix}: unknown evidence kind {gate.get('kind')!r}")
        runner = gate.get("runner")
        if not isinstance(runner, str) or not SAFE_COMPONENT.fullmatch(runner):
            errors.append(f"{prefix}: runner must be a safe path component")
        elif runner not in runners:
            errors.append(f"{prefix}: runner {runner!r} has no hosted binding")
        if not isinstance(gate.get("required"), bool):
            errors.append(f"{prefix}: required must be boolean")
        if not _relative_existing_directory(gate.get("cwd")):
            errors.append(f"{prefix}: cwd must be a repository-contained directory")
        command = gate.get("command")
        if not isinstance(command, list) or not command or not all(
            isinstance(token, str) and token for token in command
        ):
            errors.append(f"{prefix}: command must be a non-empty argument array")
        else:
            placeholders = {
                match.group(1) for token in command for match in PLACEHOLDER.finditer(token)
            }
            unknown = placeholders - SAFE_PLACEHOLDERS
            if unknown:
                errors.append(f"{prefix}: unsupported command placeholders {sorted(unknown)}")
            if command[0] == "cargo" and "--locked" not in command:
                errors.append(f"{prefix}: Cargo evidence commands must use --locked")
        scopes = gate.get("claim_scopes")
        expanded_scopes: set[tuple[str, str, str]] = set()
        if not isinstance(scopes, list) or not scopes:
            errors.append(f"{prefix}: claim_scopes must be a non-empty list")
        else:
            for index, scope in enumerate(scopes):
                scope_prefix = f"{prefix}.claim_scopes[{index}]"
                if not isinstance(scope, dict):
                    errors.append(f"{scope_prefix}: expected an object")
                    continue
                platform_id = scope.get("platform")
                composition_id = scope.get("composition")
                capabilities = scope.get("capabilities")
                if not isinstance(platform_id, str) or platform_id not in platforms:
                    errors.append(f"{scope_prefix}: unknown platform {platform_id!r}")
                    continue
                platform = platforms[platform_id]
                compositions = platform.get("compositions", {}) if isinstance(platform, dict) else {}
                if not isinstance(composition_id, str) or composition_id not in compositions:
                    errors.append(f"{scope_prefix}: unknown composition {composition_id!r}")
                    continue
                composition = compositions[composition_id]
                surface = composition.get("surface") if isinstance(composition, dict) else None
                vocabulary_name = SURFACE_VOCABULARY.get(surface)
                vocabulary = (
                    native_vocabulary
                    if vocabulary_name == "providers"
                    else set(matrix.get(vocabulary_name, []))
                    if vocabulary_name
                    else set()
                )
                if not isinstance(capabilities, list) or not capabilities or not all(
                    isinstance(capability, str) and capability for capability in capabilities
                ):
                    errors.append(f"{scope_prefix}: capabilities must be a non-empty name list")
                    continue
                if _duplicates(capabilities):
                    errors.append(f"{scope_prefix}: duplicate capabilities")
                for capability in capabilities:
                    if capability not in vocabulary:
                        errors.append(
                            f"{scope_prefix}: capability {capability!r} is not valid for {surface!r}"
                        )
                        continue
                    expanded = (platform_id, composition_id, capability)
                    if expanded in expanded_scopes:
                        errors.append(f"{scope_prefix}: duplicate expanded claim scope {expanded!r}")
                    expanded_scopes.add(expanded)
        gate_claim_scopes[gate_id] = expanded_scopes
        environment = gate.get("environment", {})
        if not isinstance(environment, dict) or not all(
            isinstance(key, str)
            and SAFE_ENVIRONMENT_NAME.fullmatch(key)
            and isinstance(value, str)
            for key, value in environment.items()
        ):
            errors.append(f"{prefix}: environment must map safe variable names to strings")
        if gate.get("required") is False and not gate.get("condition"):
            errors.append(f"{prefix}: conditional gate must explain its condition")

    claim_scope_uses: set[tuple[str, str, str, str]] = set()
    for platform_id, platform in platforms.items():
        prefix = f"platforms.{platform_id}"
        if not isinstance(platform_id, str) or not SAFE_COMPONENT.fullmatch(platform_id):
            errors.append("platforms: platform IDs must be safe components")
            continue
        if not isinstance(platform, dict):
            errors.append(f"{prefix}: expected an object")
            continue
        tier = platform.get("tier")
        if tier not in tiers:
            errors.append(f"{prefix}: unknown tier {tier!r}")
        if not isinstance(platform.get("arch"), str) or not platform.get("arch"):
            errors.append(f"{prefix}: arch must be a non-empty string")
        if not _relative_existing_directory(platform.get("implementation_root")):
            errors.append(f"{prefix}: implementation_root must be a repository-contained directory")

        compositions = platform.get("compositions")
        if not isinstance(compositions, dict):
            errors.append(f"{prefix}: compositions must be an object")
            continue
        native_claims = 0
        complete_native_compositions: list[set[str]] = []
        for composition_id, composition in compositions.items():
            comp_prefix = f"{prefix}.compositions.{composition_id}"
            if not isinstance(composition_id, str) or not SAFE_COMPONENT.fullmatch(composition_id):
                errors.append(f"{prefix}: composition IDs must be safe components")
                continue
            if not isinstance(composition, dict):
                errors.append(f"{comp_prefix}: expected an object")
                continue
            surface = composition.get("surface")
            if surface not in SURFACE_VOCABULARY:
                errors.append(f"{comp_prefix}: unknown surface {surface!r}")
                continue
            vocabulary = native_vocabulary if surface == "native" else facade_offers
            claims = composition.get("claims")
            if not isinstance(claims, dict) or not claims:
                errors.append(f"{comp_prefix}: claims must be a non-empty object")
                continue
            if surface == "native":
                native_claims += len(claims)
            implemented_capabilities: set[str] = set()
            for capability, claim in claims.items():
                claim_prefix = f"{comp_prefix}.claims.{capability}"
                if capability not in vocabulary:
                    errors.append(f"{claim_prefix}: capability is not in the {surface} vocabulary")
                if capability in feature_capabilities and (
                    platform_id,
                    composition_id,
                    capability,
                ) not in feature_bindings:
                    errors.append(
                        f"{claim_prefix}: board-feature claim has no exact registry binding"
                    )
                if not isinstance(claim, dict):
                    errors.append(f"{claim_prefix}: expected an object")
                    continue
                if claim.get("maturity") not in maturities:
                    errors.append(f"{claim_prefix}: unknown maturity {claim.get('maturity')!r}")
                elif claim.get("maturity") == "implemented":
                    implemented_capabilities.add(capability)
                evidence = claim.get("evidence")
                if not isinstance(evidence, list) or not evidence:
                    errors.append(f"{claim_prefix}: evidence must be a non-empty list")
                    continue
                if _duplicates(evidence):
                    errors.append(f"{claim_prefix}: duplicate evidence gate")
                claim_kinds: set[str] = set()
                for gate_id in evidence:
                    gate_references.add(gate_id)
                    gate = gates.get(gate_id)
                    if not isinstance(gate, dict):
                        errors.append(f"{claim_prefix}: unknown evidence gate {gate_id!r}")
                        continue
                    claim_kinds.add(gate.get("kind"))
                    scope = (platform_id, composition_id, capability)
                    if scope not in gate_claim_scopes.get(gate_id, set()):
                        errors.append(
                            f"{claim_prefix}: gate {gate_id!r} is not scoped to this exact claim"
                        )
                    else:
                        claim_scope_uses.add((gate_id, *scope))
                if not claim_kinds & {HOST_EVIDENCE, TARGET_EVIDENCE}:
                    errors.append(f"{claim_prefix}: lacks executable host-test or target-build evidence")
                if claim.get("maturity") == "implemented" and not any(
                    isinstance(gates.get(gate_id), dict)
                    and gates[gate_id].get("required") is True
                    for gate_id in evidence
                ):
                    errors.append(
                        f"{claim_prefix}: implemented maturity needs a required evidence gate"
                    )
                if claim_kinds == {TARGET_EVIDENCE} and not (
                    claim.get("limitations") or platform.get("limitations")
                ):
                    errors.append(f"{claim_prefix}: target-build-only claim must state a limitation")

            if surface == "native":
                complete_native_compositions.append(implemented_capabilities)

        if tier == "provider" and native_claims == 0:
            errors.append(f"{prefix}: provider tier requires at least one native claim")
        if tier == "deep" and not any(
            providers.issubset(capabilities) for capabilities in complete_native_compositions
        ):
            errors.append(
                f"{prefix}: deep tier requires one native composition with every implemented "
                "provider capability"
            )
        if tier == "absent" and compositions:
            errors.append(f"{prefix}: absent tier cannot publish compositions")

    unused = set(gates) - gate_references
    if unused:
        errors.append(f"evidence_gates: unreferenced gates {sorted(unused)}")
    for gate_id, scopes in gate_claim_scopes.items():
        for platform_id, composition_id, capability in scopes:
            if (gate_id, platform_id, composition_id, capability) not in claim_scope_uses:
                errors.append(
                    f"evidence_gates.{gate_id}: unused exact claim scope "
                    f"{platform_id}.{composition_id}.{capability}"
                )
    if check_runner_bindings:
        errors.extend(validate_runner_bindings(matrix))
    return errors


def validate_runner_bindings(matrix: dict) -> list[str]:
    """Bind every declared receipt runner to one hosted job and concrete driver."""
    errors: list[str] = []
    runners = matrix.get("runners", {})
    gates = matrix.get("evidence_gates", {})
    if not isinstance(runners, dict) or not isinstance(gates, dict):
        return errors
    try:
        workflow_text = WORKFLOW.read_text(encoding="utf-8")
    except OSError as exc:
        return [f"runners: cannot read hosted workflow: {exc}"]

    for hosted_job in re.findall(
        r"(?ms)^  [A-Za-z0-9_-]+:\s*\n.*?(?=^  [A-Za-z0-9_-]+:\s*$|\Z)",
        workflow_text,
    ):
        job_header = hosted_job.splitlines()[0].strip().removesuffix(":")
        if re.search(r"(?m)^\s+(?:python|python3)\s", hosted_job) and "actions/setup-python@" not in hosted_job:
            errors.append(f"hosted workflow job {job_header!r} invokes Python without setup-python")

    for runner_id, runner in runners.items():
        if not isinstance(runner_id, str) or not isinstance(runner, dict):
            continue
        prefix = f"runners.{runner_id}"
        job_id = runner.get("workflow_job")
        if not isinstance(job_id, str):
            continue
        job_block = _workflow_job_block(workflow_text, job_id)
        if job_block is None:
            errors.append(f"{prefix}: hosted workflow job {job_id!r} is missing")
            continue
        if "actions/setup-python@" not in job_block:
            errors.append(f"{prefix}: hosted workflow job must install Python explicitly")
        driver_text = job_block
        driver_path = runner.get("receipt_driver")
        resolved_driver = _resolved_repository_path(driver_path)
        if resolved_driver is None or not resolved_driver.is_file():
            continue
        if resolved_driver != WORKFLOW.resolve():
            if str(driver_path).replace("\\", "/") not in job_block:
                errors.append(f"{prefix}: hosted job does not invoke receipt_driver")
            driver_text = resolved_driver.read_text(encoding="utf-8")

        for option in ("--begin-receipts", "--assert-receipts"):
            if not re.search(
                rf"{re.escape(option)}\s+{re.escape(runner_id)}(?:\s|$)", driver_text
            ):
                errors.append(f"{prefix}: receipt_driver omits {option} {runner_id}")

        required_gates = sorted(
            gate_id
            for gate_id, gate in gates.items()
            if isinstance(gate, dict)
            and gate.get("runner") == runner_id
            and gate.get("required") is True
        )
        for gate_id in required_gates:
            if not re.search(
                rf"--run-gate\s+{re.escape(gate_id)}(?:\s|$)", driver_text
            ):
                errors.append(f"{prefix}: receipt_driver omits required gate {gate_id!r}")
    return errors


def validate_receipts(matrix: dict, runner: str, receipts: list[str]) -> list[str]:
    """Verify a CI runner reported every required gate assigned to it."""
    errors = validate(matrix)
    gates = matrix.get("evidence_gates", {})
    if _duplicates(receipts):
        errors.append(f"receipts.{runner}: duplicate gate IDs {sorted(_duplicates(receipts))}")
    expected = {
        gate_id for gate_id, gate in gates.items()
        if gate.get("runner") == runner and gate.get("required") is True
    }
    optional = {
        gate_id for gate_id, gate in gates.items()
        if gate.get("runner") == runner and gate.get("required") is False
    }
    if not expected and not optional:
        errors.append(f"receipts: unknown runner {runner!r}")
        return errors
    supplied = set(receipts)
    unknown = supplied - expected - optional
    missing = expected - supplied
    if unknown:
        errors.append(f"receipts.{runner}: unknown or wrong-runner gates {sorted(unknown)}")
    if missing:
        errors.append(f"receipts.{runner}: missing required gates {sorted(missing)}")
    return errors


def _receipt_directory(
    runner: str, receipt_root: pathlib.Path | None = None
) -> pathlib.Path:
    if not SAFE_COMPONENT.fullmatch(runner):
        raise ValueError("runner must be a safe path component")
    if receipt_root is None:
        work = (ROOT / "_work").resolve()
        base = (work / "platform-evidence").resolve()
        if not base.is_relative_to(work):
            raise RuntimeError("evidence receipt root escaped _work")
    else:
        base = receipt_root.resolve()
    unresolved = base / runner
    if unresolved.is_symlink():
        raise RuntimeError("evidence runner directory cannot be a symlink")
    directory = unresolved.resolve()
    if directory.parent != base or not directory.is_relative_to(base):
        raise RuntimeError("evidence runner directory escaped its receipt root")
    return directory


def _gate_digest(gate_id: str, gate: dict) -> str:
    payload = json.dumps(
        {"gate_id": gate_id, "gate": gate}, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def _matrix_digest(matrix: dict) -> str:
    payload = json.dumps(matrix, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def _digest_chunk(hasher, label: bytes, payload: bytes) -> None:
    """Add one unambiguous, length-delimited field to a worktree digest."""
    hasher.update(len(label).to_bytes(4, "big"))
    hasher.update(label)
    hasher.update(len(payload).to_bytes(8, "big"))
    hasher.update(payload)


def _git_output(source_root: pathlib.Path, *arguments: str) -> bytes:
    try:
        process = subprocess.Popen(
            ["git", "-C", os.fspath(source_root), *arguments],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        stdout, stderr = process.communicate()
    except OSError as exc:
        raise RuntimeError(f"git {' '.join(arguments)} could not start: {exc}") from exc
    if process.returncode:
        detail = stderr.decode("utf-8", errors="replace").strip()
        suffix = f": {detail}" if detail else ""
        raise RuntimeError(f"git {' '.join(arguments)} failed{suffix}")
    return stdout


def _public_worktree_digest(source_root: pathlib.Path | None = None) -> str:
    """Fingerprint HEAD, tracked edits, and every nonignored untracked file.

    Git's exclude rules intentionally keep ignored build and receipt state (including
    `_work`) outside this public-source freshness boundary.
    """
    root = (source_root or ROOT).resolve()
    top_level_raw = _git_output(root, "rev-parse", "--show-toplevel").strip()
    top_level = pathlib.Path(os.fsdecode(top_level_raw)).resolve()
    if top_level != root:
        raise RuntimeError(f"source root {root} is not the Git worktree root {top_level}")

    head = _git_output(root, "rev-parse", "--verify", "HEAD").strip()
    tracked_diff = _git_output(
        root,
        "diff",
        "--binary",
        "--full-index",
        "--no-ext-diff",
        "--no-textconv",
        "HEAD",
        "--",
        ".",
    )
    untracked_output = _git_output(
        root, "ls-files", "--others", "--exclude-standard", "-z", "--", "."
    )
    untracked_names = sorted(name for name in untracked_output.split(b"\0") if name)

    hasher = hashlib.sha256()
    _digest_chunk(hasher, b"format", b"nobro-public-worktree-v1")
    _digest_chunk(hasher, b"head", head)
    _digest_chunk(hasher, b"tracked-diff", tracked_diff)
    for raw_name in untracked_names:
        decoded_name = os.fsdecode(raw_name)
        relative = pathlib.PurePath(decoded_name)
        if relative.is_absolute() or ".." in relative.parts:
            raise RuntimeError(f"unsafe untracked path reported by Git: {decoded_name!r}")
        path = root / decoded_name
        try:
            mode = path.lstat().st_mode
            if stat.S_ISLNK(mode):
                kind = b"symlink"
                content = os.fsencode(os.readlink(path))
            elif stat.S_ISREG(mode):
                kind = b"file"
                content = path.read_bytes()
            else:
                raise RuntimeError(
                    f"unsupported non-file public worktree entry: {decoded_name!r}"
                )
        except OSError as exc:
            raise RuntimeError(
                f"cannot fingerprint untracked path {decoded_name!r}: {exc}"
            ) from exc
        _digest_chunk(hasher, b"untracked-path", raw_name)
        _digest_chunk(hasher, b"untracked-kind", kind)
        _digest_chunk(hasher, b"untracked-mode", f"{stat.S_IMODE(mode):04o}".encode("ascii"))
        _digest_chunk(hasher, b"untracked-content", content)
    return hasher.hexdigest()


def _host_target() -> str:
    rustc_version = subprocess.check_output(
        ["rustc", "-vV"], text=True, encoding="utf-8", errors="replace"
    )
    return next(
        line.split(":", 1)[1].strip()
        for line in rustc_version.splitlines()
        if line.startswith("host:")
    )


def _active_session(
    matrix: dict,
    runner: str,
    receipt_root: pathlib.Path | None = None,
    source_root: pathlib.Path | None = None,
) -> tuple[dict | None, list[str]]:
    directory = _receipt_directory(runner, receipt_root)
    marker = directory / ".active"
    if marker.is_symlink():
        return None, [f"receipts.{runner}: active session cannot be a symlink"]
    try:
        active = json.loads(marker.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None, [f"receipts.{runner}: no active clean receipt session"]
    except (OSError, json.JSONDecodeError) as exc:
        return None, [f"receipts.{runner}: invalid active session: {exc}"]
    try:
        worktree_digest = _public_worktree_digest(source_root)
    except RuntimeError as exc:
        return None, [f"receipts.{runner}: cannot fingerprint public worktree: {exc}"]
    expected = {
        "schema": SCHEMA,
        "runner": runner,
        "matrix_digest": _matrix_digest(matrix),
        "worktree_digest": worktree_digest,
    }
    if not isinstance(active, dict) or any(active.get(key) != value for key, value in expected.items()):
        return None, [f"receipts.{runner}: stale or altered active session"]
    session_id = active.get("session_id")
    if not isinstance(session_id, str) or not re.fullmatch(r"[0-9a-f]{32}", session_id):
        return None, [f"receipts.{runner}: invalid active session ID"]
    return active, []


def begin_receipts(
    matrix: dict,
    runner: str,
    receipt_root: pathlib.Path | None = None,
    source_root: pathlib.Path | None = None,
) -> int:
    """Start a clean receipt session for one declared runner under ignored `_work`."""
    errors = validate(matrix)
    if errors:
        for error in errors:
            print(f"PLATFORM TIERS: {error}")
        print("RESULT: FAIL")
        return 1
    runners = matrix.get("runners", {})
    if runner not in runners:
        print(f"PLATFORM TIERS: receipts: unknown runner {runner!r}")
        print("RESULT: FAIL")
        return 1
    directory = _receipt_directory(runner, receipt_root)
    if directory.exists():
        if not directory.is_dir():
            print(f"PLATFORM TIERS: receipts.{runner}: receipt path is not a directory")
            print("RESULT: FAIL")
            return 1
        shutil.rmtree(directory)
    directory.mkdir(parents=True)
    try:
        worktree_digest = _public_worktree_digest(source_root)
    except RuntimeError as exc:
        print(f"PLATFORM TIERS: receipts.{runner}: cannot fingerprint public worktree: {exc}")
        print("RESULT: FAIL")
        return 1
    active = {
        "schema": SCHEMA,
        "runner": runner,
        "matrix_digest": _matrix_digest(matrix),
        "worktree_digest": worktree_digest,
        "session_id": secrets.token_hex(16),
    }
    (directory / ".active").write_text(
        json.dumps(active, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"EVIDENCE RECEIPTS {runner}: READY")
    return 0


def _load_receipts(
    matrix: dict,
    runner: str,
    receipt_root: pathlib.Path | None = None,
    source_root: pathlib.Path | None = None,
) -> tuple[list[str], list[str]]:
    directory = _receipt_directory(runner, receipt_root)
    active, errors = _active_session(matrix, runner, receipt_root, source_root)
    if active is None:
        return [], errors
    receipts: list[str] = []
    for path in sorted(directory.glob("*.json")):
        if path.is_symlink():
            errors.append(f"receipts.{runner}: receipt files cannot be symlinks")
            continue
        try:
            record = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            errors.append(f"receipts.{runner}: unreadable receipt {path.name}: {exc}")
            continue
        gate_id = record.get("gate_id")
        gate = matrix.get("evidence_gates", {}).get(gate_id)
        if not isinstance(gate, dict) or gate.get("runner") != runner:
            errors.append(f"receipts.{runner}: invalid gate in {path.name}")
            continue
        if path.name != f"{gate_id}.json":
            errors.append(f"receipts.{runner}: receipt filename does not match its gate")
            continue
        expected_record = {
            "schema": SCHEMA,
            "runner": runner,
            "matrix_digest": active["matrix_digest"],
            "worktree_digest": active["worktree_digest"],
            "session_id": active["session_id"],
        }
        if any(record.get(key) != value for key, value in expected_record.items()):
            errors.append(f"receipts.{runner}: stale or altered receipt {path.name}")
            continue
        if record.get("gate_digest") != _gate_digest(gate_id, gate):
            errors.append(f"receipts.{runner}: stale or altered receipt {path.name}")
            continue
        receipts.append(gate_id)
    return receipts, errors


def assert_runner_receipts(
    matrix: dict,
    runner: str,
    receipt_root: pathlib.Path | None = None,
    source_root: pathlib.Path | None = None,
) -> list[str]:
    receipts, load_errors = _load_receipts(matrix, runner, receipt_root, source_root)
    return validate_receipts(matrix, runner, receipts) + load_errors


def execute_gate(
    matrix: dict,
    gate_id: str,
    receipt_root: pathlib.Path | None = None,
    source_root: pathlib.Path | None = None,
) -> int:
    """Execute one gate from the matrix so its command cannot drift from its receipt."""
    errors = validate(matrix)
    gate = matrix.get("evidence_gates", {}).get(gate_id)
    if not isinstance(gate, dict):
        errors.append(f"run-gate: unknown evidence gate {gate_id!r}")
    active = None
    if isinstance(gate, dict):
        active, session_errors = _active_session(
            matrix, gate["runner"], receipt_root, source_root
        )
        errors.extend(session_errors)
    if errors:
        for error in errors:
            print(f"PLATFORM TIERS: {error}")
        print("RESULT: FAIL")
        return 1

    values: dict[str, str] = {}
    if any("{host_target}" in token for token in gate["command"]):
        values["host_target"] = _host_target()
    command = [token.format_map(values) for token in gate["command"]]
    if command[0] == "python":
        command[0] = sys.executable
    environment = dict(os.environ)
    environment.update(gate.get("environment", {}))
    receipt = _receipt_directory(gate["runner"], receipt_root) / f"{gate_id}.json"
    receipt.parent.mkdir(parents=True, exist_ok=True)
    receipt.unlink(missing_ok=True)
    result = subprocess.run(command, cwd=ROOT / gate["cwd"], env=environment, check=False)
    if result.returncode == 0:
        refreshed_active, source_errors = _active_session(
            matrix, gate["runner"], receipt_root, source_root
        )
        if refreshed_active != active:
            for error in source_errors or [
                f"receipts.{gate['runner']}: active session changed during gate execution"
            ]:
                print(f"PLATFORM TIERS: {error}")
            print(f"EVIDENCE GATE {gate_id}: FAIL")
            return 1
        record = {
            "schema": SCHEMA,
            "runner": gate["runner"],
            "gate_id": gate_id,
            "gate_digest": _gate_digest(gate_id, gate),
            "matrix_digest": active["matrix_digest"],
            "worktree_digest": active["worktree_digest"],
            "session_id": active["session_id"],
        }
        temporary = receipt.with_suffix(".tmp")
        temporary.unlink(missing_ok=True)
        temporary.write_text(json.dumps(record, sort_keys=True) + "\n", encoding="utf-8")
        temporary.replace(receipt)
    print(f"EVIDENCE GATE {gate_id}: {'PASS' if result.returncode == 0 else 'FAIL'}")
    return result.returncode


def _expect(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def _expect_error(errors: list[str], fragment: str) -> None:
    _expect(
        any(fragment in error for error in errors),
        f"expected error containing {fragment!r}, got {errors}",
    )


def _quiet_call(function, *args, **kwargs):
    with contextlib.redirect_stdout(io.StringIO()):
        return function(*args, **kwargs)


def selftest() -> int:
    good = json.loads(MATRIX.read_text(encoding="utf-8"))
    feature_registry = json.loads(FEATURE_REGISTRY.read_text(encoding="utf-8"))
    _expect(validate(good) == [], f"real matrix should be clean: {validate(good)}")

    bad_reference = copy.deepcopy(good)
    bad_reference["reference_platform"] = "rp2350"
    _expect_error(validate(bad_reference), "reference_platform must")

    false_deep = copy.deepcopy(good)
    false_deep["platforms"]["rp2350"]["tier"] = "deep"
    _expect_error(validate(false_deep), "deep tier requires one native composition")

    unknown_claim = copy.deepcopy(good)
    unknown_claim["platforms"]["rp2350"]["compositions"]["native"]["claims"]["magic"] = {
        "maturity": "implemented",
        "evidence": ["rp2350-target-build"],
    }
    _expect_error(validate(unknown_claim), "capability is not")

    future_feature_registry = copy.deepcopy(feature_registry)
    future_feature_registry["backends"].append(
        {
            "id": "test-audio-backend",
            "capability_kind": "audio_i2s",
            "stack_family": "audio-i2s",
            "adapter_component_id": "adapter-servo-roboservo",
            "deployment": "firmware",
            "maturity": "compile-only",
            "evidence": ["target-build"],
            "provenance_id": None,
            "supported_targets": ["esp32s3"],
            "limitations": ["Selftest fixture only."],
        }
    )
    future_feature_registry["bindings"].append(
        {
            "id": "test-audio-binding",
            "backend_id": "test-audio-backend",
            "capability_kind": "audio_i2s",
            "platform": "esp32s3",
            "composition": "native",
            "instance": "audio0",
            "maturity": "compile-only",
            "evidence_gates": ["esp32s3-target-build"],
            "measured_price": {field: 0 for field in check_board_features.PRICE_FIELDS},
            "price_provenance": {
                field: "declared-zero" for field in check_board_features.PRICE_FIELDS
            },
            "coexistence": {
                field: [] for field in check_board_features.COEXISTENCE_FIELDS
            },
            "disabled_symbol_gate": {
                "baseline": "same-board-no-audio",
                "feature": "audio_i2s",
                "forbidden_symbols": ["test_audio_backend"],
                "max_flash_delta_bytes": 0,
                "max_ram_delta_bytes": 0,
            },
            "report_wiring": {
                "provider_id": "audio_i2s",
                "status_field": "audio0",
                "evidence_gate": "esp32s3-target-build",
            },
        }
    )
    future_feature = copy.deepcopy(good)
    future_feature["platforms"]["esp32s3"]["compositions"]["native"]["claims"][
        "audio_i2s"
    ] = {
        "maturity": "experimental",
        "evidence": ["esp32s3-target-build"],
        "limitations": "selftest target-build only",
    }
    future_feature["evidence_gates"]["esp32s3-target-build"]["claim_scopes"][0][
        "capabilities"
    ].append("audio_i2s")
    _expect(
        validate(
            future_feature,
            check_runner_bindings=False,
            feature_registry=future_feature_registry,
        )
        == [],
        "a registry-defined capability with an exact binding must need no validator edit",
    )

    no_evidence = copy.deepcopy(good)
    no_evidence["platforms"]["rp2350"]["compositions"]["native"]["claims"]["timebase"][
        "evidence"
    ] = []
    _expect_error(validate(no_evidence), "evidence must be")

    unknown_gate = copy.deepcopy(good)
    unknown_gate["platforms"]["rp2350"]["compositions"]["native"]["claims"]["timebase"][
        "evidence"
    ] = ["paper"]
    _expect_error(validate(unknown_gate), "unknown evidence gate")

    wrong_scope = copy.deepcopy(good)
    wrong_scope["evidence_gates"]["rp2350-target-build"]["claim_scopes"][0][
        "composition"
    ] = "another-stack"
    _expect_error(validate(wrong_scope), "not scoped to this exact claim")

    unrelated_gate = copy.deepcopy(good)
    unrelated_gate["platforms"]["ra4m1"]["compositions"]["native"]["claims"]["spi"] = {
        "maturity": "implemented",
        "evidence": ["ra4m1-provider-host"],
    }
    _expect_error(validate(unrelated_gate), "not scoped to this exact claim")

    paper_claim = copy.deepcopy(good)
    paper_claim["platforms"]["rp2350"].pop("limitations")
    _expect_error(validate(paper_claim), "target-build-only claim")

    fake_physical = copy.deepcopy(good)
    fake_physical["evidence_gates"]["rp2350-target-build"]["kind"] = "physical"
    _expect_error(validate(fake_physical), "unknown evidence kind 'physical'")

    unlocked = copy.deepcopy(good)
    unlocked["evidence_gates"]["rp2350-target-build"]["command"].remove("--locked")
    _expect_error(validate(unlocked), "must use --locked")

    future_composition = copy.deepcopy(good)
    future_composition["platforms"]["ra4m1"]["compositions"]["second-native"] = {
        "surface": "native",
        "claims": {
            "timebase": {
                "maturity": "experimental",
                "evidence": ["ra4m1-second-native-host"],
            }
        },
    }
    future_composition["evidence_gates"]["ra4m1-second-native-host"] = {
        "kind": "host-test",
        "runner": "cross-mcu",
        "required": True,
        "cwd": ".",
        "command": ["python", "-c", "pass"],
        "claim_scopes": [
            {
                "platform": "ra4m1",
                "composition": "second-native",
                "capabilities": ["timebase"],
            }
        ],
    }
    _expect(
        validate(future_composition, check_runner_bindings=False) == [],
        "a separately evidenced future composition must validate",
    )

    missing_root = copy.deepcopy(good)
    missing_root["platforms"]["samd21"]["implementation_root"] = "core/ports/not-present"
    _expect_error(validate(missing_root), "implementation_root")
    file_root = copy.deepcopy(good)
    file_root["platforms"]["samd21"]["implementation_root"] = "README.md"
    _expect_error(validate(file_root), "implementation_root")

    rust_receipts = ["nrf52840-hal-host", "nrf52840-usb-host"]
    _expect(validate_receipts(good, "rust-matrix", rust_receipts) == [], "complete receipts")
    _expect_error(validate_receipts(good, "rust-matrix", []), "missing required")
    _expect_error(
        validate_receipts(good, "rust-matrix", ["ra4m1-target-build"]), "wrong-runner"
    )

    with tempfile.TemporaryDirectory(prefix="nobro-platform-receipts-") as temporary:
        receipt_root = pathlib.Path(temporary)
        source_root = receipt_root / "public-source"
        source_root.mkdir()
        (source_root / ".gitignore").write_text("_work/\n", encoding="utf-8")
        tracked_fixture = source_root / "tracked.txt"
        tracked_fixture.write_text("tracked\n", encoding="utf-8")
        for command in (
            ["git", "init", "--quiet", os.fspath(source_root)],
            [
                "git", "-C", os.fspath(source_root), "config", "user.email",
                "selftest@nobro.invalid",
            ],
            ["git", "-C", os.fspath(source_root), "config", "user.name", "Nobro selftest"],
            ["git", "-C", os.fspath(source_root), "add", ".gitignore", "tracked.txt"],
            [
                "git", "-C", os.fspath(source_root), "-c", "commit.gpgsign=false",
                "commit", "--quiet", "--no-verify", "-m", "fixture",
            ],
        ):
            subprocess.check_call(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        source_fixture = source_root / "untracked-source.txt"
        source_fixture.write_text("before\n", encoding="utf-8")

        baseline_digest = _public_worktree_digest(source_root)
        ignored_fixture = source_root / "_work" / "ignored.txt"
        ignored_fixture.parent.mkdir()
        ignored_fixture.write_text("ignored build output\n", encoding="utf-8")
        _expect(
            _public_worktree_digest(source_root) == baseline_digest,
            "ignored _work content must not alter the public-worktree digest",
        )
        tracked_fixture.write_text("tracked edit\n", encoding="utf-8")
        _expect(
            _public_worktree_digest(source_root) != baseline_digest,
            "tracked edits must alter the public-worktree digest",
        )
        tracked_fixture.write_text("tracked\n", encoding="utf-8")
        _expect(
            _public_worktree_digest(source_root) == baseline_digest,
            "restoring tracked content must restore the public-worktree digest",
        )
        source_fixture.write_text("changed untracked content\n", encoding="utf-8")
        _expect(
            _public_worktree_digest(source_root) != baseline_digest,
            "nonignored untracked content must alter the public-worktree digest",
        )
        source_fixture.write_text("before\n", encoding="utf-8")
        subprocess.check_call(
            [
                "git", "-C", os.fspath(source_root), "-c", "commit.gpgsign=false",
                "commit", "--quiet", "--no-verify", "--allow-empty", "-m", "new-head",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        _expect(
            _public_worktree_digest(source_root) != baseline_digest,
            "a different HEAD must alter the public-worktree digest",
        )

        _expect(
            _quiet_call(begin_receipts, good, "rust-matrix", receipt_root, source_root) == 0,
            "begin receipts",
        )
        with mock.patch.object(
            sys.modules[__name__], "_host_target", return_value="x86_64-selftest"
        ), mock.patch.object(subprocess, "run", return_value=mock.Mock(returncode=0)) as run:
            _expect(
                _quiet_call(
                    execute_gate, good, "nrf52840-hal-host", receipt_root, source_root
                ) == 0,
                "HAL gate success",
            )
            _expect(
                _quiet_call(
                    execute_gate, good, "nrf52840-usb-host", receipt_root, source_root
                ) == 0,
                "USB gate success",
            )
            first_command = run.call_args_list[0].args[0]
            _expect(isinstance(first_command, list), "gate command must execute as an argv list")
            _expect("--locked" in first_command, "host evidence must execute locked")
            _expect(
                run.call_args_list[0].kwargs["cwd"] == ROOT / "core",
                "gate must execute from its exact declared cwd",
            )
        _expect(
            assert_runner_receipts(good, "rust-matrix", receipt_root, source_root) == [],
            "fresh complete runner receipts must validate",
        )

        _expect(
            _quiet_call(
                begin_receipts, good, "arduino-package", receipt_root, source_root
            ) == 0,
            "begin Arduino receipts",
        )
        with mock.patch.object(subprocess, "run", return_value=mock.Mock(returncode=0)) as run:
            _expect(
                _quiet_call(
                    execute_gate, good, "arduino-facade-contract", receipt_root, source_root
                ) == 0,
                "Arduino facade gate success",
            )
            _expect(
                _quiet_call(
                    execute_gate, good, "arduino-ra4m1-compile", receipt_root, source_root
                ) == 0,
                "Arduino compile gate success",
            )
            compile_call = run.call_args_list[1]
            _expect(
                compile_call.args[0][0] == sys.executable,
                "declared python commands must use the active interpreter",
            )
            _expect(
                compile_call.kwargs["env"]["NOBRO_ARDUINO_FQBNS"]
                == good["evidence_gates"]["arduino-ra4m1-compile"]["environment"][
                    "NOBRO_ARDUINO_FQBNS"
                ],
                "gate environment must match the declaration exactly",
            )
        _expect(
            assert_runner_receipts(good, "arduino-package", receipt_root, source_root) == [],
            "Arduino runner receipts must validate",
        )

        receipt_directory = _receipt_directory("rust-matrix", receipt_root)
        receipt = receipt_directory / "nrf52840-hal-host.json"
        original_record = json.loads(receipt.read_text(encoding="utf-8"))
        for field, value in (
            ("schema", "wrong-schema"),
            ("runner", "wrong-runner"),
            ("session_id", "f" * 32),
            ("matrix_digest", "0" * 64),
            ("worktree_digest", "0" * 64),
            ("gate_digest", "0" * 64),
        ):
            altered = dict(original_record)
            altered[field] = value
            receipt.write_text(json.dumps(altered), encoding="utf-8")
            _expect_error(
                assert_runner_receipts(good, "rust-matrix", receipt_root, source_root),
                "stale or altered receipt",
            )
        receipt.write_text(json.dumps(original_record), encoding="utf-8")

        wrong_filename = receipt_directory / "copied-receipt.json"
        receipt.replace(wrong_filename)
        _expect_error(
            assert_runner_receipts(good, "rust-matrix", receipt_root, source_root),
            "receipt filename does not match",
        )
        wrong_filename.replace(receipt)

        active_path = receipt_directory / ".active"
        original_active_text = active_path.read_text(encoding="utf-8")
        active_path.write_text("not json", encoding="utf-8")
        _expect_error(
            assert_runner_receipts(good, "rust-matrix", receipt_root, source_root),
            "invalid active session",
        )
        active_path.write_text(original_active_text, encoding="utf-8")
        missing_active = receipt_directory / ".inactive"
        active_path.replace(missing_active)
        _expect_error(
            assert_runner_receipts(good, "rust-matrix", receipt_root, source_root),
            "no active clean",
        )
        missing_active.replace(active_path)

        _expect(
            _quiet_call(begin_receipts, good, "rust-matrix", receipt_root, source_root) == 0,
            "restart receipts",
        )
        with mock.patch.object(
            sys.modules[__name__], "_host_target", return_value="x86_64-selftest"
        ), mock.patch.object(subprocess, "run", return_value=mock.Mock(returncode=0)):
            _expect(
                _quiet_call(
                    execute_gate, good, "nrf52840-hal-host", receipt_root, source_root
                ) == 0,
                "seed receipt",
            )
        with mock.patch.object(
            sys.modules[__name__], "_host_target", return_value="x86_64-selftest"
        ), mock.patch.object(subprocess, "run", return_value=mock.Mock(returncode=7)):
            _expect(
                _quiet_call(
                    execute_gate, good, "nrf52840-hal-host", receipt_root, source_root
                ) == 7,
                "failed gate",
            )
        _expect(not receipt.is_file(), "a failed gate must remove its prior receipt")

        active_path = _receipt_directory("rust-matrix", receipt_root) / ".active"
        active = json.loads(active_path.read_text(encoding="utf-8"))
        active["matrix_digest"] = "0" * 64
        active_path.write_text(json.dumps(active), encoding="utf-8")
        _expect_error(
            assert_runner_receipts(good, "rust-matrix", receipt_root, source_root),
            "active session",
        )

        no_session_root = receipt_root / "no-session"
        with mock.patch.object(subprocess, "run") as run:
            _expect(
                _quiet_call(
                    execute_gate,
                    good,
                    "arduino-facade-contract",
                    no_session_root,
                    source_root,
                )
                == 1,
                "gate without begin must fail",
            )
            _expect(not run.called, "gate without an active session must not execute")

        mutation_root = receipt_root / "mutation-receipts"
        source_fixture.write_text("before\n", encoding="utf-8")
        _expect(
            _quiet_call(
                begin_receipts, good, "arduino-package", mutation_root, source_root
            ) == 0,
            "begin source-mutation receipts",
        )
        source_fixture.write_text("changed before execution\n", encoding="utf-8")
        with mock.patch.object(subprocess, "run") as run:
            _expect(
                _quiet_call(
                    execute_gate,
                    good,
                    "arduino-facade-contract",
                    mutation_root,
                    source_root,
                ) == 1,
                "a source change after begin must reject execution",
            )
            _expect(not run.called, "stale source must fail before the gate command runs")
        _expect_error(
            assert_runner_receipts(good, "arduino-package", mutation_root, source_root),
            "stale or altered active session",
        )

        source_fixture.write_text("before\n", encoding="utf-8")
        _expect(
            _quiet_call(
                begin_receipts, good, "arduino-package", mutation_root, source_root
            ) == 0,
            "restart source-mutation receipts",
        )

        def mutate_source_after_gate(*_args, **_kwargs):
            source_fixture.write_text("changed during execution\n", encoding="utf-8")
            return mock.Mock(returncode=0)

        with mock.patch.object(subprocess, "run", side_effect=mutate_source_after_gate) as run:
            _expect(
                _quiet_call(
                    execute_gate,
                    good,
                    "arduino-facade-contract",
                    mutation_root,
                    source_root,
                ) == 1,
                "a source change during a successful command must reject its receipt",
            )
            _expect(run.called, "the post-execution source check must follow the command")
        mutation_receipt = (
            _receipt_directory("arduino-package", mutation_root)
            / "arduino-facade-contract.json"
        )
        _expect(
            not mutation_receipt.exists(),
            "a gate that changes public source must not leave a receipt",
        )
        _expect_error(
            assert_runner_receipts(good, "arduino-package", mutation_root, source_root),
            "stale or altered active session",
        )

        try:
            _receipt_directory("../escape", receipt_root)
        except ValueError:
            pass
        else:
            raise AssertionError("unsafe runner path must be rejected")

        outside = receipt_root / "outside"
        outside.mkdir()
        link = receipt_root / "linked-runner"
        try:
            link.symlink_to(outside, target_is_directory=True)
        except OSError:
            pass
        else:
            try:
                _receipt_directory("linked-runner", receipt_root)
            except RuntimeError:
                pass
            else:
                raise AssertionError("symlinked runner directory must be rejected")

    print("PLATFORM TIERS SELFTEST: PASS")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--run-gate", metavar="GATE", help="execute one declared gate command")
    parser.add_argument(
        "--begin-receipts", metavar="RUNNER", help="clear and start one runner receipt session"
    )
    parser.add_argument(
        "--assert-receipts", metavar="RUNNER",
        help="validate the current runner session's successful evidence receipts",
    )
    args = parser.parse_args()
    matrix = json.loads(MATRIX.read_text(encoding="utf-8"))

    if args.selftest:
        return selftest()
    if args.begin_receipts:
        return begin_receipts(matrix, args.begin_receipts)
    if args.run_gate:
        return execute_gate(matrix, args.run_gate)
    if args.assert_receipts:
        errors = assert_runner_receipts(matrix, args.assert_receipts)
    else:
        errors = validate(matrix)
    for error in errors:
        print(f"PLATFORM TIERS: {error}")
    if errors:
        print("RESULT: FAIL")
        return 1

    if args.assert_receipts:
        print(f"RESULT: PASS ({args.assert_receipts} evidence receipts complete)")
    else:
        deep = [name for name, spec in matrix["platforms"].items() if spec["tier"] == "deep"]
        providers = [name for name, spec in matrix["platforms"].items() if spec["tier"] == "provider"]
        print(
            f"RESULT: PASS ({len(matrix['platforms'])} platforms; "
            f"deep={', '.join(deep)}; provider-tier={', '.join(providers)})"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
