#!/usr/bin/env python3
"""Verify the pinned Arduino-ESP32 BLE facade and zero-disabled targets."""

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
SOURCE_ID = "source-arduino-esp32"
SOURCE_PIN = "0d1440d1be38ab530d274fe87ee88565fe167392"
BACKEND_ID = "backend-ble-arduino-esp"
COMPONENT_ID = "adapter-wireless-ble-arduino-esp"
LIBRARY_ID = "library-arduino-esp-ble"
GATE_ID = "arduino-esp-ble-target-build"
TARGETS = (
    ("esp32", "esp32:esp32:esp32", "bluedroid", "esp_bluedroid_init"),
    ("esp32c3", "esp32:esp32:esp32c3", "nimble", "nimble_port_init"),
    ("esp32s3", "esp32:esp32:esp32s3", "nimble", "nimble_port_init"),
)
SUPPORTED_TARGETS = tuple(sorted(target[1] for target in TARGETS))
FORBIDDEN_DISABLED = (
    "ArduinoEspBleStack",
    "BLEDevice",
    "BLEServer",
    "esp_bluedroid_init",
    "nimble_port_init",
)
EXPECTED_C3_WORKLOAD = {
    "namespace": "esp32c3-arduino-wifi-ble-gatt-http",
    "configuration_words": [
        160, 4, 20, 8, 250000, 10, 2, 11, 4, 20, 100, 300, 8500
    ],
    "configuration_fingerprint": "af6cfa6df529484f",
    "operations_per_second": 4,
}
EXPECTED_C3_FIXED_PRICE = {
    "flash_bytes": 324703,
    "static_ram_bytes": 21276,
    "retained_heap_bytes": 77448,
    "stack_bytes": 0,
    "vendor_reserved_ram_bytes": 0,
    "worker_threads": 2,
    "interrupt_slots": 0,
    "dma_channels": 0,
    "controller_firmware_bytes": 0,
    "peripheral_channels": 0,
}
EXPECTED_C3_RUNTIME_PRICE = {
    "transient_heap_peak_bytes": 0,
    "stack_high_water_bytes": 3716,
    "cpu_cycles_per_second": 4156381,
    "latency_p99_cycles": 26824448,
    "latency_max_cycles": 35823264,
}
EXPECTED_C3_COEXISTENCE = {
    "leases": ["esp32c3-shared-radio"],
    "exclusive_resources": ["ble-controller"],
    "compatible_instances": ["wifi0"],
    "core_affinity": ["cpu0"],
}
EXPECTED_ESP32_WORKLOAD = {
    "namespace": "esp32-arduino-wifi-ble-gatt-http",
    "configuration_words": [
        240, 4, 20, 8, 250000, 10, 2, 11, 4, 20, 100, 300, 8500
    ],
    "configuration_fingerprint": "8ef5f7224c27afd9",
    "operations_per_second": 4,
}
EXPECTED_ESP32_FIXED_PRICE = {
    "flash_bytes": 1663227,
    "static_ram_bytes": 79072,
    "retained_heap_bytes": 153604,
    "stack_bytes": 43124,
    "vendor_reserved_ram_bytes": 0,
    "worker_threads": 8,
    "interrupt_slots": 0,
    "dma_channels": 0,
    "controller_firmware_bytes": 0,
    "peripheral_channels": 0,
}
EXPECTED_ESP32_RUNTIME_PRICE = {
    "transient_heap_peak_bytes": 18656,
    "stack_high_water_bytes": 17528,
    "cpu_cycles_per_second": 34200363,
    "latency_p99_cycles": 47247480,
    "latency_max_cycles": 68852952,
}
EXPECTED_ESP32_COEXISTENCE = {
    "leases": ["esp32-shared-radio"],
    "exclusive_resources": ["ble-controller"],
    "compatible_instances": ["wifi0"],
    "core_affinity": ["cpu0", "cpu1"],
}
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

