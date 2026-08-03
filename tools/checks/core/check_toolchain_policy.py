#!/usr/bin/env python3
"""Enforce declared MSRV inheritance and pinned hosted toolchains."""

from __future__ import annotations

from pathlib import Path
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[3]
CORE = ROOT / "core"
WORKFLOW = ROOT / ".github" / "workflows" / "gates.yml"
MSRV = "1.85"

STANDALONE = {
    "ports/avr_nano/Cargo.toml": MSRV,
    "ports/esp32/Cargo.toml": MSRV,
    "ports/esp32c3/Cargo.toml": MSRV,
    "ports/esp32p4/Cargo.toml": "1.97",
    "ports/esp32s3/Cargo.toml": MSRV,
    "ports/ra4m1/Cargo.toml": MSRV,
    "ports/rp2040/Cargo.toml": MSRV,
    "ports/rp2350/Cargo.toml": MSRV,
    "ports/samd21/Cargo.toml": MSRV,
}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def load(path: Path) -> dict:
    return tomllib.loads(path.read_text(encoding="utf-8"))


def main() -> int:
    workspace = load(CORE / "Cargo.toml")
    require(workspace["workspace"]["package"]["rust-version"] == MSRV, "workspace MSRV drift")
    members = workspace["workspace"]["members"]
    for member in members:
        manifest = CORE / member / "Cargo.toml"
        package = load(manifest)["package"]
        require(package.get("rust-version", {}).get("workspace") is True,
                f"{manifest.relative_to(ROOT)}: must inherit workspace MSRV")
    for relative, expected in STANDALONE.items():
        package = load(CORE / relative)["package"]
        require(package.get("rust-version") == expected,
                f"core/{relative}: expected rust-version {expected}")

    workflow = WORKFLOW.read_text(encoding="utf-8")
    require("rustup toolchain install 1.97.0" in workflow, "hosted stable toolchain is not pinned")
    require("rustup toolchain install 1.85.0" in workflow, "hosted MSRV verification is missing")
    require("nightly-2026-07-03" in workflow, "hosted nightly toolchain is not pinned")
    print(f"toolchain policy: PASS (workspace MSRV {MSRV}, {len(members)} inherited packages)")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (KeyError, OSError, TypeError, ValueError) as error:
        print(f"toolchain policy: FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
