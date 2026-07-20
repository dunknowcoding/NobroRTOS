#!/usr/bin/env python3
"""Verify the exact UNO R4 ArduinoBLE binding and zero-disabled target cost."""

from __future__ import annotations

import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile


ROOT = pathlib.Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "packages" / "arduino"
FEATURES = ROOT / "core" / "boards" / "feature_providers.json"
CATALOG = ROOT / "core" / "adapters" / "catalog.json"
TIERS = ROOT / "core" / "boards" / "platform_tiers.json"
FQBN = "arduino:renesas_uno:unor4wifi"
BACKEND_ID = "backend-ble-arduino-ble"
COMPONENT_ID = "adapter-wireless-ble-arduino-ble"
LIBRARY_ID = "library-arduino-ble"
GATE_ID = "arduino-ble-unor4-target-build"
BINDING_ID = "binding-ble-arduino-ble-ra4m1"
SOURCE_ID = "source-arduino-ble"
SOURCE_PIN = "281377b3588814e4c174c08ec711e10e35b1c9f9"
SIZE = re.compile(
    r"Sketch uses (?P<flash>\d+) bytes.*?"
    r"Global variables use (?P<ram>\d+) bytes",
    re.DOTALL,
)

BASELINE = r'''#include <NobroRTOS.h>
nobro::NobroApp<2, 1> app;
void setup() {
  auto source = app.sensor("source", 10);
  auto sink = app.service("sink", 20);
  app.wire(source, sink);
  Serial.begin(115200);
  Serial.println(app.admit() ? "NOBRO:READY" : app.errorCode());
}
void loop() {}
'''

DISABLED = r'''#define NOBRO_ARDUINO_BLE_DISABLED 1
#include <NobroArduinoBLE.h>
#include <NobroRTOS.h>
nobro::NobroApp<2, 1> app;
void setup() {
  auto source = app.sensor("source", 10);
  auto sink = app.service("sink", 20);
  app.wire(source, sink);
  Serial.begin(115200);
  Serial.println(app.admit() ? "NOBRO:READY" : app.errorCode());
}
void loop() {}
'''

FEATURE = r'''#include <NobroRTOS.h>
#include <NobroArduinoBLE.h>

nobro::ArduinoBleStack ble;
nobro::NobroApp<2, 1> app;
volatile bool exerciseProvider = false;

void setup() {
  auto source = app.sensor("source", 10);
  auto sink = app.service("sink", 20);
  app.wire(source, sink);
  if (exerciseProvider) {
    const uint8_t advertisement[] = {'n', 'o', 'b', 'r', 'o'};
    const uint8_t response[] = {'o', 'k'};
    nobro_ble_event_t event = {};
    bool available = false;
    ble.mount();
    const uint64_t now = micros();
    ble.advertise(advertisement, sizeof(advertisement), now, now + 1000);
    ble.poll(event, available);
    ble.respondGatt(1, 1, response, sizeof(response));
    ble.disconnect();
    ble.stopAdvertising();
    ble.quiesce();
    ble.recover();
  }
  Serial.begin(115200);
  const nobro_stack_identity_t identity = ble.identity();
  Serial.println(app.admit() ? "NOBRO:READY" : app.errorCode());
  Serial.println(identity.backend_id);
  Serial.println(ble.staticRamBytes());
  Serial.println(ble.vendorManagedHeap() ? "vendor-heap" : "no-vendor-heap");
}
void loop() {}
'''

COEXISTENCE = r'''#include <NobroRTOS.h>
#include <NobroArduinoBLE.h>
#include <NobroArduinoWiFiS3.h>

nobro::ArduinoBleStack ble;
nobro::ArduinoWiFiS3Stack wifi;
volatile bool exerciseProviders = false;

void setup() {
  if (exerciseProviders) {
    const uint8_t advertisement[] = {'c', 'o', 'e', 'x'};
    ble.mount();
    wifi.mount();
    const uint64_t now = micros();
    ble.advertise(advertisement, sizeof(advertisement), now, now + 1000);
    wifi.poll();
    ble.stopAdvertising();
    ble.quiesce();
    wifi.quiesce();
  }
  Serial.begin(115200);
  Serial.println(ble.identity().backend_id);
  Serial.println(wifi.identity().backend_id);
}
void loop() {}
'''

