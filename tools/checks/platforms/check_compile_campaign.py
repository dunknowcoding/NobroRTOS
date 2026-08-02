#!/usr/bin/env python3
"""Require hosted witnesses for every maintained compiled ecosystem claim."""

from __future__ import annotations

import json
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github" / "workflows" / "gates.yml"
MATRIX = ROOT / "tools" / "checks" / "ci_matrix.sh"


def workflow_steps(text: str) -> dict[str, str]:
    matches = list(re.finditer(r"^      - name: (.+)$", text, re.MULTILINE))
    return {
        match.group(1).strip(): text[match.start() : matches[index + 1].start()]
        if index + 1 < len(matches) else text[match.start() :]
        for index, match in enumerate(matches)
    }


# A target family is public only when its member is compiled in the named
# hosted step. Tokens intentionally identify the exact representative FQBN or
# registered gate, so adding a catalog target cannot silently inherit an
# unrelated build elsewhere in the workflow.
LIBRARY_WITNESSES = {
    "library-arduino-ble": {
        "arduino:renesas_uno:unor4wifi": ("compile public package examples (AVR, UNO R4, ESP32-S3, ESP8266)", "arduino-ble-unor4-target-build"),
    },
    "library-arduino-esp-ble": {
        target: ("compile public package examples (AVR, UNO R4, ESP32-S3, ESP8266)", "arduino-esp-ble-target-build")
        for target in ("esp32:esp32:esp32", "esp32:esp32:esp32c3", "esp32:esp32:esp32s3")
    },
    "library-arduino-esp-wifi": {
        target: ("compile public package examples (AVR, UNO R4, ESP32-S3, ESP8266)", "arduino-esp-wifi-target-build")
        for target in ("esp32:esp32:esp32", "esp32:esp32:esp32c3", "esp32:esp32:esp32s3")
    },
    "library-arduino-wifis3": {
        "arduino:renesas_uno:unor4wifi": ("compile public package examples (AVR, UNO R4, ESP32-S3, ESP8266)", "arduino-wifis3-target-build"),
    },
    "library-arduino-esp8266-wifi": {
        "esp8266:esp8266:d1_mini": ("compile public package examples (AVR, UNO R4, ESP32-S3, ESP8266)", "esp8266-arduino-target-build"),
    },
    "library-difinders": {
        "arduino:avr": ("pinned sensor member integrations", "arduino:avr:uno"),
        "arduino:esp32": ("pinned sensor member integrations", "esp32:esp32:esp32c3"),
        "arduino:nrf52": ("compile pinned sensor member integrations (ArduinoNRF)", "arduinonrf:nrf52:promicro_nrf52840"),
        "arduino:renesas": ("pinned sensor member integrations", "arduino:renesas_uno:unor4wifi"),
        "arduino:samd": ("pinned sensor member integrations", "arduino:samd:mkrzero"),
    },
    "library-ina-series-sensor": {
        "arduino:avr": ("pinned sensor member integrations", "arduino:avr:uno"),
        "arduino:esp32": ("pinned sensor member integrations", "esp32:esp32:esp32c3"),
        "arduino:nrf52": ("compile pinned sensor member integrations (ArduinoNRF)", "arduinonrf:nrf52:promicro_nrf52840"),
        "arduino:renesas": ("pinned sensor member integrations", "arduino:renesas_uno:unor4wifi"),
        "arduino:samd": ("pinned sensor member integrations", "arduino:samd:mkrzero"),
    },
    "library-niusaudio": {
        "arduino:esp32s3": ("compile public package examples (AVR, UNO R4, ESP32-S3, ESP8266)", "esp32s3-arduino-audio-target-build"),
    },
    "library-niusdisplay": {
        "arduino:avr:uno": ("prepare and compile pinned display member", "arduino:avr:uno"),
        "portable-c99": ("prepare and compile pinned display member", "compile_matrix.py --group tests"),
    },
    "library-niuscam": {
        "arduino:esp32": ("pinned camera integration", "camera-arduino-target-build"),
    },
    "library-niuscrypto": {
        "arduino:nrf52": ("compile pinned NiusCrypto integration (ArduinoNRF)", "arduinonrf:nrf52:promicro_nrf52840"),
    },
    "library-niusimu": {
        "arduino:avr": ("pinned NiusIMU integration", "arduino:avr:nano:cpu=atmega328old"),
        "arduino:esp32": ("pinned NiusIMU integration", "esp32:esp32:esp32s3"),
        "arduino:nrf52": ("compile pinned NiusIMU integration (ArduinoNRF)", "arduinonrf:nrf52:promicro_nrf52840"),
        "arduino:renesas": ("pinned NiusIMU integration", "arduino:renesas_uno:unor4wifi"),
        "arduino:samd": ("pinned NiusIMU integration", "arduino:samd:arduino_zero_native"),
    },
    "library-niusthread": {
        "arduino:nrf52": ("compile pinned NiusThread integration (ArduinoNRF)", "arduinonrf:nrf52:promicro_nrf52840"),
    },
    "library-niuswireless": {
        "arduino:avr": ("pinned wireless integrations", "arduino:avr:nano:cpu=atmega328old"),
        "arduino:esp32s3": ("pinned wireless integrations", "esp32:esp32:esp32s3"),
        "arduino:nrf52": ("compile pinned NiusZigbee integration (ArduinoNRF)", "--compile-nrf"),
        "arduino:renesas": ("pinned wireless integrations", "arduino:renesas_uno:unor4wifi"),
        "rp2040:rp2040:rpipico2w": ("pinned wireless integrations", "rp2040:rp2040:rpipico2w"),
        "arduino:samd": ("pinned wireless integrations", "arduino:samd:arduino_zero_native"),
    },
    "library-niuszigbee": {
        "arduino:nrf52": ("compile pinned NiusZigbee integration (ArduinoNRF)", "--compile-zigbee"),
    },
    "library-roboservo": {
        "arduino:avr:uno": ("pinned RoboServo target matrix", "arduino:avr:uno"),
        "rp2040:rp2040:rpipico": ("pinned RoboServo target matrix", "rp2040:rp2040:rpipico"),
        "arduino:renesas_uno:unor4wifi": ("pinned RoboServo target matrix", "arduino:renesas_uno:unor4wifi"),
        "arduinonrf:nrf52:promicro_nrf52840": ("compile pinned RoboServo integration (ArduinoNRF)", "arduinonrf:nrf52:promicro_nrf52840"),
        "esp32:esp32:esp32s3": ("pinned RoboServo target matrix", "esp32:esp32:esp32s3"),
        "esp8266:esp8266:d1_mini": ("pinned RoboServo target matrix", "esp8266:esp8266:d1_mini"),
    },
}

