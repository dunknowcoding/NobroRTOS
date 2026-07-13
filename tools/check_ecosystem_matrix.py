#!/usr/bin/env python3
"""Gate the post-Wave-50 ecosystem/resource truth matrix.

The matrix records what is absent as deliberately as what exists. A future wave that
adds an adapter/provider must update the row in the same commit; otherwise
this gate fails instead of allowing prose and source to drift apart.
"""

import argparse
import copy
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
MATRIX = ROOT / "core" / "ecosystem" / "integration_matrix.json"
VALID_STATUS = {
    "absent", "c_abi_only", "host_only", "heartbeat_only", "conformance",
    "physical_ad_hoc", "integrated", "provider", "physical_hil",
}


def validate(matrix: dict, root: pathlib.Path = ROOT) -> list[str]:
    errors: list[str] = []
    if matrix.get("schema") != "nobro-ecosystem-matrix-v1":
        errors.append("wrong or missing schema")
    domain_ids: list[str] = []
    for domain in matrix.get("domains", []):
        domain_id = domain.get("id")
        domain_ids.append(domain_id)
        contract = domain.get("contract_crate", "")
        if not contract.startswith("core/crates/") or not (root / contract / "Cargo.toml").is_file():
            errors.append(f"{domain_id}: contract_crate must be an existing core/crates module")
        source = root / contract / "src" / "lib.rs"
        source_text = source.read_text(encoding="utf-8") if source.is_file() else ""
        for token in ("pub trait ImuBackend", "pub struct ImuSample", "pub struct ImuCalibration", "pub struct ImuDiagnostics"):
            if token not in source_text:
                errors.append(f"{domain_id}: contract lacks {token}")
        for token in ("embedded_hal", "write_read(", "REG_WHO_AM_I"):
            if token in source_text:
                errors.append(f"{domain_id}: hardware implementation leaked into the domain contract: {token}")
        c_contract = root / domain.get("c_contract", "")
        c_text = c_contract.read_text(encoding="utf-8") if c_contract.is_file() else ""
        for token in ("NOBRO_IMU_API_VERSION 0x0100u", "NOBRO_IMU_CALIBRATION_MAGIC 0x4D49u", "nobro_imu_sample_t", "nobro_imu_diagnostics_t"):
            if token not in c_text:
                errors.append(f"{domain_id}: C contract lacks {token}")
        family_ids: list[str] = []
        for family in domain.get("sensor_families", []):
            family_ids.append(family.get("id"))
            adapter = family.get("adapter", "")
            if family.get("status") not in VALID_STATUS:
                errors.append(f"{domain_id}/{family.get('id')}: invalid family status")
            if not adapter.startswith("core/adapters/") or not (root / adapter / "Cargo.toml").is_file():
                errors.append(f"{domain_id}/{family.get('id')}: adapter must be an existing core/adapters module")
            elif "nobro-imu" not in (root / adapter / "Cargo.toml").read_text(encoding="utf-8"):
                errors.append(f"{domain_id}/{family.get('id')}: adapter does not depend on the domain crate")
        if len(family_ids) != len(set(family_ids)) or any(not item for item in family_ids):
            errors.append(f"{domain_id}: sensor-family ids must be non-empty and unique")
        library_ids: list[str] = []
        for library in domain.get("library_members", []):
            library_ids.append(library.get("id"))
            status = library.get("status")
            adapter = library.get("adapter", "")
            if status not in VALID_STATUS:
                errors.append(f"{domain_id}/{library.get('id')}: invalid library status")
            exists = (root / adapter).exists()
            if status == "absent" and exists:
                errors.append(f"{domain_id}/{library.get('id')}: absent adapter exists")
            if status != "absent" and not exists:
                errors.append(f"{domain_id}/{library.get('id')}: integrated adapter is missing")
        if len(library_ids) != len(set(library_ids)) or any(not item for item in library_ids):
            errors.append(f"{domain_id}: library-member ids must be non-empty and unique")
    if len(domain_ids) != len(set(domain_ids)) or any(not item for item in domain_ids):
        errors.append("domain ids must be non-empty and unique")
    if (root / "core" / "ecosystems").exists():
        errors.append("core/ecosystems duplicates crates/adapters/apps ownership")
    if "benchmarks" in matrix:
        errors.append("maintainer comparisons must not appear in the public ecosystem matrix")
    rows = matrix.get("integrations", [])
    ids = [row.get("id") for row in rows]
    if any(not isinstance(item, str) or not item for item in ids):
        errors.append("every row needs a non-empty id")
    if len(set(ids)) != len(ids):
        errors.append("row ids must be unique")
    for row in rows:
        row_id = row.get("id", "?")
        status = row.get("status")
        if status not in VALID_STATUS:
            errors.append(f"{row_id}: unknown status {status!r}")
        evidence = row.get("evidence_path")
        if evidence:
            path = root / evidence
            if not path.is_file():
                errors.append(f"{row_id}: missing evidence_path {evidence}")
                text = ""
            else:
                text = path.read_text(encoding="utf-8")
            for token in row.get("required_tokens", []):
                if token not in text:
                    errors.append(f"{row_id}: evidence lacks required token {token!r}")
            for token in row.get("forbidden_tokens", []):
                if token in text:
                    errors.append(f"{row_id}: evidence contains forbidden token {token!r}")
        closure = row.get("closure_path")
        expected = row.get("closure_path_state")
        if closure or expected:
            if not closure or expected not in {"present", "absent"}:
                errors.append(f"{row_id}: closure path/state must be paired")
            else:
                exists = (root / closure).exists()
                if exists != (expected == "present"):
                    errors.append(
                        f"{row_id}: {closure} is {'present' if exists else 'absent'}, "
                        f"matrix says {expected}"
                    )
        review = row.get("review")
        if not isinstance(review, str) or "-" not in review:
            errors.append(f"{row_id}: missing review finding id")
    return errors


def selftest() -> int:
    good = json.loads(MATRIX.read_text(encoding="utf-8"))
    assert not validate(good), validate(good)
    duplicate = copy.deepcopy(good)
    duplicate["integrations"][1]["id"] = duplicate["integrations"][0]["id"]
    assert any("unique" in error for error in validate(duplicate))
    bad_status = copy.deepcopy(good)
    bad_status["integrations"][0]["status"] = "marketing-complete"
    assert any("unknown status" in error for error in validate(bad_status))
    bad_token = copy.deepcopy(good)
    bad_token["integrations"][0]["required_tokens"] = ["not really present"]
    assert any("required token" in error for error in validate(bad_token))
    print("ECOSYSTEM MATRIX SELFTEST: PASS")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    matrix = json.loads(MATRIX.read_text(encoding="utf-8"))
    errors = validate(matrix)
    for error in errors:
        print(f"ECOSYSTEM MATRIX: {error}")
    rows = matrix.get("integrations", [])
    counts = {status: sum(row.get("status") == status for row in rows)
              for status in sorted(VALID_STATUS)}
    counts = {status: count for status, count in counts.items() if count}
    print(f"ECOSYSTEM MATRIX: {'PASS' if not errors else 'FAIL'} "
          f"({len(rows)} rows; {json.dumps(counts, sort_keys=True)})")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