FORBIDDEN_DISABLED = (
    "ArduinoBleStack",
    "BLELocalDevice",
    "HCIVirtualTransportAT",
    "HCITransport",
)


def run(command: list[str]) -> str:
    completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if completed.returncode:
        raise RuntimeError((completed.stdout + completed.stderr).strip())
    return completed.stdout + completed.stderr


def write_sketch(root: pathlib.Path, name: str, source: str) -> pathlib.Path:
    sketch = root / name
    sketch.mkdir()
    (sketch / f"{name}.ino").write_text(source, encoding="utf-8")
    return sketch


def compile_sketch(
    cli: str, root: pathlib.Path, name: str, source: str
) -> tuple[int, int, pathlib.Path]:
    sketch = write_sketch(root, name, source)
    build = root / f"{name}-build"
    output = run(
        [
            cli,
            "compile",
            "--fqbn",
            FQBN,
            "--library",
            str(PACKAGE),
            "--build-cache-path",
            str(root / "cache"),
            "--build-path",
            str(build),
            str(sketch),
        ]
    )
    match = SIZE.search(output)
    if not match:
        raise RuntimeError(f"{name}: Arduino size summary missing")
    return int(match["flash"]), int(match["ram"]), build


def verify_disabled_map(build: pathlib.Path) -> None:
    maps = list(build.glob("*.map"))
    if len(maps) != 1:
        raise RuntimeError("disabled build map is missing or ambiguous")
    text = maps[0].read_text(encoding="utf-8", errors="replace")
    hits = [symbol for symbol in FORBIDDEN_DISABLED if symbol in text]
    if hits:
        raise RuntimeError(f"disabled ArduinoBLE adapter retained symbols: {hits}")


def record(records: list[dict], identifier: str, label: str) -> dict:
    matches = [item for item in records if item.get("id") == identifier]
    if len(matches) != 1:
        raise RuntimeError(f"{label} {identifier!r} is missing or duplicated")
    return matches[0]


def verify_library_version(cli: str) -> None:
    listing = run([cli, "lib", "list"])
    if not re.search(r"(?m)^ArduinoBLE\s+2\.1\.0(?:\s|$)", listing):
        raise RuntimeError("ArduinoBLE 2.1.0 is not installed")


