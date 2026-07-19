#!/usr/bin/env python3
"""Verify the pinned Arduino-ESP32 WiFi facade and zero-disabled cost."""

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
FQBNS = (
    "esp32:esp32:esp32c3",
    "esp32:esp32:esp32",
    "esp32:esp32:esp32s3",
)
SUPPORTED_TARGETS = (
    "esp32:esp32:esp32",
    "esp32:esp32:esp32c3",
    "esp32:esp32:esp32s3",
)
BACKEND_ID = "backend-wifi-arduino-esp"
COMPONENT_ID = "adapter-wireless-wifi-arduino-esp"
LIBRARY_ID = "library-arduino-esp-wifi"
GATE_ID = "arduino-esp-wifi-target-build"
BINDING_ID = "binding-wifi-arduino-esp-esp32c3"
SOURCE_ID = "source-arduino-esp32"
SOURCE_PIN = "0d1440d1be38ab530d274fe87ee88565fe167392"
SIZE = re.compile(
    r"Sketch uses (?P<flash>\d+) bytes.*?"
    r"Global variables use (?P<ram>\d+) bytes",
    re.DOTALL,
)

BASELINE = r"""#include <NobroRTOS.h>
nobro::NobroApp<2, 1> app;
void setup() {
  auto source = app.sensor("source", 10);
  auto sink = app.service("sink", 20);
  app.wire(source, sink);
  Serial.begin(115200);
  Serial.println(app.admit() ? "NOBRO:READY" : app.errorCode());
}
void loop() {}
"""

DISABLED = r"""#define NOBRO_ESP_WIFI_DISABLED 1
#include <NobroArduinoEspWiFi.h>
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
"""

FEATURE = r"""#include <NobroRTOS.h>
#include <NobroArduinoEspWiFi.h>

nobro::ArduinoEspWiFiStack wifi;
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
"""

