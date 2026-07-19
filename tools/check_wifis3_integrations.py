#!/usr/bin/env python3
"""Verify the UNO R4 WiFiS3 adapter, target build, and zero-disabled cost."""

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
BACKEND_ID = "backend-wifi-arduino-wifis3"
COMPONENT_ID = "adapter-wireless-wifi-arduino-wifis3"
LIBRARY_ID = "library-arduino-wifis3"
GATE_ID = "arduino-wifis3-target-build"
BINDING_ID = "binding-wifi-arduino-wifis3-ra4m1"
SOURCE_PIN = "424e86eff92d37f72123c2b641dd8bbf06a38b47"
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

DISABLED = r'''#define NOBRO_WIFI_S3_DISABLED 1
#include <NobroArduinoWiFiS3.h>
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
#include <NobroArduinoWiFiS3.h>

nobro::ArduinoWiFiS3Stack wifi;
nobro::NobroApp<2, 1> app;
volatile bool exerciseProvider = false;

void setup() {
  auto source = app.sensor("source", 10);
  auto sink = app.service("sink", 20);
  app.wire(source, sink);
  if (exerciseProvider) {
    nobro_wifi_network_t networks[2] = {};
    size_t count = 0;
    const uint8_t ssid[] = {'n', 'o', 'b', 'r', 'o'};
    const uint8_t secret[] = {'r', 'u', 'n', 't', 'i', 'm', 'e', '1'};
    const nobro_wifi_credentials_t credentials = {
        ssid, sizeof(ssid), secret, sizeof(secret)};
    wifi.mount();
    wifi.scan(networks, 2, count);
    const uint64_t now = micros();
    wifi.join(credentials, now, now + 1000);
    wifi.poll();
    wifi.leave();
    wifi.quiesce();
    wifi.recover();
  }
  Serial.begin(115200);
  const nobro_stack_identity_t identity = wifi.identity();
  Serial.println(app.admit() ? "NOBRO:READY" : app.errorCode());
  Serial.println(identity.backend_id);
  Serial.println(wifi.staticRamBytes());
  Serial.println(wifi.vendorManagedHeap() ? "vendor-heap" : "no-vendor-heap");
}
void loop() {}
'''

