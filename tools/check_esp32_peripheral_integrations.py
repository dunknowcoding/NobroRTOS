#!/usr/bin/env python3
"""Verify ESP32 continuous-ADC, LEDC, and RMT target builds and zero-disabled cost."""

from __future__ import annotations

import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "packages" / "arduino"
FQBNS = (
    "esp32:esp32:esp32",
    "esp32:esp32:esp32c3",
    "esp32:esp32:esp32s3",
    "esp32:esp32:esp32p4",
)
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

DISABLED = r'''#define NOBRO_ESP32_PERIPHERALS_DISABLED 1
#include <NobroEsp32Peripherals.h>
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
#include <NobroEsp32Peripherals.h>

nobro::Esp32ContinuousAdc<1> adc;
nobro::Esp32PersistentContinuousAdc<1, 32> persistentAdc;
nobro::Esp32LedcPwm pwm(4);
nobro::Esp32RmtPulse<2> pulse(5);
nobro::NobroApp<2, 1> app;
volatile bool exerciseProviders = false;

void setup() {
  Serial.begin(115200);
  auto source = app.sensor("source", 10);
  auto sink = app.service("sink", 20);
  app.wire(source, sink);
  if (exerciseProviders) {
    const uint8_t pins[1] = {1};
    const uint16_t adcConversions =
        decltype(adc)::alignedConversionsPerChannel(1, 16);
    const nobro_adc_dma_config_t adcConfig = {
        1, 12, adcConversions, 20000};
    nobro_adc_sample_t samples[1] = {};
    size_t count = 0;
    adc.configure(pins, adcConfig);
    adc.start();
    adc.readFrame(samples, 1, 1000, count);
    adc.quiesce();
    adc.recover();
    adc.release();

    const uint16_t persistentConversions =
        decltype(persistentAdc)::alignedConversionsPerChannel(1, 16);
    const nobro_adc_dma_config_t persistentConfig = {
        1, 12, persistentConversions, 20000};
    persistentAdc.configure(pins, persistentConfig);
    persistentAdc.start();
    persistentAdc.readFrame(samples, 1, 1000, count);
    persistentAdc.quiesce();
    persistentAdc.recover();
    persistentAdc.release();

    const nobro_pwm_config_t pwmConfig = {20000, 10};
    pwm.configure(pwmConfig);
    pwm.setDuty(512);
    pwm.quiesce();
    pwm.recover();
    pwm.release();

    const nobro_pulse_symbol_t symbols[2] = {{4, 6}, {8, 2}};
    pulse.configure(1000000);
    pulse.transmit(symbols, 2, 1000);
    pulse.quiesce();
    pulse.recover();
    pulse.release();
  }
  Serial.println(app.admit() ? "NOBRO:READY" : app.errorCode());
  Serial.println(adc.staticRamBytes() + persistentAdc.staticRamBytes() +
                 pwm.staticRamBytes() + pulse.staticRamBytes());
}
void loop() {}
'''

ADC_FEATURE = r'''#include <NobroRTOS.h>
#include <NobroEsp32Peripherals.h>
nobro::Esp32ContinuousAdc<1> adc;
nobro::NobroApp<2, 1> app;
volatile bool exerciseProvider = false;
void setup() {
  auto source = app.sensor("source", 10);
  auto sink = app.service("sink", 20);
  app.wire(source, sink);
  if (exerciseProvider) {
    const uint8_t pins[1] = {1};
    const uint16_t conversions =
        decltype(adc)::alignedConversionsPerChannel(1, 16);
    const nobro_adc_dma_config_t config = {
        1, 12, conversions, 20000};
    nobro_adc_sample_t samples[1] = {};
    size_t count = 0;
    adc.configure(pins, config);
    adc.start();
    adc.readFrame(samples, 1, 1000, count);
    adc.quiesce();
    adc.recover();
    adc.release();
  }
  Serial.begin(115200);
  Serial.println(app.admit() ? "NOBRO:READY" : app.errorCode());
  Serial.println(adc.staticRamBytes());
}
void loop() {}
'''

PERSISTENT_ADC_FEATURE = r'''#include <NobroRTOS.h>
#include <NobroEsp32Peripherals.h>
nobro::Esp32PersistentContinuousAdc<1, 32> adc;
nobro::NobroApp<2, 1> app;
volatile bool exerciseProvider = false;
void setup() {
  auto source = app.sensor("source", 10);
  auto sink = app.service("sink", 20);
  app.wire(source, sink);
  if (exerciseProvider) {
    const uint8_t pins[1] = {1};
    const uint16_t conversions =
        decltype(adc)::alignedConversionsPerChannel(1, 16);
    const nobro_adc_dma_config_t config = {
        1, 12, conversions, 20000};
    nobro_adc_sample_t samples[1] = {};
    size_t count = 0;
    adc.configure(pins, config);
    adc.start();
    adc.readFrame(samples, 1, 1000, count);
    adc.quiesce();
    adc.recover();
    adc.release();
  }
  Serial.begin(115200);
  Serial.println(app.admit() ? "NOBRO:READY" : app.errorCode());
  Serial.println(adc.staticRamBytes() + adc.persistentBufferBytes());
}
void loop() {}
'''