DISABLED = r"""#define NOBRO_ESP_BLE_DISABLED 1
#include <NobroArduinoEspBLE.h>
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
#include <NobroArduinoEspBLE.h>

nobro::ArduinoEspBleStack ble;
nobro::NobroApp<2, 1> app;
volatile bool exerciseProvider = false;

void setup() {
  auto source = app.sensor("source", 10);
  auto sink = app.service("sink", 20);
  app.wire(source, sink);
  if (exerciseProvider) {
    const uint8_t payload[] = {'n', 'o', 'b', 'r', 'o'};
    nobro_ble_event_t event = {};
    bool available = false;
    ble.mount();
    const uint64_t now = micros();
    ble.advertise(payload, sizeof(payload), now, now + 1000000);
    ble.poll(event, available);
    ble.respondGatt(1, 1, payload, sizeof(payload));
    ble.disconnect();
    ble.stopAdvertising();
    ble.quiesce();
    ble.recover();
  }
  Serial.begin(115200);
  Serial.println(app.admit() ? "NOBRO:READY" : app.errorCode());
  Serial.println(ble.identity().backend_id);
  Serial.println(ble.vendorHost());
  Serial.println(ble.staticRamBytes());
  Serial.println(ble.vendorManagedHeap() ? "vendor-heap" : "no-vendor-heap");
  Serial.println(ble.vendorManagedTasks() ? "vendor-tasks" : "no-vendor-tasks");
}
void loop() {}
"""


def run(command: list[str]) -> str:
    completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if completed.returncode:
        raise RuntimeError((completed.stdout + completed.stderr).strip())
    return completed.stdout + completed.stderr


def record(records: list[dict], identifier: str, label: str) -> dict:
    matches = [item for item in records if item.get("id") == identifier]
    if len(matches) != 1:
        raise RuntimeError(f"{label} {identifier!r} is missing or duplicated")
    return matches[0]


def compile_sketch(
    cli: str,
    root: pathlib.Path,
    name: str,
    source: str,
    fqbn: str,
) -> tuple[int, int, pathlib.Path]:
    sketch = root / name
    sketch.mkdir()
    (sketch / f"{name}.ino").write_text(source, encoding="utf-8")
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


def map_text(build: pathlib.Path) -> str:
    maps = list(build.glob("*.map"))
    if len(maps) != 1:
        raise RuntimeError("target build map is missing or ambiguous")
    return maps[0].read_text(encoding="utf-8", errors="replace")


def verify_disabled_map(build: pathlib.Path) -> None:
    text = map_text(build)
    hits = [symbol for symbol in FORBIDDEN_DISABLED if symbol in text]
    if hits:
        raise RuntimeError(f"disabled Arduino-ESP32 BLE retained symbols: {hits}")


