#!/usr/bin/env python3
"""Gate the post-Wave-50 ecosystem/resource truth matrix.

The matrix records what is absent as deliberately as what exists. A future wave that
adds an adapter/provider/benchmark must update the row in the same commit; otherwise
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
    rows = matrix.get("integrations", []) + matrix.get("benchmarks", [])
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
    duplicate["benchmarks"][0]["id"] = duplicate["integrations"][0]["id"]
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
    rows = matrix.get("integrations", []) + matrix.get("benchmarks", [])
    counts = {status: sum(row.get("status") == status for row in rows)
              for status in sorted(VALID_STATUS)}
    counts = {status: count for status, count in counts.items() if count}
    print(f"ECOSYSTEM MATRIX: {'PASS' if not errors else 'FAIL'} "
          f"({len(rows)} rows; {json.dumps(counts, sort_keys=True)})")
    return int(bool(errors))


if __name__ == "__main__":
    sys.exit(main())
