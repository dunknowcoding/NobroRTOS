#!/usr/bin/env python3
"""Reject ambiguous report-checksum names and unauthenticated trust claims."""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys


ROOT = pathlib.Path(__file__).resolve().parents[3]
RUST_ROOTS = (
    ROOT / "core" / "crates",
    ROOT / "core" / "adapters",
    ROOT / "core" / "boards",
    ROOT / "core" / "ports",
    ROOT / "core" / "apps",
)
DIAGNOSTIC_REPORT_TYPES = (
    "HealthReport",
    "RuntimeReport",
    "ModuleRuntimeReport",
    "DegradeApplicationReport",
    "EventLogReport",
    "AdmissionReport",
    "ManifestReport",
    "ExecutorTimingReport",
    "CapacityReport",
    "BoardProfileReport",
    "BoardPackageReport",
    "AdapterCompatibilityReport",
    "RosBridgeContractReport",
    "AiModelContractReport",
    "ImuHealthReport",
)
OLD_IDENTIFIERS = (
    "verify_checksum",
    "nobro_report_checksum_words",
    "nobro_report_status_from_checksum",
    "seal_report",
)


def tracked_files() -> tuple[pathlib.Path, ...]:
    raw = subprocess.run(
        ["git", "ls-files", "-z"], cwd=ROOT, check=True, capture_output=True
    ).stdout
    return tuple(
        ROOT / item.decode("utf-8")
        for item in raw.split(b"\0")
        if item
    )


def rust_report_errors(path: pathlib.Path, text: str) -> list[str]:
    errors: list[str] = []
    report_structs = {
        match.group("name"): match.group("body")
        for match in re.finditer(
            r"\b(?:pub\s+)?struct\s+(?P<name>(?:[A-Za-z0-9_]*Report|Report))"
            r"(?:\s*<[^{}]+>)?\s*\{(?P<body>.*?)\n\}",
            text,
            re.DOTALL,
        )
    }
    for name, body in report_structs.items():
        if re.search(r"\b(?:pub\s+)?checksum\s*:", body):
            errors.append(f"{path}: {name} exposes ambiguous checksum field")

    for report_type in DIAGNOSTIC_REPORT_TYPES:
        body = report_structs.get(report_type)
        if body is None:
            continue
        if "pub diagnostic_checksum:" not in body:
            errors.append(f"{path}: {report_type} lacks diagnostic_checksum field")

        impl = re.search(
            rf"\bimpl(?:\s*<[^{{>]+>)?\s+{re.escape(report_type)}"
            rf"(?:\s*<[^{{>]+>)?\s*\{{(?P<body>.*?)(?=\n\}}\n)",
            text,
            re.DOTALL,
        )
        if impl is None:
            continue
        impl_body = impl.group("body")
        if "diagnostic_checksum_matches" not in impl_body:
            errors.append(
                f"{path}: {report_type} lacks diagnostic_checksum_matches"
            )
        if re.search(r"\bpub\s+fn\s+seal\s*\(", impl_body):
            errors.append(f"{path}: {report_type} exports ambiguous seal method")
    return errors


def audit() -> list[str]:
    errors: list[str] = []
    tracked = tracked_files()
    rust_files = [
        path
        for path in tracked
        if path.suffix == ".rs"
        and any(path.is_relative_to(root) for root in RUST_ROOTS)
        and path.is_file()
    ]
    for path in rust_files:
        text = path.read_text(encoding="utf-8")
        errors.extend(rust_report_errors(path.relative_to(ROOT), text))
        for identifier in OLD_IDENTIFIERS[:1]:
            if re.search(rf"\b{re.escape(identifier)}\b", text):
                errors.append(
                    f"{path.relative_to(ROOT)}: ambiguous report API {identifier}"
                )

    c_header = (ROOT / "bindings/c/include/nobro_rtos.h").read_text(encoding="utf-8")
    python_reports = (
        ROOT / "bindings/python/nobro_rtos/reports.py"
    ).read_text(encoding="utf-8")
    combined_surface = c_header + "\n" + python_reports
    for identifier in OLD_IDENTIFIERS[1:]:
        if re.search(rf"\b{re.escape(identifier)}\b", combined_surface):
            errors.append(f"public report surface retains ambiguous API {identifier}")
    for required in (
        "diagnostic_checksum",
        "nobro_report_diagnostic_checksum_words",
        "nobro_report_status_from_diagnostic_checksum",
        "diagnostic_checksum_matches",
        "finalize_diagnostic_report",
    ):
        if required not in combined_surface:
            errors.append(f"public report surface lacks {required}")

    secure = (
        ROOT / "core/crates/nobro_secure/src/security_v2.rs"
    ).read_text(encoding="utf-8")
    for required in (
        "pub struct VerifiedReportPayload",
        "pub fn open<'a>",
        "Option<VerifiedReportPayload<'a>>",
    ):
        if required not in secure:
            errors.append(f"authenticated report boundary lacks {required}")

    public_docs = "\n".join(
        path.read_text(encoding="utf-8")
        for path in (
            ROOT / "README.md",
            ROOT / "docs/API.md",
            ROOT / "docs/USER_GUIDE.md",
            ROOT / "docs/ARCHITECTURE.md",
            ROOT / "bindings/python/README.md",
            ROOT / "tutorials/03-arduino-and-python/README.md",
        )
    )
    for ambiguous in (
        "sealed checksum",
        "seals its checksum",
        "self-verifying reports",
    ):
        if ambiguous in public_docs.casefold():
            errors.append(f"public docs retain ambiguous phrase {ambiguous!r}")
    for required in (
        "do not authenticate a report",
        "AuthenticatedReportEnvelope::open",
        "attacker-controlled boundary",
    ):
        if required.casefold() not in public_docs.casefold():
            errors.append(f"public docs lack trust-boundary statement {required!r}")
    return errors


def selftest() -> None:
    bad = """
pub struct HealthReport {
    pub checksum: u32,
}
impl HealthReport {
    pub fn seal(&mut self) {}
    pub fn verify_checksum(&self) -> bool { true }
}
"""
    found = rust_report_errors(pathlib.Path("fixture.rs"), bad)
    if len(found) < 4 or not any("ambiguous checksum field" in item for item in found):
        raise RuntimeError(f"report trust-boundary selftest failed: {found}")


def main() -> int:
    selftest()
    errors = audit()
    for error in errors:
        print(f"FAIL: {error}")
    print(
        "REPORT TRUST BOUNDARY: "
        f"{'PASS' if not errors else 'FAIL'} "
        f"(all tracked Rust report structs; "
        f"{len(DIAGNOSTIC_REPORT_TYPES)} typed contract families)"
    )
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
