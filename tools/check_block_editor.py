#!/usr/bin/env python3
"""Static validation for the NobroRTOS block editor package."""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[1]
PKG = ROOT / "packages" / "block-editor"
sys.path.insert(0, str(ROOT / "tools"))
import nobro_app  # noqa: E402  (shared app.json validator - keeps editor + catalog in sync)


def require(condition: bool, message: str, errors: list[str]) -> None:
    if not condition:
        errors.append(message)


def _local_leak_needles() -> list[str]:
    """Machine-local identifiers that must never appear in shipped package text.
    Loaded from an untracked file so the identifiers themselves stay out of the
    public tree; absent file = no extra needles (public clones skip this)."""
    from pathlib import Path
    f = Path(__file__).with_name("leak_needles.local.txt")
    if not f.exists():
        return []
    return [ln.strip() for ln in f.read_text(encoding="utf-8").splitlines()
            if ln.strip() and not ln.startswith("#")]


def main() -> int:
    errors: list[str] = []
    required = ["index.html", "styles.css", "app.js", "README.md"]
    for name in required:
        require((PKG / name).is_file(), f"missing {name}", errors)

    html = (PKG / "index.html").read_text(encoding="utf-8") if (PKG / "index.html").exists() else ""
    js = (PKG / "app.js").read_text(encoding="utf-8") if (PKG / "app.js").exists() else ""
    css = (PKG / "styles.css").read_text(encoding="utf-8") if (PKG / "styles.css").exists() else ""

    for asset in re.findall(r'(?:href|src)="([^"]+)"', html):
        if asset.startswith(("http://", "https://", "#")):
            continue
        require((PKG / asset).is_file(), f"missing linked asset {asset}", errors)

    for token in ["appJson", "downloadJson", "actuators", "sensors", "behaviors", "board",
                  "ai_models", "mlModels", "loadModels"]:
        require(token in js, f"missing editor token {token}", errors)
    require("@media" in css, "responsive CSS missing", errors)
    for needle in _local_leak_needles():
        require(needle not in html + js + css, "machine-local identifier leaked", errors)

    sample = {
        "name": "hello_device",
        "board": "nrf52840",
        "actuators": [{"name": "arm", "brand": "sg90", "channel": 0}],
        "sensors": [{"name": "imu", "brand": "mpu6050", "bus": "i2c", "address": "0x68"}],
        "behaviors": ["sweep actuator when imu detects a tap"],
        "ai_models": [{"model_id": 0x4E4E4D31, "backend": "on_device",
                       "input_bytes_max": 64, "output_bytes_max": 4, "arena_bytes": 256,
                       "timeout_us": 2000, "stale_after_us": 100000}],
    }
    # The editor's ai_models output must pass the shared app.json validator unchanged.
    require(not nobro_app.validate(json.loads(json.dumps(sample))),
            "sample ML app.json fails nobro_app.validate", errors)

    # models.json is the checked-in ML-block catalog. Verify that every card names a
    # tracked public model artifact and carries an AiModelContract-shaped entry.
    models_path = PKG / "models.json"
    if not models_path.is_file():
        errors.append("missing models.json (regenerate the checked-in model assets)")
    else:
        try:
            cards = json.loads(models_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            cards = {}
            errors.append(f"models.json invalid JSON: {exc}")
        if not isinstance(cards, dict):
            errors.append("models.json must contain an object of model cards")
            cards = {}
        tracked = set(subprocess.run(
            ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
        ).stdout.splitlines())
        for preset, card in cards.items():
            require(isinstance(card, dict), f"model {preset}: card must be an object", errors)
            if not isinstance(card, dict):
                continue
            require(card.get("preset") == preset,
                    f"model {preset}: preset must match its catalog key", errors)
            require(isinstance(card.get("label"), str) and bool(card["label"].strip()),
                    f"model {preset}: label must be a non-empty string", errors)
            source = card.get("source")
            source_is_text = isinstance(source, str) and bool(source.strip())
            require(source_is_text, f"model {preset}: source must be a repo-relative path", errors)
            if source_is_text:
                source_path = PurePosixPath(source)
                source_is_safe = (
                    not source_path.is_absolute()
                    and bool(source_path.parts)
                    and "\\" not in source
                    and "\0" not in source
                    and not re.match(r"^[A-Za-z]:", source)
                    and all(part not in ("", ".", "..") for part in source_path.parts)
                )
                require(source_is_safe,
                        f"model {preset}: source must be a safe repo-relative path", errors)
                if source_is_safe:
                    normalized = source_path.as_posix()
                    source_file = ROOT / Path(*source_path.parts)
                    source_is_contained = source_file.resolve().is_relative_to(ROOT.resolve())
                    require(source_is_contained,
                            f"model {preset}: source resolves outside the repository", errors)
                    require(normalized in tracked,
                            f"model {preset}: source is not tracked: {normalized}", errors)
                    require(source_is_contained and source_file.is_file(),
                            f"model {preset}: source is missing: {normalized}", errors)
            contract = card.get("contract")
            require(isinstance(contract, dict), f"model {preset}: missing contract", errors)
            if isinstance(contract, dict):
                probe = {"board": "nrf52840", "ai_models": [contract]}
                require(not nobro_app.validate(probe),
                        f"model {preset}: contract fails validation", errors)

    print({"package": "block-editor", "files": required, "errors": errors, "ok": not errors})
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
