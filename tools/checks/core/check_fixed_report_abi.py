#!/usr/bin/env python3
"""Check the frozen C/Rust/Python layouts of versioned fixed reports.

The checked-in JSON is an ABI baseline, not generated documentation. A layout
change therefore fails until the author deliberately assigns a new version and
keeps the older decoder represented as a separate versioned type.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[3]
HEADER = ROOT / "bindings" / "c" / "include" / "nobro_rtos.h"
RUST = ROOT / "core" / "crates" / "nobro_host" / "src" / "lib.rs"
BASELINE = ROOT / "sdk" / "fixed-report-abi.json"
PYTHON_REPORTS = ROOT / "bindings" / "python" / "nobro_rtos" / "reports.py"

RUST_TYPES = {
    "nobro_board_profile_report": "BoardProfileReport",
    "nobro_board_package_report": "BoardPackageReport",
    "nobro_manifest_report": "ManifestReport",
    "nobro_adapter_compat_report": "AdapterCompatibilityReport",
    "nobro_ai_model_report": "AiModelReport",
    "nobro_ros_bridge_report": "RosBridgeReport",
    "nobro_admission_report": "AdmissionReport",
    "nobro_runtime_report": "RuntimeReport",
    "nobro_health_report_v1": "HealthReportV1",
    "nobro_health_report": "HealthReport",
    "nobro_backend_operation_report": "BackendOperationReport",
    "nobro_event_log_report": "EventLogReport",
    "nobro_module_runtime_report": "ModuleRuntimeReport",
    "nobro_degrade_application_report": "DegradeApplicationReport",
}

PYTHON_FIELDS = {
    "nobro_board_profile_report": "BOARD_PROFILE_FIELDS",
    "nobro_board_package_report": "BOARD_PACKAGE_FIELDS",
    "nobro_manifest_report": "MANIFEST_FIELDS",
    "nobro_adapter_compat_report": "ADAPTER_COMPAT_FIELDS",
    "nobro_ai_model_report": "AI_MODEL_FIELDS",
    "nobro_ros_bridge_report": "ROS_BRIDGE_FIELDS",
    "nobro_admission_report": "ADMISSION_FIELDS",
    "nobro_runtime_report": "RUNTIME_FIELDS",
    "nobro_health_report_v1": "HEALTH_V1_FIELDS",
    "nobro_health_report": "HEALTH_FIELDS",
    "nobro_backend_operation_report": "BACKEND_OPERATION_FIELDS",
    "nobro_event_log_report": "EVENT_LOG_FIELDS",
    "nobro_module_runtime_report": "MODULE_RUNTIME_FIELDS",
    "nobro_degrade_application_report": "DEGRADE_APPLICATION_FIELDS",
}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def constants(text: str) -> dict[str, int]:
    found = {}
    for name, value in re.findall(r"^#define\s+(NOBRO_[A-Z0-9_]+)\s+(0x[0-9A-Fa-f]+|[0-9]+)u?$", text, re.M):
        found[name] = int(value, 0)
    return found


def report_macro(stem: str, suffix: str) -> str:
    name = stem.removeprefix("nobro_").upper()
    if name == "HEALTH_REPORT_V1":
        name = "HEALTH_REPORT"
    return f"NOBRO_{name}_{suffix}"


def rust_fields(text: str, type_name: str) -> list[str]:
    match = re.search(rf"pub struct {re.escape(type_name)}\s*\{{(?P<body>.*?)\n\}}", text, re.S)
    require(match is not None, f"missing Rust mirror {type_name}")
    fields = re.findall(r"^\s*pub\s+([a-zA-Z0-9_]+):\s*u32,", match.group("body"), re.M)
    require(fields, f"{type_name}: fixed report must contain u32 fields")
    return fields


def load_python_module():
    package_root = ROOT / "bindings" / "python"
    sys.path.insert(0, str(package_root))
    spec = importlib.util.spec_from_file_location("nobro_rtos.reports", PYTHON_REPORTS)
    require(spec is not None and spec.loader is not None, "cannot load Python report decoder")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def current_contract() -> dict:
    header = HEADER.read_text(encoding="utf-8")
    rust = RUST.read_text(encoding="utf-8")
    macros = constants(header)
    python = load_python_module()
    records = []
    pattern = re.compile(
        r"typedef struct (?P<stem>nobro_[a-z0-9_]+_report(?:_v1)?)\s*\{"
        r"(?P<body>.*?)\}\s*(?P<alias>nobro_[a-z0-9_]+_report(?:_v1)?_t);",
        re.S,
    )
    parsed = {}
    for match in pattern.finditer(header):
        stem = match.group("stem")
        if stem not in RUST_TYPES:
            continue
        fields = re.findall(r"^\s*uint32_t\s+([a-zA-Z0-9_]+);", match.group("body"), re.M)
        require(fields[-1:] == ["diagnostic_checksum"], f"{stem}: checksum must be last")
        require(rust_fields(rust, RUST_TYPES[stem]) == fields, f"{stem}: Rust layout drift")
        python_fields = list(getattr(python, PYTHON_FIELDS[stem]))
        require(python_fields == fields, f"{stem}: Python layout drift")
        magic_name = report_macro(stem, "MAGIC")
        version_name = "NOBRO_REPORT_VERSION"
        if stem == "nobro_health_report":
            version_name = "NOBRO_HEALTH_REPORT_VERSION"
        elif stem == "nobro_health_report_v1":
            version_name = "NOBRO_HEALTH_REPORT_VERSION_V1"
        require(magic_name in macros, f"{stem}: missing {magic_name}")
        require(version_name in macros, f"{stem}: missing {version_name}")
        record = {
            "name": stem,
            "c_type": match.group("alias"),
            "rust_type": RUST_TYPES[stem],
            "magic": f"0x{macros[magic_name]:08X}",
            "version": macros[version_name],
            "size": len(fields) * 4,
            "alignment": 4,
            "fields": [
                {"name": name, "type": "u32", "offset": index * 4}
                for index, name in enumerate(fields)
            ],
        }
        key = (record["magic"], record["version"])
        require(key not in parsed, f"duplicate magic/version identity: {stem}")
        parsed[key] = stem
        records.append(record)
    require(set(RUST_TYPES) == {item["name"] for item in records}, "fixed-report set drift")
    records.sort(key=lambda item: (item["magic"], item["version"], item["name"]))
    return {"schema": "nobro-fixed-report-abi-v1", "word_size": 4, "reports": records}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    current = current_contract()
    text = json.dumps(current, indent=2) + "\n"
    if args.write:
        BASELINE.write_text(text, encoding="utf-8", newline="\n")
        print(f"wrote {BASELINE.relative_to(ROOT)}")
        return 0
    require(BASELINE.is_file(), "missing sdk/fixed-report-abi.json")
    require(BASELINE.read_text(encoding="utf-8") == text, "fixed-report ABI drift; add a new version and preserve old decoder")
    print(f"fixed-report ABI: PASS ({len(current['reports'])} versioned layouts)")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, AttributeError) as error:
        print(f"fixed-report ABI: FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