def verify_enabled_host(
    build: pathlib.Path,
    host: str,
    expected_symbol: str,
) -> None:
    text = map_text(build)
    other = "nimble_port_init" if host == "bluedroid" else "esp_bluedroid_init"
    if expected_symbol not in text or other in text:
        raise RuntimeError(
            f"{host} target does not retain exactly its board-package BLE host"
        )


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
    if record(catalog["provenance"], SOURCE_ID, "catalog provenance") != {
        **expected_source,
        "pinned": True,
        "clean": True,
    }:
        raise RuntimeError("Arduino-ESP32 catalog provenance is stale")

    backend = record(features["backends"], BACKEND_ID, "backend")
    if (
        backend.get("adapter_component_id") != COMPONENT_ID
        or backend.get("capability_kind") != "ble_link"
        or backend.get("stack_family") != "ble"
        or backend.get("maturity") != "implemented"
        or backend.get("evidence") != ["host-test", "physical", "target-build"]
        or backend.get("provenance_id") != SOURCE_ID
        or backend.get("supported_targets") != targets
        or not backend.get("limitations")
    ):
        raise RuntimeError("Arduino-ESP32 BLE backend boundary is stale")

    expected_scopes = []
    for platform, _, host, _ in TARGETS:
        binding = record(
            features["bindings"],
            f"binding-ble-arduino-esp-{platform}",
            "binding",
        )
        common_invalid = (
            binding.get("backend_id") != BACKEND_ID
            or binding.get("capability_kind") != "ble_link"
            or binding.get("platform") != platform
            or binding.get("composition") != "arduino"
            or binding.get("instance") != "ble0"
            or binding.get("evidence_gates") != [GATE_ID]
            or set(
                binding.get("disabled_symbol_gate", {}).get(
                    "forbidden_symbols", []
                )
            )
            != set(FORBIDDEN_DISABLED)
            or binding.get("report_wiring")
            != {
                "provider_id": "ble_link",
                "status_field": f"{platform}_ble0",
                "evidence_gate": GATE_ID,
            }
        )
        if platform == "esp32":
            fixed_provenance = {
                field: (
                    "measured"
                    if field
                    in {
                        "flash_bytes",
                        "static_ram_bytes",
                        "retained_heap_bytes",
                        "stack_bytes",
                        "worker_threads",
                    }
                    else "source-derived"
                )
                for field in EXPECTED_ESP32_FIXED_PRICE
            }
            priced_invalid = (
                binding.get("maturity") != "implemented"
                or binding.get("workload") != EXPECTED_ESP32_WORKLOAD
                or binding.get("measured_fixed_price")
                != EXPECTED_ESP32_FIXED_PRICE
                or binding.get("fixed_price_provenance")
                != fixed_provenance
                or binding.get("measured_runtime_price")
                != EXPECTED_ESP32_RUNTIME_PRICE
                or binding.get("runtime_price_provenance")
                != {
                    field: "measured"
                    for field in EXPECTED_ESP32_RUNTIME_PRICE
                }
                or binding.get("coexistence") != EXPECTED_ESP32_COEXISTENCE
                or not binding.get("price_basis")
                or "whole" not in binding["price_basis"].get("fixed", "")
            )
        elif platform == "esp32c3":
            provenance = {field: "measured" for field in EXPECTED_C3_RUNTIME_PRICE}
            fixed_provenance = {
                field: (
                    "measured"
                    if field
                    in {
                        "flash_bytes",
                        "static_ram_bytes",
                        "retained_heap_bytes",
                        "worker_threads",
                    }
                    else "source-derived"
                )
                for field in EXPECTED_C3_FIXED_PRICE
            }
            priced_invalid = (
                binding.get("maturity") != "implemented"
                or binding.get("workload") != EXPECTED_C3_WORKLOAD
                or binding.get("measured_fixed_price")
                != EXPECTED_C3_FIXED_PRICE
                or binding.get("fixed_price_provenance")
                != fixed_provenance
                or binding.get("measured_runtime_price")
                != EXPECTED_C3_RUNTIME_PRICE
                or binding.get("runtime_price_provenance") != provenance
                or binding.get("coexistence") != EXPECTED_C3_COEXISTENCE
                or not binding.get("price_basis")
            )
        else:
            priced_invalid = (
                binding.get("maturity") != "compile-only"
                or binding.get("price_state") != "unmeasured"
                or host not in " ".join(binding.get("limitations", [])).lower()
            )
        if common_invalid or priced_invalid:
            raise RuntimeError(f"{platform} BLE binding is stale or falsely priced")
        expected_scopes.append(
            {
                "platform": platform,
                "composition": "arduino",
                "capabilities": ["ble_link"],
            }
        )

    component = record(catalog["components"], COMPONENT_ID, "component")
    library = record(catalog["components"], LIBRARY_ID, "library")
    if (
        component.get("path") != "core/adapters/wireless/ble/arduino-esp"
        or component.get("maturity") != "implemented"
        or component.get("evidence")
        != ["host-test", "physical", "target-build"]
        or component.get("supported_targets") != targets
        or library.get("facade")
        != "packages/arduino/src/NobroArduinoEspBLE.h"
        or library.get("provenance_id") != SOURCE_ID
        or library.get("maturity") != "implemented"
        or library.get("evidence") != ["physical", "target-build"]
        or library.get("supported_targets") != targets
    ):
        raise RuntimeError("Arduino-ESP32 BLE catalog relationship is stale")

    gate = tiers.get("evidence_gates", {}).get(GATE_ID)
    if (
        not isinstance(gate, dict)
        or gate.get("command") != ["python", "tools/check_arduino_esp_ble.py"]
        or gate.get("runner") != "arduino-package"
        or gate.get("claim_scopes") != expected_scopes
    ):
        raise RuntimeError("Arduino-ESP32 BLE receipt gate is stale")
    for platform, _, host, _ in TARGETS:
        claim = (
            tiers.get("platforms", {})
            .get(platform, {})
            .get("compositions", {})
            .get("arduino", {})
            .get("claims", {})
            .get("ble_link")
        )
        if (
            not isinstance(claim, dict)
            or claim.get("maturity") != "implemented"
            or claim.get("evidence") != [GATE_ID]
            or host not in claim.get("limitations", "").lower()
        ):
            raise RuntimeError(f"{platform} BLE tier claim is stale")

    arduino_header = PACKAGE / "src" / "NobroArduinoEspBLE.h"
    platformio_header = (
        ROOT / "packages" / "platformio" / "include" / "NobroArduinoEspBLE.h"
    )
    header = arduino_header.read_text(encoding="utf-8")
    if header != platformio_header.read_text(encoding="utf-8"):
        raise RuntimeError("Arduino and PlatformIO ESP BLE facades drifted")
    for token in (
        "#if !defined(NOBRO_ESP_BLE_DISABLED)",
        "#if !defined(ARDUINO_ARCH_ESP32)",
        "CONFIG_BLUEDROID_ENABLED",
        "CONFIG_NIMBLE_ENABLED",
        "NOBRO_STACK_QUEUE_FULL",
        "#include <BLE2902.h>",
        "#if defined(CONFIG_BLUEDROID_ENABLED)",
        "descriptor_ = new BLE2902()",
        "characteristic_->addDescriptor(descriptor_)",
        "awaitDisconnected(1000000U)",
        "resetGattValue()",
        "resetVendorStack()",
        "nobro_ble_event_t pending_events_[4]",
        "event_count_ == eventCapacity()",
        "BLEDevice::deinit(false)",
        "BLEDevice::setCustomGapHandler(bluedroidGapEvent)",
        "ESP_GAP_BLE_ADV_START_COMPLETE_EVT",
        "advertising_config_failed_",
        "characteristic->getData()",
        "vendorManagedHeap() const { return true; }",
        "vendorManagedTasks() const { return true; }",
        "globalController() const { return true; }",
    ):
        if token not in header:
            raise RuntimeError(
                f"Arduino-ESP32 BLE facade lacks required boundary {token!r}"
            )