ADAPTER_WITNESSES = {
    "adapter-audio-esp32s3-es8311": ("workflow", "esp32s3-arduino-audio-target-build"),
    "adapter-bus-embedded-hal-i2c": ("matrix", "portable adapter conformance"),
    "adapter-bus-embedded-hal-spi": ("matrix", "portable adapter conformance"),
    "adapter-camera-niuscam": ("workflow", "camera-arduino-target-build"),
    "adapter-display-niusdisplay": ("workflow", "NiusDisplayBounded"),
    "adapter-imu-mpu9250": ("matrix", "portable adapter conformance"),
    "adapter-sensors-esp32-adc-continuous": ("workflow", "esp32-arduino-peripheral-target-build"),
    "adapter-servo-esp32-ledc": ("workflow", "esp32-arduino-peripheral-target-build"),
    "adapter-servo-esp32-rmt": ("workflow", "esp32-arduino-peripheral-target-build"),
    "adapter-servo-roboservo": ("workflow", "RoboServoBounded"),
    "adapter-wireless-ble-arduino-ble": ("workflow", "arduino-ble-unor4-target-build"),
    "adapter-wireless-ble-arduino-esp": ("workflow", "arduino-esp-ble-target-build"),
    "adapter-wireless-radio-comms": ("matrix", "portable adapter conformance"),
    "adapter-wireless-wifi-arduino-esp": ("workflow", "arduino-esp-wifi-target-build"),
    "adapter-wireless-wifi-arduino-wifis3": ("workflow", "arduino-wifis3-target-build"),
    "adapter-wireless-wifi-arduino-esp8266": ("workflow", "esp8266-arduino-target-build"),
    "contract-imu": ("matrix", "portable adapter conformance"),
    "contract-wireless": ("matrix", "portable adapter conformance"),
}