LEDC_FEATURE = r'''#include <NobroRTOS.h>
#include <NobroEsp32Peripherals.h>
nobro::Esp32LedcPwm pwm(4);
nobro::NobroApp<2, 1> app;
volatile bool exerciseProvider = false;
void setup() {
  auto source = app.sensor("source", 10);
  auto sink = app.service("sink", 20);
  app.wire(source, sink);
  if (exerciseProvider) {
    const nobro_pwm_config_t config = {20000, 10};
    pwm.configure(config);
    pwm.setDuty(512);
    pwm.quiesce();
    pwm.recover();
    pwm.release();
  }
  Serial.begin(115200);
  Serial.println(app.admit() ? "NOBRO:READY" : app.errorCode());
  Serial.println(pwm.staticRamBytes());
}
void loop() {}
'''

RMT_FEATURE = r'''#include <NobroRTOS.h>
#include <NobroEsp32Peripherals.h>
nobro::Esp32RmtPulse<2> pulse(5);
nobro::NobroApp<2, 1> app;
volatile bool exerciseProvider = false;
void setup() {
  auto source = app.sensor("source", 10);
  auto sink = app.service("sink", 20);
  app.wire(source, sink);
  if (exerciseProvider) {
    const nobro_pulse_symbol_t symbols[2] = {{4, 6}, {8, 2}};
    pulse.configure(1000000);
    pulse.transmit(symbols, 2, 1000);
    pulse.quiesce();
    pulse.recover();
    pulse.release();
  }
  Serial.begin(115200);
  Serial.println(app.admit() ? "NOBRO:READY" : app.errorCode());
  Serial.println(pulse.staticRamBytes());
}
void loop() {}
'''

FORBIDDEN_DISABLED = (
    "analogContinuous",
    "adc_continuous_read",
    "ledcAttach",
    "rmtInit",
    "Esp32ContinuousAdc",
    "Esp32PersistentContinuousAdc",
    "Esp32LedcPwm",
    "Esp32RmtPulse",
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
    fqbn: str,
    root: pathlib.Path,
    name: str,
    source: str,
) -> tuple[int, int, pathlib.Path]:
    safe_name = name.replace(":", "-")
    sketch = write_sketch(root, safe_name, source)
    build = root / f"{safe_name}-build"
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
        raise RuntimeError(f"{fqbn}/{name}: Arduino size summary missing")
    return int(match["flash"]), int(match["ram"]), build


def verify_disabled_map(build: pathlib.Path) -> None:
    maps = list(build.glob("*.map"))
    if len(maps) != 1:
        raise RuntimeError("disabled build map is missing or ambiguous")
    text = maps[0].read_text(encoding="utf-8", errors="replace")
    hits = [symbol for symbol in FORBIDDEN_DISABLED if symbol in text]
    if hits:
        raise RuntimeError(f"disabled peripheral providers retained symbols: {hits}")


def main() -> int:
    cli = shutil.which("arduino-cli") or shutil.which("arduino-cli.exe")
    if not cli:
        print("ESP32 PERIPHERAL INTEGRATIONS: FAIL (arduino-cli not found)")
        return 1
    try:
        with tempfile.TemporaryDirectory(prefix="nobro-esp32-provider-build-") as temp:
            root = pathlib.Path(temp)
            reference_fqbn = "esp32:esp32:esp32s3"
            baseline = compile_sketch(cli, reference_fqbn, root, "baseline", BASELINE)
            disabled = compile_sketch(cli, reference_fqbn, root, "disabled", DISABLED)
            if baseline[:2] != disabled[:2]:
                raise RuntimeError(
                    "disabled providers are not zero-cost: "
                    f"baseline={baseline[:2]} disabled={disabled[:2]}"
                )
            verify_disabled_map(disabled[2])
            individual: dict[str, tuple[int, int]] = {}
            for index, (provider, source) in enumerate(
                (
                    ("adc_dma", ADC_FEATURE),
                    ("adc_dma_persistent", PERSISTENT_ADC_FEATURE),
                    ("pwm_ledc", LEDC_FEATURE),
                    ("pulse_rmt", RMT_FEATURE),
                )
            ):
                price = compile_sketch(
                    cli,
                    reference_fqbn,
                    root,
                    f"individual-{index}",
                    source,
                )[:2]
                if price[0] <= baseline[0] or price[1] < baseline[1]:
                    raise RuntimeError(
                        f"{provider}: enabled price is not observable: "
                        f"baseline={baseline[:2]} feature={price}"
                    )
                individual[provider] = (
                    price[0] - baseline[0],
                    price[1] - baseline[1],
                )
            prices: dict[str, tuple[int, int]] = {}
            for index, fqbn in enumerate(FQBNS):
                feature = compile_sketch(
                    cli, fqbn, root, f"feature-{index}", FEATURE
                )
                prices[fqbn] = feature[:2]
            enabled = prices[reference_fqbn]
            print(
                "  PASS zero-disabled "
                f"flash={baseline[0]} ram={baseline[1]}; "
                f"ESP32-S3 enabled-delta flash={enabled[0] - baseline[0]} "
                f"ram={enabled[1] - baseline[1]}"
            )
            for provider, delta in individual.items():
                print(
                    f"  PASS ESP32-S3 {provider} delta "
                    f"flash={delta[0]} ram={delta[1]}"
                )
            for fqbn, price in prices.items():
                print(f"  PASS target-build {fqbn} flash={price[0]} ram={price[1]}")
    except (OSError, RuntimeError) as error:
        print(f"ESP32 PERIPHERAL INTEGRATIONS: FAIL ({error})")
        return 1
    print("ESP32 PERIPHERAL INTEGRATIONS: PASS (ADC-DMA, LEDC, RMT)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