FORBIDDEN_DISABLED = (
    "ArduinoEspWiFiStack",
    "WiFiClass",
    "WiFiGenericClass",
    "esp_wifi_init",
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
    fqbn: str,
) -> tuple[int, int, pathlib.Path]:
    sketch = write_sketch(root, name, source)
    build = root / f"{name}-build"
    output = run(
        [
            cli,
            "compile",
            "--fqbn",
            fqbn,
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
        raise RuntimeError(f"disabled Arduino-ESP32 WiFi retained symbols: {hits}")


def record(records: list[dict], identifier: str, label: str) -> dict:
    matches = [item for item in records if item.get("id") == identifier]
    if len(matches) != 1:
        raise RuntimeError(f"{label} {identifier!r} is missing or duplicated")
    return matches[0]


def verify_metadata() -> None:
    features = json.loads(FEATURES.read_text(encoding="utf-8"))
    catalog = json.loads(CATALOG.read_text(encoding="utf-8"))
    tiers = json.loads(TIERS.read_text(encoding="utf-8"))
    targets = list(SUPPORTED_TARGETS)

    expected_source = {
        "id": SOURCE_ID,
        "source": "https://github.com/espressif/arduino-esp32",
        "revision": SOURCE_PIN,
        "version": "3.3.10",
        "license": "LGPL-2.1-or-later",
    }
    if record(features["provenance"], SOURCE_ID, "provenance") != expected_source:
        raise RuntimeError("Arduino-ESP32 provenance is stale")
    catalog_source = record(catalog["provenance"], SOURCE_ID, "catalog provenance")
    if catalog_source != {**expected_source, "pinned": True, "clean": True}:
        raise RuntimeError("Arduino-ESP32 catalog provenance is stale")

    backend = record(features["backends"], BACKEND_ID, "backend")
    if (
        backend.get("adapter_component_id") != COMPONENT_ID
        or backend.get("capability_kind") != "wifi_link"
        or backend.get("stack_family") != "wifi"
        or backend.get("maturity") != "implemented"
        or backend.get("evidence")
        != ["host-test", "target-build", "physical"]
        or backend.get("provenance_id") != SOURCE_ID
        or backend.get("supported_targets") != targets
    ):
        raise RuntimeError("Arduino-ESP32 WiFi backend evidence is stale")

    binding = record(features["bindings"], BINDING_ID, "binding")
    if (
        binding.get("backend_id") != BACKEND_ID
        or binding.get("platform") != "esp32c3"
        or binding.get("composition") != "arduino"
        or binding.get("instance") != "wifi0"
        or binding.get("maturity") != "compile-only"
        or binding.get("evidence_gates") != [GATE_ID]
        or binding.get("price_state") != "unmeasured"
        or any(key.startswith("measured_") for key in binding)
        or binding.get("report_wiring")
        != {
            "provider_id": "wifi_link",
            "status_field": "esp32c3_wifi0",
            "evidence_gate": GATE_ID,
        }
        or set(binding.get("disabled_symbol_gate", {}).get("forbidden_symbols", []))
        != set(FORBIDDEN_DISABLED)
    ):
        raise RuntimeError("ESP32-C3 WiFi binding is stale or falsely priced")

    component = record(catalog["components"], COMPONENT_ID, "component")
    library = record(catalog["components"], LIBRARY_ID, "library")
    if (
        component.get("path") != "core/adapters/wireless/wifi/arduino-esp"
        or component.get("maturity") != "implemented"
        or component.get("evidence")
        != ["host-test", "physical", "target-build"]
        or component.get("supported_targets") != targets
        or library.get("facade") != "packages/arduino/src/NobroArduinoEspWiFi.h"
        or library.get("provenance_id") != SOURCE_ID
        or library.get("maturity") != "implemented"
        or library.get("evidence") != ["physical", "target-build"]
        or library.get("supported_targets") != targets
    ):
        raise RuntimeError("Arduino-ESP32 WiFi catalog relationship is stale")

    gate = tiers.get("evidence_gates", {}).get(GATE_ID)
    claim = (
        tiers.get("platforms", {})
        .get("esp32c3", {})
        .get("compositions", {})
        .get("arduino", {})
        .get("claims", {})
        .get("wifi_link")
    )
    if (
        not isinstance(claim, dict)
        or claim.get("maturity") != "experimental"
        or claim.get("evidence") != [GATE_ID]
        or not claim.get("limitations")
    ):
        raise RuntimeError("ESP32-C3 WiFi tier claim is stale")
    if (
        not isinstance(gate, dict)
        or gate.get("command") != ["python", "tools/check_arduino_esp_wifi.py"]
        or gate.get("runner") != "arduino-package"
    ):
        raise RuntimeError("Arduino-ESP32 WiFi receipt gate is stale")

    header = (PACKAGE / "src" / "NobroArduinoEspWiFi.h").read_text(
        encoding="utf-8"
    )
    for token in (
        "#if !defined(NOBRO_ESP_WIFI_DISABLED)",
        "#if !defined(ARDUINO_ARCH_ESP32)",
        "#include <WiFi.h>",
        "#include <esp_wifi.h>",
        "WiFi.persistent(false);",
        "WiFi.STA.begin(false)",
        "esp_wifi_scan_start(&config, true)",
        "esp_wifi_scan_get_ap_record(&record)",
        "clearFailedAssociation()",
        "cleared != ESP_ERR_WIFI_STATE",
        "vendorManagedHeap() const { return true; }",
        "runtime-only",
    ):
        if token not in header:
            raise RuntimeError(
                f"Arduino-ESP32 WiFi facade lacks required boundary {token!r}"
            )


def main() -> int:
    cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
    if not cli:
        print("ARDUINO ESP WIFI: FAIL (arduino-cli not found)")
        return 1
    try:
        verify_metadata()
        with tempfile.TemporaryDirectory(prefix="nobro-arduino-esp-wifi-") as temp:
            root = pathlib.Path(temp)
            baseline = compile_sketch(
                cli, root, "baseline", BASELINE, "esp32:esp32:esp32c3"
            )
            disabled = compile_sketch(
                cli, root, "disabled", DISABLED, "esp32:esp32:esp32c3"
            )
            if baseline[:2] != disabled[:2]:
                raise RuntimeError(
                    "disabled Arduino-ESP32 WiFi facade is not zero-cost: "
                    f"baseline={baseline[:2]} disabled={disabled[:2]}"
                )
            verify_disabled_map(disabled[2])

            enabled_sizes: list[tuple[str, int, int]] = []
            for index, fqbn in enumerate(FQBNS):
                enabled = compile_sketch(
                    cli, root, f"enabled-{index}", FEATURE, fqbn
                )
                enabled_sizes.append((fqbn, enabled[0], enabled[1]))
            c3_enabled = enabled_sizes[0]
            if c3_enabled[1] <= baseline[0] or c3_enabled[2] < baseline[1]:
                raise RuntimeError(
                    "enabled ESP32-C3 WiFi price is not observable: "
                    f"baseline={baseline[:2]} enabled={c3_enabled[1:]}"
                )

            print(
                "  PASS zero-disabled "
                f"flash={baseline[0]} ram={baseline[1]}; "
                f"C3 enabled-delta flash={c3_enabled[1] - baseline[0]} "
                f"ram={c3_enabled[2] - baseline[1]}"
            )
            for fqbn, flash, ram in enabled_sizes:
                print(f"  PASS {fqbn} enabled flash={flash} ram={ram}")
    except (OSError, RuntimeError, ValueError) as error:
        print(f"ARDUINO ESP WIFI: FAIL ({error})")
        return 1
    print(
        "ARDUINO ESP WIFI: PASS "
        "(pinned 3.3.10 family target-build; physical backend, unpriced binding)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