def main() -> int:
    errors: list[str] = []
    workflow = WORKFLOW.read_text(encoding="utf-8")
    matrix = MATRIX.read_text(encoding="utf-8")
    steps = workflow_steps(workflow)
    profiles = [json.loads(path.read_text(encoding="utf-8")) for path in sorted((ROOT / "core/boards").glob("*/*/board.json"))]
    maintained = [item for item in profiles if item["firmware_generation"]["support"] != "unavailable"]
    route_counts: dict[str, int] = {}
    for profile in maintained:
        support = profile["firmware_generation"]["support"]
        route_counts[support] = route_counts.get(support, 0) + 1
    if route_counts != {"application-image": 4, "arduino-composition": 7, "maintained-port": 5}:
        errors.append(f"maintained route coverage drift: {route_counts}")
    for token in ("check_firmware_generation.py --arduino-builds", "check_firmware_generation.py"):
        if token not in workflow + matrix:
            errors.append(f"hosted firmware route witness missing: {token}")

    tiers = json.loads((ROOT / "core/boards/platform_tiers.json").read_text(encoding="utf-8"))
    gates = tiers["evidence_gates"]
    runners = tiers["runners"]
    features = json.loads((ROOT / "core/boards/feature_providers.json").read_text(encoding="utf-8"))
    compiled_bindings = [item for item in features["bindings"] if item["maturity"] in {"compile-only", "implemented"}]
    for binding in compiled_bindings:
        target_gates = []
        for gate_id in binding.get("evidence_gates", []):
            gate = gates.get(gate_id)
            if gate and gate.get("kind") == "target-build":
                target_gates.append(gate_id)
                driver = (ROOT / runners[gate["runner"]]["receipt_driver"]).read_text(encoding="utf-8")
                if f"--run-gate {gate_id}" not in driver:
                    errors.append(f"{binding['id']}: target gate is not invoked by its hosted runner: {gate_id}")
        if not target_gates:
            errors.append(f"{binding['id']}: compiled binding has no target-build gate")

    catalog = json.loads((ROOT / "core/adapters/catalog.json").read_text(encoding="utf-8"))
    target_components = {item["id"]: item for item in catalog["components"] if "target-build" in item.get("evidence", [])}
    expected = set(LIBRARY_WITNESSES) | set(ADAPTER_WITNESSES)
    if set(target_components) != expected:
        errors.append("target-build component witness set drift: missing=" + str(sorted(set(target_components) - expected)) + " extra=" + str(sorted(expected - set(target_components))))
    for component_id, witnesses in LIBRARY_WITNESSES.items():
        component = target_components.get(component_id)
        if not component:
            continue
        if set(component["supported_targets"]) != set(witnesses):
            errors.append(f"{component_id}: supported target/witness mismatch")
        for target, (step_name, token) in witnesses.items():
            step = steps.get(step_name, "")
            if token not in step:
                errors.append(f"{component_id}/{target}: missing {token!r} in hosted step {step_name!r}")
    for component_id, (source, token) in ADAPTER_WITNESSES.items():
        haystack = workflow if source == "workflow" else matrix
        if token not in haystack:
            errors.append(f"{component_id}: missing hosted {source} witness {token!r}")

    negative_tokens = {
        "feature conflict / exact-one": "rp2350 CYW43439 exact-one negative selections",
        "unavailable capability / one-backend": "check_firmware_generation.py",
        "board/boot mismatch": "linker/boot mismatch negative",
        "lifecycle cleanup": "provider-lifecycle-host",
        "no-heap": "wireless no-heap feature tests",
        "optional heap": "wireless adaptive alloc feature tests",
        "resource admission": "sdk/cli/nobro.py admit --selftest",
    }
    combined = workflow + matrix + (ROOT / "tools/checks/platforms/check_board_profiles.py").read_text(encoding="utf-8")
    for scope, token in negative_tokens.items():
        if token not in combined:
            errors.append(f"negative campaign missing {scope}: {token}")

    if errors:
        for error in errors:
            print(f"[FAIL] {error}")
        print(f"COMPILE CAMPAIGN: FAIL ({len(errors)} issue(s))")
        return 1
    print("COMPILE CAMPAIGN: PASS "
          f"({len(maintained)} routes, {len(compiled_bindings)} exact backends, "
          f"{len(target_components)} target-built components, 7 negative scopes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
