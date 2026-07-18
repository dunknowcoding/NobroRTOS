#!/usr/bin/env python3
"""Validate the static task/wire block editor and its canonical fixture."""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[1]
PKG = ROOT / "packages" / "block-editor"
sys.path.insert(0, str(ROOT / "tools"))
import nobro_app  # noqa: E402


def require(condition: bool, message: str, errors: list[str]) -> None:
    if not condition:
        errors.append(message)


def _local_leak_needles() -> list[str]:
    path = Path(__file__).with_name("leak_needles.local.txt")
    if not path.exists():
        return []
    return [
        line.strip()
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.startswith("#")
    ]


def main() -> int:
    errors: list[str] = []
    required = ["index.html", "styles.css", "app.js", "README.md", "models.json"]
    for name in required:
        require((PKG / name).is_file(), f"missing {name}", errors)

    html = (PKG / "index.html").read_text(encoding="utf-8")
    js = (PKG / "app.js").read_text(encoding="utf-8")
    css = (PKG / "styles.css").read_text(encoding="utf-8")
    readme = (PKG / "README.md").read_text(encoding="utf-8")

    for asset in re.findall(r'(?:href|src)="([^"]+)"', html):
        if not asset.startswith(("http://", "https://", "#")):
            require((PKG / asset).is_file(), f"missing linked asset {asset}", errors)
    for token in [
        "nobro-app-v1",
        "tasks",
        "wires",
        "periodic",
        "control",
        "service",
        "capacity",
        "appJson",
        "downloadJson",
    ]:
        require(token in js, f"missing editor contract token {token}", errors)
    for stale in ["actuators", "sensors", "behaviors", "ai_models", "loadModels"]:
        require(stale not in js, f"legacy hardware JSON token remains: {stale}", errors)
    require("@media" in css, "responsive CSS missing", errors)
    require("payload transport" in readme, "task/wire boundary is undocumented", errors)
    for needle in _local_leak_needles():
        require(needle not in html + js + css + readme, "machine-local identifier leaked", errors)

    fixture = json.loads(
        (ROOT / "tutorials" / "hello-device" / "app.json").read_text(encoding="utf-8")
    )
    require(not nobro_app.validate(fixture), "block fixture fails canonical validator", errors)
    require(
        fixture == json.loads(
            (ROOT / "sdk" / "firmware" / "starter-app.json").read_text(encoding="utf-8")
        ),
        "starter and tutorial app fixtures drift",
        errors,
    )

    node = subprocess.run(
        ["where.exe", "node"], capture_output=True, text=True
    ) if sys.platform == "win32" else subprocess.run(
        ["sh", "-c", "command -v node"], capture_output=True, text=True
    )
    if node.returncode == 0:
        checked = subprocess.run(
            ["node", "--check", str(PKG / "app.js")],
            capture_output=True,
            text=True,
        )
        require(checked.returncode == 0, checked.stderr.strip() or "app.js syntax error", errors)

    cards = json.loads((PKG / "models.json").read_text(encoding="utf-8"))
    require(isinstance(cards, dict), "models.json must contain an object", errors)
    tracked = set(
        subprocess.run(
            ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
        ).stdout.splitlines()
    )
    required_model_fields = {
        "model_id",
        "backend",
        "input_bytes_max",
        "output_bytes_max",
        "arena_bytes",
        "timeout_us",
        "stale_after_us",
    }
    for preset, card in cards.items():
        require(isinstance(card, dict), f"model {preset}: card must be an object", errors)
        if not isinstance(card, dict):
            continue
        require(card.get("preset") == preset, f"model {preset}: preset mismatch", errors)
        source = card.get("source")
        source_path = PurePosixPath(source) if isinstance(source, str) else None
        safe = (
            source_path is not None
            and not source_path.is_absolute()
            and all(part not in ("", ".", "..") for part in source_path.parts)
            and "\\" not in source
        )
        require(safe, f"model {preset}: unsafe source", errors)
        if safe:
            require(source_path.as_posix() in tracked, f"model {preset}: source not tracked", errors)
        contract = card.get("contract")
        require(
            isinstance(contract, dict) and set(contract) == required_model_fields,
            f"model {preset}: invalid contract shape",
            errors,
        )

    print({"package": "block-editor", "files": required, "errors": errors, "ok": not errors})
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
