#!/usr/bin/env python3
"""Verify the pinned NiusAudio member and ESP32-S3 Nobro facade build."""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

import check_board_features

ROOT = pathlib.Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "packages" / "arduino"
FEATURES = ROOT / "core" / "boards" / "feature_providers.json"
PIN = "fa2d3913c790b2856f95bce51e5fbd77b8c5c2b2"
VERSION = "0.3.1"
FQBN = "esp32:esp32:esp32s3"
BINDING_ID = "binding-audio-esp32s3-es8311"
EVIDENCE_GATE = "esp32s3-arduino-audio-target-build"
WORKLOAD_NAMESPACE = "esp32s3-es8311-arduino"
WORKLOAD_WORDS = [16_000, 1, 0, 192, 0]
EXPECTED_FIXED_PRICE = {
    "flash_bytes": 87_589,
    "static_ram_bytes": 2_508,
    "retained_heap_bytes": 16_164,
    "stack_bytes": 0,
    "vendor_reserved_ram_bytes": 0,
    "worker_threads": 0,
    "interrupt_slots": 2,
    "dma_channels": 2,
    "controller_firmware_bytes": 0,
    "peripheral_channels": 2,
}
EXPECTED_RUNTIME_PRICE = {
    "transient_heap_peak_bytes": 0,
    "stack_high_water_bytes": 2_608,
    "cpu_cycles_per_second": 71_918_002,
    "latency_p99_cycles": 3_583_845,
    "latency_max_cycles": 3_592_082,
}
EXPECTED_FIXED_PROVENANCE = {
    "flash_bytes": "measured",
    "static_ram_bytes": "measured",
    "retained_heap_bytes": "measured",
    "stack_bytes": "source-derived",
    "vendor_reserved_ram_bytes": "source-derived",
    "worker_threads": "measured",
    "interrupt_slots": "source-derived",
    "dma_channels": "source-derived",
    "controller_firmware_bytes": "source-derived",
    "peripheral_channels": "source-derived",
}
EXPECTED_RUNTIME_PROVENANCE = {
    "transient_heap_peak_bytes": "source-derived",
    "stack_high_water_bytes": "measured",
    "cpu_cycles_per_second": "measured",
    "latency_p99_cycles": "measured",
    "latency_max_cycles": "measured",
}
EXPECTED_PRICE_BASIS = {
    "toolchain": "Arduino-ESP32 3.3.10 with ESP-IDF 5.5.4 and NiusAudio 0.3.1",
    "fixed": (
        "same-target isolated link delta, active retained-heap delta after repeated "
        "recovery, and pinned source ownership"
    ),
    "runtime": (
        "conservative maximum from three physical runs at 100 transfers per second"
    ),
}
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

DISABLED = r'''#define NOBRO_AUDIO_DISABLED 1
#include <NobroNiusAudio.h>
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

FEATURE = r'''#include <NiusAudio.h>
#include <NobroRTOS.h>
#include <NobroNiusAudio.h>

NiusAudioWeActEs8311Board codec;
nobro::NiusEs8311AudioAdapter<2, 96> audio(codec);
volatile bool exerciseAudio = false;

