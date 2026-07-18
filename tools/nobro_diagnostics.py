"""Load the versioned NobroRTOS public diagnostic registry."""

from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REGISTRY_PATH = ROOT / "sdk" / "error-codes.json"


def entries() -> tuple[dict[str, str], ...]:
    document = json.loads(REGISTRY_PATH.read_text(encoding="utf-8"))
    if document.get("schema") != "nobro-error-codes-v1":
        raise ValueError("unsupported NobroRTOS error-code registry")
    values = document.get("codes")
    if not isinstance(values, list):
        raise ValueError("NobroRTOS error-code registry needs a codes array")
    return tuple(values)


def surface(name: str) -> dict[str, tuple[str, str]]:
    return {
        item["key"]: (item["code"], item["message"])
        for item in entries()
        if item["surface"] == name
    }
