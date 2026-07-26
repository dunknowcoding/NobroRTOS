#!/usr/bin/env python3
"""Validate public claim-to-check linkage against the canonical gate list."""

from __future__ import annotations

import importlib.util
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
MANIFEST = ROOT / "sdk" / "claim-checks.json"
WORKFLOW = ROOT / ".github" / "workflows" / "gates.yml"
ALLOWED_STATUS = {"verified", "limited"}
REQUIRED_PUBLIC_DOCS = {
    "README.md",
    "docs/README.md",
    "docs/GETTING_STARTED.md",
    "docs/USER_GUIDE.md",
    "docs/API.md",
    "docs/ARCHITECTURE.md",
    "docs/PORTING.md",
    "docs/LIMITATIONS.md",
    "docs/CAMERA_SUPPORT.md",
    "docs/WIRELESS_SUPPORT.md",
    "docs/ERROR_CODES.md",
    "docs/api-index.md",
}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def load_run_checks():
    path = ROOT / "tools" / "checks" / "run_checks.py"
    spec = importlib.util.spec_from_file_location("nobro_run_checks", path)
    require(spec is not None and spec.loader is not None, "cannot load run_checks")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def tracked_paths() -> set[PurePosixPath]:
    result = subprocess.run(
        ["git", "ls-files", "-z"], cwd=ROOT, check=True, capture_output=True
    )
    return {
        PurePosixPath(raw.decode("utf-8").replace("\\", "/"))
        for raw in result.stdout.split(b"\0")
        if raw
    }


def command_receipt(command: list[str]) -> str | None:
    for item in command:
        normalized = item.replace("\\", "/")
        if normalized.startswith(("tools/", "sdk/")) and normalized.endswith(".py"):
            return normalized
    return None


def main() -> int:
    document = json.loads(MANIFEST.read_text(encoding="utf-8"))
    require(document.get("schema") == "nobro-claim-checks-v1", "schema drift")
    claims = document.get("claims")
    require(isinstance(claims, list) and claims, "claims must be a non-empty list")

    run_checks = load_run_checks()
    specs = {
        name: command
        for name, command, _cwd in run_checks.gate_specs(quick=True)
    }
    workflow = WORKFLOW.read_text(encoding="utf-8")
    tracked = tracked_paths()
    seen: set[str] = set()
    covered_surfaces: set[str] = set()

    for claim in claims:
        claim_id = claim.get("id")
        require(isinstance(claim_id, str) and claim_id, "claim id missing")
        require(claim_id not in seen, f"duplicate claim id: {claim_id}")
        seen.add(claim_id)
        require(claim.get("status") in ALLOWED_STATUS, f"{claim_id}: invalid status")
        gates = claim.get("gates")
        surfaces = claim.get("surfaces")
        require(isinstance(gates, list) and gates, f"{claim_id}: no gates")
        require(isinstance(surfaces, list) and surfaces, f"{claim_id}: no surfaces")
        for surface in surfaces:
            path = PurePosixPath(surface)
            require(path in tracked, f"{claim_id}: untracked/missing surface {surface}")
            covered_surfaces.add(surface)
        for gate in gates:
            require(gate in specs, f"{claim_id}: unknown local gate {gate}")
            receipt = command_receipt(specs[gate])
            require(receipt is not None, f"{claim_id}: gate has no product command")
            require(
                re.search(
                    rf"^\s*(?:run:\s*)?(?:python|py)(?:\s+-O)?\s+"
                    rf"{re.escape(receipt)}(?:\s|$)",
                    workflow,
                    re.MULTILINE,
                )
                is not None,
                f"{claim_id}: {gate} is not invoked by hosted CI",
            )
    missing_docs = REQUIRED_PUBLIC_DOCS - covered_surfaces
    require(not missing_docs, f"public docs lack claim-gate coverage: {sorted(missing_docs)}")

    print(
        f"CLAIM CONTRACT: PASS ({len(claims)} claims, "
        f"{sum(len(item['gates']) for item in claims)} links)"
    )
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"CLAIM CONTRACT: FAIL ({error})")
        sys.exit(1)