void setup() {
  Serial.begin(115200);
  const nobro_audio_format_t format = {16000, 96, 1, 16};
  int16_t frame[96] = {};
  if (exerciseAudio) {
    audio.configure(1, 2, 3, 4, 5, 6, 7, 8, format);
    audio.begin();
    audio.submit(frame, 96);
    audio.pump(10000);
    audio.capture(frame, 96, 10000);
    audio.quiesce();
    audio.recover();
  }
  Serial.println(audio.staticRamBytes());
}
void loop() {}
'''

FORBIDDEN_DISABLED = (
    "NiusAudio",
    "NiusEs8311AudioAdapter",
    "i2s_new_channel",
    "NiusAudioEs8311Codec",
)


def run(command: list[str], cwd: pathlib.Path = ROOT) -> str:
    completed = subprocess.run(command, cwd=cwd, capture_output=True, text=True)
    if completed.returncode:
        raise RuntimeError((completed.stdout + completed.stderr).strip())
    return completed.stdout + completed.stderr


def verify_binding(registry: dict) -> dict:
    binding = next(
        (
            item
            for item in registry.get("bindings", [])
            if item.get("id") == BINDING_ID
        ),
        None,
    )
    if binding is None:
        raise RuntimeError("exact ESP32-S3 audio binding is missing")
    expected_workload = {
        "namespace": WORKLOAD_NAMESPACE,
        "configuration_words": WORKLOAD_WORDS,
        "configuration_fingerprint": check_board_features.workload_fingerprint(
            WORKLOAD_NAMESPACE, WORKLOAD_WORDS
        ),
        "operations_per_second": 100,
    }
    if binding.get("workload") != expected_workload:
        raise RuntimeError("audio binding workload identity is stale")
    if binding.get("measured_fixed_price") != EXPECTED_FIXED_PRICE:
        raise RuntimeError("audio binding fixed price is stale")
    if binding.get("measured_runtime_price") != EXPECTED_RUNTIME_PRICE:
        raise RuntimeError("audio binding runtime price is stale")
    if binding.get("fixed_price_provenance") != EXPECTED_FIXED_PROVENANCE:
        raise RuntimeError("audio binding fixed-price provenance is stale")
    if binding.get("runtime_price_provenance") != EXPECTED_RUNTIME_PROVENANCE:
        raise RuntimeError("audio binding runtime-price provenance is stale")
    if binding.get("price_basis") != EXPECTED_PRICE_BASIS:
        raise RuntimeError("audio binding price basis is stale")
    if binding.get("evidence_gates") != [EVIDENCE_GATE]:
        raise RuntimeError("audio binding evidence gate is stale")
    if binding.get("report_wiring") != {
        "provider_id": "audio_i2s",
        "status_field": "esp32s3_audio0",
        "evidence_gate": EVIDENCE_GATE,
    }:
        raise RuntimeError("audio binding report wiring is stale")
    if set(binding.get("disabled_symbol_gate", {}).get("forbidden_symbols", [])) != set(
        FORBIDDEN_DISABLED
    ):
        raise RuntimeError("audio binding disabled-symbol proof is stale")
    return binding


def verify_checkout(library: pathlib.Path) -> pathlib.Path:
    library = library.resolve(strict=True)
    properties = (library / "library.properties").read_text(encoding="utf-8")
    if "name=NiusAudio" not in properties or f"version={VERSION}" not in properties:
        raise RuntimeError(f"NiusAudio must be exactly version {VERSION}")
    if run(["git", "rev-parse", "HEAD"], library).strip() != PIN:
        raise RuntimeError(f"NiusAudio checkout is not pinned to {PIN}")
    if run(["git", "status", "--porcelain"], library).strip():
        raise RuntimeError("NiusAudio checkout has local modifications")
    license_text = (library / "LICENSE").read_text(encoding="utf-8")
    if "Apache License" not in license_text or "Version 2.0" not in license_text:
        raise RuntimeError("NiusAudio Apache-2.0 license is missing")
    registry = json.loads(FEATURES.read_text(encoding="utf-8"))
    provenance = next(
        (
            item
            for item in registry["provenance"]
            if item["id"] == "source-niusaudio"
        ),
        None,
    )
    if provenance != {
        "id": "source-niusaudio",
        "source": "https://github.com/dunknowcoding/NiusAudio",
        "revision": PIN,
        "version": VERSION,
        "license": "Apache-2.0",
    }:
        raise RuntimeError("board-feature NiusAudio provenance is stale")
    backend = next(
        (
            item
            for item in registry.get("backends", [])
            if item.get("id") == "backend-audio-esp32s3-es8311"
        ),
        None,
    )
    if not isinstance(backend, dict) or "physical" not in backend.get("evidence", []):
        raise RuntimeError("audio backend physical evidence state is stale")
    verify_binding(registry)
    required = (
        "src/modules/weact_es8311_ns4150b/NiusAudioWeActEs8311.h",
        "src/platform/generic/NiusAudioGenericBackend.cpp",
        "src/modules/generic_i2s_input/NiusAudioGenericI2sInput.h",
        "src/modules/generic_i2s_output/NiusAudioGenericI2sOutput.h",
    )
    for relative in required:
        if not (library / relative).is_file():
            raise RuntimeError(f"NiusAudio surface missing {relative}")
    return library


def write_sketch(root: pathlib.Path, name: str, source: str) -> pathlib.Path:
    sketch = root / name
    sketch.mkdir()
    (sketch / f"{name}.ino").write_text(source, encoding="utf-8")
    return sketch


def compile_sketch(
    cli: str,
    library: pathlib.Path,
    root: pathlib.Path,
    name: str,
    source: str,
    include_member: bool,
) -> tuple[int, int, pathlib.Path]:
    sketch = write_sketch(root, name, source)
    build = root / f"{name}-build"
    command = [
        cli,
        "compile",
        "--fqbn",
        FQBN,
        "--library",
        str(PACKAGE),
    ]
    if include_member:
        command.extend(["--library", str(library)])
    command.extend(["--build-path", str(build), str(sketch)])
    output = run(command)
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
        raise RuntimeError(f"disabled audio stack retained symbols: {hits}")


def compile_matrix(library: pathlib.Path) -> None:
    cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
    if not cli:
        raise RuntimeError("arduino-cli not found")
    with tempfile.TemporaryDirectory(prefix="nobro-audio-") as temp:
        root = pathlib.Path(temp)
        baseline = compile_sketch(cli, library, root, "baseline", BASELINE, False)
        disabled = compile_sketch(cli, library, root, "disabled", DISABLED, False)
        feature = compile_sketch(cli, library, root, "feature", FEATURE, True)
        if baseline[:2] != disabled[:2]:
            raise RuntimeError(
                "disabled audio stack is not zero-cost: "
                f"baseline={baseline[:2]} disabled={disabled[:2]}"
            )
        verify_disabled_map(disabled[2])
        if feature[0] <= baseline[0] or feature[1] <= baseline[1]:
            raise RuntimeError(
                f"feature price is not observable: baseline={baseline[:2]} "
                f"feature={feature[:2]}"
            )
        delta = (feature[0] - baseline[0], feature[1] - baseline[1])
        binding = verify_binding(json.loads(FEATURES.read_text(encoding="utf-8")))
        fixed = binding["measured_fixed_price"]
        registry_delta = (fixed["flash_bytes"], fixed["static_ram_bytes"])
        if delta != registry_delta:
            raise RuntimeError(
                "audio isolated build price differs: "
                f"registry={registry_delta} build={delta}"
            )
        print(
            "  PASS zero-disabled "
            f"flash={baseline[0]} ram={baseline[1]}; "
            f"enabled-delta flash={delta[0]} ram={delta[1]}; "
            "report esp32s3_audio0=target-build"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--library", type=pathlib.Path, required=True)
    parser.add_argument("--compile", action="store_true")
    args = parser.parse_args()
    try:
        library = verify_checkout(args.library)
        if args.compile:
            compile_matrix(library)
    except (OSError, RuntimeError) as error:
        print(f"AUDIO INTEGRATIONS: FAIL ({error})")
        return 1
    print(
        "AUDIO INTEGRATIONS: PASS "
        f"(NiusAudio {VERSION}; ESP32-S3 ES8311 facade; pinned {PIN[:12]})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
