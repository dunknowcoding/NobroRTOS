#!/usr/bin/env python3
"""Validate the honest platform capability matrix (PORT-01, Wave 48).

`core/boards/platform_tiers.json` declares, per platform, which portable
`nobro_hal` provider contracts it satisfies and at what tier. This gate keeps
that matrix honest and in sync with the tree:

  * every declared platform has a real port directory under `core/ports/`;
  * every `implements`/`planned` provider is a known provider name;
  * a `provider`- or `deep`-tier platform that claims `timebase` actually has
    a `portable.rs` implementing the timebase provider (no paper claims);
  * exactly one platform is `deep` (the nRF52840 reference) and it is the only
    one claiming automated HIL — the rest must say bench-gated, so breadth is
    never overstated.

    python tools/check_platform_tiers.py            # validate
    python tools/check_platform_tiers.py --selftest # gate

Exit 0 only when the matrix is internally consistent and matches the tree.
"""
import argparse
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
MATRIX = ROOT / "core" / "boards" / "platform_tiers.json"
PORTS = ROOT / "core" / "ports"
PORT_DIR = {  # matrix key -> port directory (they differ where the port predates the key)
    "nrf52840": None,  # the deep HAL lives in nobro-hal, not a port dir
    "rp2350": "rp2350",
    "esp32c3": "esp32c3",
    "esp32s3": "esp32s3",
    "ra4m1": "ra4m1",
    "samd21": "samd21",
    "avr_nano": "avr_nano",
}
VALID_TIERS = {"deep", "provider", "conformance", "absent"}


def validate(matrix: dict) -> list[str]:
    errors = []
    providers = set(matrix["providers"])
    platforms = matrix["platforms"]

    deep = [name for name, p in platforms.items() if p["tier"] == "deep"]
    if deep != ["nrf52840"]:
        errors.append(f"exactly one deep platform (nrf52840) expected, got {deep}")

    for name, spec in platforms.items():
        if spec["tier"] not in VALID_TIERS:
            errors.append(f"{name}: unknown tier {spec['tier']!r}")
        for provider in spec.get("implements", []) + spec.get("planned", []):
            if provider not in providers:
                errors.append(f"{name}: unknown provider {provider!r}")

        # A provider/deep platform claiming timebase must actually implement it.
        if spec["tier"] in ("provider", "deep") and "timebase" in spec.get("implements", []):
            if name == "nrf52840":
                impl = (ROOT / "core" / "crates" / "nobro_hal" / "src" / "platform"
                        / "nrf52840" / "mod.rs")
                proof = impl.is_file() and "impl HalClock" in impl.read_text(encoding="utf-8")
            else:
                port = PORT_DIR.get(name)
                portable = PORTS / port / "src" / "portable.rs" if port else None
                proof = (portable is not None and portable.is_file()
                         and "impl HalClock" in portable.read_text(encoding="utf-8"))
            if not proof:
                errors.append(f"{name}: claims timebase provider but no HalClock impl found")

        # Only the deep platform may claim automated HIL.
        hil = spec.get("hil", "")
        if name != "nrf52840" and "automated" in hil:
            errors.append(f"{name}: only the deep platform may claim automated HIL")
        if name == "nrf52840" and "automated" not in hil:
            errors.append("nrf52840 (deep) must document automated HIL")

        # Declared platforms must have a real port (except the nRF deep HAL).
        port = PORT_DIR.get(name, "MISSING")
        if port == "MISSING":
            errors.append(f"{name}: not mapped to a port directory")
        elif port is not None and not (PORTS / port).is_dir():
            errors.append(f"{name}: port directory core/ports/{port} does not exist")

    return errors


def selftest() -> int:
    # A good matrix passes; injected inconsistencies each fail.
    good = json.loads(MATRIX.read_text(encoding="utf-8"))
    assert validate(good) == [], f"real matrix should be clean: {validate(good)}"

    two_deep = json.loads(MATRIX.read_text(encoding="utf-8"))
    two_deep["platforms"]["rp2350"]["tier"] = "deep"
    assert validate(two_deep), "two deep platforms must fail"

    paper = json.loads(MATRIX.read_text(encoding="utf-8"))
    paper["platforms"]["samd21"]["tier"] = "provider"
    paper["platforms"]["samd21"]["implements"] = ["timebase"]
    assert any("HalClock" in e for e in validate(paper)), "paper timebase claim must fail"

    overclaim = json.loads(MATRIX.read_text(encoding="utf-8"))
    overclaim["platforms"]["rp2350"]["hil"] = "automated on the bench"
    assert validate(overclaim), "non-deep automated-HIL claim must fail"

    print("PLATFORM TIERS SELFTEST: PASS")
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
        print(f"PLATFORM TIERS: {error}")
    if errors:
        print("RESULT: FAIL")
        return 1
    provider_tier = [n for n, p in matrix["platforms"].items() if p["tier"] == "provider"]
    print(f"RESULT: PASS ({len(matrix['platforms'])} platforms; "
          f"deep=nrf52840; provider-tier={', '.join(provider_tier)})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