def main() -> int:
    cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
    if not cli:
        print("ARDUINO ESP BLE: FAIL (arduino-cli not found)")
        return 1
    try:
        verify_metadata()
        with tempfile.TemporaryDirectory(prefix="nobro-arduino-esp-ble-") as temp:
            root = pathlib.Path(temp)
            for index, (platform, fqbn, host, host_symbol) in enumerate(TARGETS):
                baseline = compile_sketch(
                    cli, root, f"baseline-{index}", BASELINE, fqbn
                )
                disabled = compile_sketch(
                    cli, root, f"disabled-{index}", DISABLED, fqbn
                )
                if baseline[:2] != disabled[:2]:
                    raise RuntimeError(
                        f"{platform} disabled facade is not zero-cost: "
                        f"baseline={baseline[:2]} disabled={disabled[:2]}"
                    )
                verify_disabled_map(disabled[2])
                enabled = compile_sketch(
                    cli, root, f"enabled-{index}", FEATURE, fqbn
                )
                if enabled[0] <= baseline[0] or enabled[1] < baseline[1]:
                    raise RuntimeError(
                        f"{platform} enabled BLE cost is not observable: "
                        f"baseline={baseline[:2]} enabled={enabled[:2]}"
                    )
                verify_enabled_host(enabled[2], host, host_symbol)
                print(
                    f"  PASS {fqbn} host={host} zero-disabled "
                    f"flash={baseline[0]} ram={baseline[1]}; "
                    f"enabled={enabled[:2]}"
                )
    except (OSError, RuntimeError, ValueError, KeyError) as error:
        print(f"ARDUINO ESP BLE: FAIL ({error})")
        return 1
    print(
        "ARDUINO ESP BLE: PASS "
        "(pinned 3.3.10 Bluedroid/NimBLE target matrix; zero-disabled)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
