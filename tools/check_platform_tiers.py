#!/usr/bin/env python3
"""Validate the public platform capability matrix.

`core/boards/platform_tiers.json` declares, per platform, which portable
`nobro_hal` provider contracts it satisfies and at what tier. This gate keeps
that matrix honest and in sync with the tree:

  * every declared platform has a real port directory under `core/ports/`;
  * every `implements` provider is a known provider name;
  * a `provider`- or `deep`-tier platform that claims `timebase` actually has
    a provider module implementing and wiring the timebase (no paper claims);
  * exactly one platform is `deep` (the nRF52840 reference).

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
VALID_TIERS = {"deep", "provider", "core", "absent"}


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
        for provider in spec.get("implements", []):
            if provider not in providers:
                errors.append(f"{name}: unknown provider {provider!r}")

        # A provider/deep platform claiming timebase must actually implement it.
        if spec["tier"] in ("provider", "deep") and "timebase" in spec.get("implements", []):
            if name == "nrf52840":
                impl = (ROOT / "core" / "crates" / "nobro_hal" / "src" / "platform"
                        / "nrf52840" / "mod.rs")
                implementation = impl.is_file() and "impl HalClock" in impl.read_text(encoding="utf-8")
            else:
                port = PORT_DIR.get(name)
                candidates = [
                    PORTS / port / "src" / "portable.rs",
                    PORTS / port / "src" / "providers.rs",
                ] if port else []
                main = PORTS / port / "src" / "main.rs" if port else None
                portable_text = "\n".join(
                    item.read_text(encoding="utf-8") for item in candidates if item.is_file()
                )
                main_text = (main.read_text(encoding="utf-8")
                             if main is not None and main.is_file() else "")
                implementation = (
                    "impl HalClock" in portable_text
                    and "with(HardwareCapability::Timebase)" in portable_text
                    and (
                        "verify_timebase_provider()" in main_text
                        or "Providers::supports(required)" in main_text
                    )
                    and "timebase=" in main_text
                    and "all_pass=" in main_text
                )
            if not implementation:
                errors.append(f"{name}: timebase claim lacks implementation/report wiring")

        if name == "ra4m1" and spec["tier"] == "provider":
            provider = PORTS / "ra4m1" / "src" / "providers.rs"
            facade = ROOT / "packages" / "arduino" / "src" / "NobroArduinoProviders.h"
            provider_text = provider.read_text(encoding="utf-8") if provider.is_file() else ""
            facade_text = facade.read_text(encoding="utf-8") if facade.is_file() else ""
            for token in ("Ra4m1Clock", "Ra4m1Alarm", "Ra4m1Usb"):
                if token not in provider_text:
                    errors.append(f"ra4m1: native provider missing {token}")
            for token in ("ArduinoAdc", "ArduinoPwm", "ArduinoI2c", "ArduinoSpi"):
                if token not in facade_text:
                    errors.append(f"ra4m1: Arduino provider facade missing {token}")

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
    assert any("timebase claim" in e for e in validate(paper)), "paper timebase claim must fail"

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
