#!/usr/bin/env python3
"""Static validation for the NobroRTOS block editor package."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PKG = ROOT / "packages" / "block-editor"


def require(condition: bool, message: str, errors: list[str]) -> None:
    if not condition:
        errors.append(message)


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

    for token in ["appJson", "downloadJson", "actuators", "sensors", "behaviors", "board"]:
        require(token in js, f"missing editor token {token}", errors)
    require("@media" in css, "responsive CSS missing", errors)
    require("IronEngineWorld" not in html + js + css, "local Python environment leaked", errors)

    sample = {
        "name": "hello_device",
        "board": "nrf52840",
        "actuators": [{"name": "arm", "brand": "sg90", "channel": 0}],
        "sensors": [{"name": "imu", "brand": "mpu6050", "bus": "i2c", "address": "0x68"}],
        "behaviors": ["sweep actuator when imu detects a tap"],
    }
    require(json.loads(json.dumps(sample))["board"] == "nrf52840", "sample app JSON invalid", errors)

    print({"package": "block-editor", "files": required, "errors": errors, "ok": not errors})
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
