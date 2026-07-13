#!/usr/bin/env python3
"""Static validation for the NobroRTOS block editor package."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PKG = ROOT / "packages" / "block-editor"
sys.path.insert(0, str(ROOT / "tools"))
import nobro_app  # noqa: E402  (shared app.json validator - keeps editor + catalog in sync)


def require(condition: bool, message: str, errors: list[str]) -> None:
    if not condition:
        errors.append(message)


def _local_leak_needles() -> list[str]:
    """Bench-private identifiers that must never appear in shipped package text.
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
        require(needle not in html + js + css, "local-bench identifier leaked", errors)

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

    # models.json is the ML-block catalog train_motion_nn.py emits; verify it is present
    # and each card carries a contract-shaped entry (mirrors AiModelContract fields).
    models_path = PKG / "models.json"
    if not models_path.is_file():
        errors.append("missing models.json (regenerate the checked-in model assets)")
    else:
        try:
            cards = json.loads(models_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            cards = {}
            errors.append(f"models.json invalid JSON: {exc}")
        for preset, card in cards.items():
            contract = (card or {}).get("contract")
            require(isinstance(contract, dict), f"model {preset}: missing contract", errors)
            if isinstance(contract, dict):
                probe = {"board": "nrf52840", "ai_models": [contract]}
                require(not nobro_app.validate(probe),
                        f"model {preset}: contract fails validation", errors)

    print({"package": "block-editor", "files": required, "errors": errors, "ok": not errors})
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