def verify_metadata() -> None:
    features = json.loads(FEATURES.read_text(encoding="utf-8"))
    catalog = json.loads(CATALOG.read_text(encoding="utf-8"))
    tiers = json.loads(TIERS.read_text(encoding="utf-8"))

    provenance = record(features["provenance"], SOURCE_ID, "provenance")
    if provenance != {
        "id": SOURCE_ID,
        "source": "https://github.com/arduino-libraries/ArduinoBLE",
        "revision": SOURCE_PIN,
        "version": "2.1.0",
        "license": "LGPL-2.1-or-later",
    }:
        raise RuntimeError("ArduinoBLE provenance is stale")

    backend = record(features["backends"], BACKEND_ID, "backend")
    if (
        backend.get("adapter_component_id") != COMPONENT_ID
        or backend.get("capability_kind") != "ble_link"
        or backend.get("stack_family") != "ble"
        or backend.get("maturity") != "implemented"
        or backend.get("evidence") != ["host-test", "physical", "target-build"]
        or backend.get("provenance_id") != SOURCE_ID
        or backend.get("supported_targets") != [FQBN]
        or not backend.get("limitations")
    ):
        raise RuntimeError("ArduinoBLE backend evidence is stale")

    binding = record(features["bindings"], BINDING_ID, "binding")
    expected_fields = {
        "id",
        "backend_id",
        "capability_kind",
        "platform",
        "composition",
        "instance",
        "maturity",
        "evidence_gates",
        "price_state",
        "limitations",
        "disabled_symbol_gate",
        "report_wiring",
    }
    if (
        set(binding) != expected_fields
        or binding.get("backend_id") != BACKEND_ID
        or binding.get("capability_kind") != "ble_link"
        or binding.get("platform") != "ra4m1"
        or binding.get("composition") != "arduino"
        or binding.get("instance") != "ble0"
        or binding.get("maturity") != "implemented"
        or binding.get("evidence_gates") != [GATE_ID]
        or binding.get("price_state") != "unmeasured"
        or not binding.get("limitations")
        or set(binding["disabled_symbol_gate"]["forbidden_symbols"])
        != set(FORBIDDEN_DISABLED)
        or binding.get("report_wiring")
        != {
            "provider_id": "ble_link",
            "status_field": "ra4m1_ble0",
            "evidence_gate": GATE_ID,
        }
    ):
        raise RuntimeError("ArduinoBLE exact unpriced binding is stale")

    component = record(catalog["components"], COMPONENT_ID, "component")
    library = record(catalog["components"], LIBRARY_ID, "library")
    if (
        component.get("path")
        != "core/adapters/wireless/ble/arduino-ble"
        or component.get("maturity") != "implemented"
        or component.get("evidence") != ["host-test", "physical", "target-build"]
        or component.get("supported_targets") != [FQBN]
        or library.get("facade") != "packages/arduino/src/NobroArduinoBLE.h"
        or library.get("provenance_id") != SOURCE_ID
        or library.get("maturity") != "implemented"
        or library.get("evidence") != ["physical", "target-build"]
    ):
        raise RuntimeError("ArduinoBLE catalog relationship is stale")

    gate = tiers.get("evidence_gates", {}).get(GATE_ID)
    claim = (
        tiers.get("platforms", {})
        .get("ra4m1", {})
        .get("compositions", {})
        .get("arduino", {})
        .get("claims", {})
        .get("ble_link")
    )
    if (
        not isinstance(gate, dict)
        or gate.get("command")
        != ["python", "tools/check_arduino_ble_integrations.py"]
        or gate.get("runner") != "arduino-package"
        or not isinstance(claim, dict)
        or claim.get("maturity") != "implemented"
        or claim.get("evidence") != [GATE_ID]
        or not claim.get("limitations")
    ):
        raise RuntimeError("ArduinoBLE tier claim or receipt gate is stale")

    header = (PACKAGE / "src" / "NobroArduinoBLE.h").read_text(
        encoding="utf-8"
    )
    for token in (
        "#if !defined(NOBRO_ARDUINO_BLE_DISABLED)",
        "#if !defined(ARDUINO_UNOR4_WIFI)",
        "#include <ArduinoBLE.h>",
        "only one mounted facade is admitted",
        "vendorManagedHeap() const { return true; }",
        "globalController() const { return true; }",
        "HCIVirtualTransportAT",
        "releaseClearedServiceRetain",
        "CMD(_HCI_END)",
        "nobro_stack_result_t disconnect()",
    ):
        if token not in header:
            raise RuntimeError(
                f"ArduinoBLE facade lacks required boundary {token!r}"
            )


def main() -> int:
    cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
    if not cli:
        print("ARDUINO BLE INTEGRATIONS: FAIL (arduino-cli not found)")
        return 1
    try:
        verify_metadata()
        verify_library_version(cli)
        with tempfile.TemporaryDirectory(prefix="nobro-arduino-ble-") as temp:
            root = pathlib.Path(temp)
            baseline = compile_sketch(cli, root, "baseline", BASELINE)
            disabled = compile_sketch(cli, root, "disabled", DISABLED)
            enabled = compile_sketch(cli, root, "enabled", FEATURE)
            coexistence = compile_sketch(cli, root, "coexistence", COEXISTENCE)
            if baseline[:2] != disabled[:2]:
                raise RuntimeError(
                    "disabled ArduinoBLE facade is not zero-cost: "
                    f"baseline={baseline[:2]} disabled={disabled[:2]}"
                )
            verify_disabled_map(disabled[2])
            if enabled[0] <= baseline[0] or enabled[1] < baseline[1]:
                raise RuntimeError(
                    "enabled ArduinoBLE cost is not observable: "
                    f"baseline={baseline[:2]} enabled={enabled[:2]}"
                )
            print(
                "  PASS zero-disabled "
                f"flash={baseline[0]} ram={baseline[1]}; "
                f"BLE target={enabled[:2]}; coexistence target={coexistence[:2]}"
            )
    except (OSError, RuntimeError, ValueError, KeyError) as error:
        print(f"ARDUINO BLE INTEGRATIONS: FAIL ({error})")
        return 1
    print(
        "ARDUINO BLE INTEGRATIONS: PASS "
        "(official UNO R4 transport; bounded teardown; zero-disabled; target-build)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
