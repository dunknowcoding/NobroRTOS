#!/usr/bin/env python3
"""UDI surface gate: ImuSal demo must expose exactly three mutually exclusive backends.

Checks that the Universal Driver Interface proof app (udi_imu_demo) carries the
expected backend features, compile-time exclusivity guards, and the public UDI rule
doc. Lab registration and endpoints are intentionally private. No hardware required.

    python tools/check_udi.py
    python tools/check_udi.py --selftest
"""
import argparse
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
UDI_APP = os.path.join(ROOT, "core", "apps", "imu", "udi_imu_demo")
CARGO = os.path.join(UDI_APP, "Cargo.toml")
APP_RS = os.path.join(UDI_APP, "src", "app.rs")
UDI_DOC = os.path.join(ROOT, "docs", "ARCHITECTURE.md")

BACKENDS = ("backend-native", "backend-eh", "backend-arduino")


def check():
    errs = []
    cargo = open(CARGO, encoding="utf-8").read()
    app = open(APP_RS, encoding="utf-8").read()

    for b in BACKENDS:
        if f"{b} =" not in cargo and f'{b} = [' not in cargo:
            errs.append(f"missing Cargo feature {b}")

    if "compile_error!(\"mount exactly one IMU backend" not in app:
        errs.append("missing mutual-exclusion compile_error in app.rs")
    if "ImuSal" not in app:
        errs.append("app.rs does not reference ImuSal trait")

    if not os.path.isfile(UDI_DOC):
        errs.append("docs/ARCHITECTURE.md missing")
    else:
        doc = open(UDI_DOC, encoding="utf-8").read()
        for token in ("ImuSal", "backend-native", "backend-arduino", "category, one trait"):
            if token not in doc:
                errs.append(f"docs/ARCHITECTURE.md missing '{token}'")

    return errs


def selftest():
    errs = check()
    ok = not errs
    print(f"backends        : {', '.join(BACKENDS)}")
    print(f"exclusivity     : {'guarded' if ok or 'compile_error' not in str(errs) else 'MISSING'}")
    print(f"errors          : {errs or 'none'}")
    print(f"RESULT: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser(description="UDI surface gate.")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    return selftest()


if __name__ == "__main__":
    sys.exit(main())