FORBIDDEN_DISABLED = (
    "ArduinoWiFiS3Stack",
    "CWifi",
    "ModemClass",
    "WiFiS3",
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
    cli: str,
    root: pathlib.Path,
    name: str,
    source: str,
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
        raise RuntimeError(f"disabled WiFiS3 adapter retained symbols: {hits}")


def record(records: list[dict], identifier: str, label: str) -> dict:
    matches = [item for item in records if item.get("id") == identifier]
    if len(matches) != 1:
        raise RuntimeError(f"{label} {identifier!r} is missing or duplicated")
    return matches[0]


def verify_metadata() -> None:
    features = json.loads(FEATURES.read_text(encoding="utf-8"))
    catalog = json.loads(CATALOG.read_text(encoding="utf-8"))
    tiers = json.loads(TIERS.read_text(encoding="utf-8"))

    provenance = record(features["provenance"], "source-arduino-renesas", "provenance")
    if provenance != {
        "id": "source-arduino-renesas",
        "source": "https://github.com/arduino/ArduinoCore-renesas",
        "revision": SOURCE_PIN,
        "version": "1.6.0",
        "license": "MIT",
    }:
        raise RuntimeError("Arduino Renesas provenance is stale")

    backend = record(features["backends"], BACKEND_ID, "backend")
    if (
        backend.get("adapter_component_id") != COMPONENT_ID
        or backend.get("capability_kind") != "wifi_link"
        or backend.get("stack_family") != "wifi"
        or backend.get("maturity") != "compile-only"
        or backend.get("evidence") != ["target-build"]
        or backend.get("provenance_id") != "source-arduino-renesas"
        or backend.get("supported_targets") != [FQBN]
        or "physical" in backend.get("evidence", [])
    ):
        raise RuntimeError("WiFiS3 backend claim exceeds or differs from compile evidence")

    binding = record(features["bindings"], BINDING_ID, "binding")
    if (
        binding.get("backend_id") != BACKEND_ID
        or binding.get("platform") != "ra4m1"
        or binding.get("composition") != "arduino"
        or binding.get("instance") != "wifi0"
        or binding.get("maturity") != "compile-only"
        or binding.get("evidence_gates") != [GATE_ID]
        or binding.get("price_state") != "unmeasured"
        or any(key.startswith("measured_") for key in binding)
        or binding.get("report_wiring")
        != {
            "provider_id": "wifi_link",
            "status_field": "ra4m1_wifi0",
            "evidence_gate": GATE_ID,
        }
        or set(
            binding.get("disabled_symbol_gate", {}).get("forbidden_symbols", [])
        )
        != set(FORBIDDEN_DISABLED)
    ):
        raise RuntimeError("WiFiS3 exact compile-only binding is stale or falsely priced")

    component = record(catalog["components"], COMPONENT_ID, "component")
    library = record(catalog["components"], LIBRARY_ID, "library")
    if (
        component.get("path")
        != "core/adapters/wireless/wifi/arduino-wifis3"
        or component.get("maturity") != "compile-only"
        or component.get("evidence") != ["target-build"]
        or component.get("supported_targets") != [FQBN]
        or library.get("facade") != "packages/arduino/src/NobroArduinoWiFiS3.h"
        or library.get("provenance_id") != "source-arduino-renesas"
        or library.get("maturity") != "compile-only"
    ):
        raise RuntimeError("WiFiS3 catalog relationship is stale")

    gate = tiers.get("evidence_gates", {}).get(GATE_ID)
    claim = (
        tiers.get("platforms", {})
        .get("ra4m1", {})
        .get("compositions", {})
        .get("arduino", {})
        .get("claims", {})
        .get("wifi_link")
    )
    if (
        not isinstance(gate, dict)
        or gate.get("command") != ["python", "tools/check_wifis3_integrations.py"]
        or gate.get("runner") != "arduino-package"
        or not isinstance(claim, dict)
        or claim.get("maturity") != "experimental"
        or claim.get("evidence") != [GATE_ID]
        or not claim.get("limitations")
    ):
        raise RuntimeError("WiFiS3 tier claim or receipt gate is stale")

    header = (PACKAGE / "src" / "NobroArduinoWiFiS3.h").read_text(encoding="utf-8")
    for token in (
        "#if !defined(NOBRO_WIFI_S3_DISABLED)",
        '#include <WiFiS3.h>',
        "vendorManagedHeap() const { return true; }",
        "cannot preempt the vendor call",
        "runtime-only",
    ):
        if token not in header:
            raise RuntimeError(f"WiFiS3 facade lacks required boundary {token!r}")


def main() -> int:
    cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
    if not cli:
        print("WIFIS3 INTEGRATIONS: FAIL (arduino-cli not found)")
        return 1
    try:
        verify_metadata()
        with tempfile.TemporaryDirectory(prefix="nobro-wifis3-") as temp:
            root = pathlib.Path(temp)
            baseline = compile_sketch(cli, root, "baseline", BASELINE)
            disabled = compile_sketch(cli, root, "disabled", DISABLED)
            enabled = compile_sketch(cli, root, "enabled", FEATURE)
            if baseline[:2] != disabled[:2]:
                raise RuntimeError(
                    "disabled WiFiS3 facade is not zero-cost: "
                    f"baseline={baseline[:2]} disabled={disabled[:2]}"
                )
            verify_disabled_map(disabled[2])
            if enabled[0] <= baseline[0] or enabled[1] < baseline[1]:
                raise RuntimeError(
                    "enabled WiFiS3 price is not observable: "
                    f"baseline={baseline[:2]} enabled={enabled[:2]}"
                )
            print(
                "  PASS zero-disabled "
                f"flash={baseline[0]} ram={baseline[1]}; "
                f"enabled-delta flash={enabled[0] - baseline[0]} "
                f"ram={enabled[1] - baseline[1]}"
            )
    except (OSError, RuntimeError, ValueError) as error:
        print(f"WIFIS3 INTEGRATIONS: FAIL ({error})")
        return 1
    print(
        "WIFIS3 INTEGRATIONS: PASS "
        "(UNO R4 target-build; bounded association; no physical promotion)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
