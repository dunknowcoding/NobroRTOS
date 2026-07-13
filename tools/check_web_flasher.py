#!/usr/bin/env python3
"""Static validation for the NobroRTOS web flasher package."""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WEB = ROOT / "packages" / "web-flasher"


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
        require((WEB / name).is_file(), f"missing {name}", errors)

    html = (WEB / "index.html").read_text(encoding="utf-8") if (WEB / "index.html").exists() else ""
    js = (WEB / "app.js").read_text(encoding="utf-8") if (WEB / "app.js").exists() else ""
    css = (WEB / "styles.css").read_text(encoding="utf-8") if (WEB / "styles.css").exists() else ""

    for asset in re.findall(r'(?:href|src)="([^"]+)"', html):
        if asset.startswith(("http://", "https://", "#")):
            continue
        require((WEB / asset).is_file(), f"missing linked asset {asset}", errors)

    require("navigator.serial" in js, "Web Serial path missing", errors)
    require("navigator.usb" in js, "WebUSB path missing", errors)
    require("requestPort" in js and "requestDevice" in js, "browser pairing calls missing", errors)
    require("crc32" in js and "dropZone" in js, "file inspection/drop path missing", errors)
    require("transferOut" in js, "WebUSB transfer path missing", errors)
    require("parseStatusLine" in js and "reportMonitorBtn" in html, "NOBRO report console missing", errors)
    require("@media" in css, "responsive CSS missing", errors)
    for needle in _local_leak_needles():
        require(needle not in html + js + css, "machine-local identifier leaked", errors)

    print({
        "package": "web-flasher",
        "files": required,
        "errors": errors,
        "ok": not errors,
    })
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
