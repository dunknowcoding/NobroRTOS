#!/usr/bin/env python3
"""Compile and execute bounded ESP32 ADC-DMA, LEDC, and RMT facade behavior."""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
FACADE = ROOT / "packages" / "arduino" / "src"

TEST = r'''
#include <assert.h>
#include <stddef.h>
#include <stdint.h>

static uint32_t now_us = 0;
static uint32_t step_us = 0;
uint32_t micros() { const uint32_t value = now_us; now_us += step_us; return value; }

typedef struct {
  uint8_t pin;
  uint8_t channel;
  int avg_read_raw;
  int avg_read_mvolts;
} adc_continuous_result_t;
static adc_continuous_result_t adc_results[2] = {{1, 0, 100, 500}, {2, 1, 200, 1000}};
static bool adc_ok = true;
static bool adc_configured = false;
static bool adc_started = false;
static unsigned adc_starts = 0;
void analogContinuousSetWidth(uint8_t) {}
bool analogContinuous(const uint8_t *, size_t, uint32_t, uint32_t, void (*)(void)) {
  if (!adc_ok || adc_configured) return false;
  adc_configured = true;
  return true;
}
bool analogContinuousRead(adc_continuous_result_t **out, uint32_t) { *out = adc_results; return adc_ok; }
bool analogContinuousStart() {
  if (!adc_ok || !adc_configured || adc_started) return false;
  ++adc_starts;
  adc_started = true;
  return true;
}
bool analogContinuousStop() {
  if (!adc_ok || !adc_started) return false;
  adc_started = false;
  return true;
}
bool analogContinuousDeinit() {
  if (!adc_ok || !adc_configured || adc_started) return false;
  adc_configured = false;
  return true;
}

static bool ledc_ok = true;
static uint32_t ledc_duty = 0;
bool ledcAttach(uint8_t, uint32_t, uint8_t) { return ledc_ok; }
bool ledcWrite(uint8_t, uint32_t duty) { ledc_duty = duty; return ledc_ok; }
bool ledcDetach(uint8_t) { return ledc_ok; }

typedef enum { RMT_RX_MODE = 0, RMT_TX_MODE = 1 } rmt_ch_dir_t;
typedef enum { RMT_MEM_NUM_BLOCKS_1 = 1 } rmt_reserve_memsize_t;
typedef union {
  struct { uint32_t duration0:15; uint32_t level0:1; uint32_t duration1:15; uint32_t level1:1; };
  uint32_t val;
} rmt_data_t;
static bool rmt_ok = true;
static bool rmt_attached = false;
static size_t rmt_count = 0;
bool rmtInit(int, rmt_ch_dir_t, rmt_reserve_memsize_t, uint32_t) {
  if (!rmt_ok || rmt_attached) return false;
  rmt_attached = true;
  return true;
}
bool rmtWrite(int, rmt_data_t *, size_t count, uint32_t) { rmt_count = count; return rmt_ok; }
bool rmtDeinit(int) {
  if (!rmt_ok || !rmt_attached) return false;
  rmt_attached = false;
  return true;
}

#define NOBRO_ESP32_PERIPHERALS_TEST 1
#include <NobroEsp32Peripherals.h>

int main() {
  nobro::Esp32ContinuousAdc<2> adc;
  const uint8_t pins[2] = {1, 2};
  const nobro_adc_dma_config_t adc_config = {2, 12, 16, 20000};
  assert(adc.configure(pins, adc_config) == NOBRO_ADC_DMA_OK);
  assert(adc.start() == NOBRO_ADC_DMA_OK);
  nobro_adc_sample_t one[1] = {};
  size_t count = 0;
  assert(adc.readFrame(one, 1, 100, count) == NOBRO_ADC_DMA_OUTPUT_TOO_SMALL);
  nobro_adc_sample_t frame[2] = {};
  assert(adc.readFrame(frame, 2, 100, count) == NOBRO_ADC_DMA_OK);
  assert(count == 2 && frame[1].raw == 200 && frame[1].millivolts == 1000);
  step_us = 10;
  assert(adc.readFrame(frame, 2, 1, count) == NOBRO_ADC_DMA_DEADLINE_MISS);
  step_us = 0;
  assert(adc.quiesce() == NOBRO_ADC_DMA_OK);
  assert(adc.recover() == NOBRO_ADC_DMA_OK);
  assert(adc.diagnostics().recoveries == 1);
  assert(adc.quiesce() == NOBRO_ADC_DMA_OK);

  nobro::Esp32LedcPwm ledc(3);
  const nobro_pwm_config_t pwm = {20000, 10};
  assert(ledc.configure(pwm) == NOBRO_PULSE_OK);
  assert(ledc.setDuty(1023) == NOBRO_PULSE_OK && ledc_duty == 1023);
  assert(ledc.setDuty(1024) == NOBRO_PULSE_INVALID_CONFIG);
  assert(ledc.quiesce() == NOBRO_PULSE_OK);
  assert(ledc.recover() == NOBRO_PULSE_OK);

  nobro::Esp32RmtPulse<2> rmt(4);
  assert(rmt.configure(1000000) == NOBRO_PULSE_OK);
  const nobro_pulse_symbol_t symbols[2] = {{4, 6}, {8, 2}};
  assert(rmt.transmit(symbols, 2, 100) == NOBRO_PULSE_OK && rmt_count == 2);
  assert(rmt.transmit(symbols, 3, 100) == NOBRO_PULSE_TOO_MANY_SYMBOLS);
  assert(rmt.quiesce() == NOBRO_PULSE_OK && !rmt_attached);
  assert(rmt.recover() == NOBRO_PULSE_OK && rmt_attached);
  rmt_ok = false;
  assert(rmt.transmit(symbols, 2, 100) == NOBRO_PULSE_DEADLINE_MISS);
  rmt_ok = true;
  assert(rmt.recover() == NOBRO_PULSE_OK);
  assert(rmt.diagnostics().recoveries == 2);
  return 0;
}
'''


def main() -> int:
    cxx = next(
        (path for name in ("g++", "clang++", "c++") if (path := shutil.which(name))),
        None,
    )
    if not cxx:
        print("ESP32 PERIPHERAL FACADE: FAIL (no C++ compiler)")
        return 1
    with tempfile.TemporaryDirectory(prefix="nobro-esp32-peripherals-") as temp:
        root = pathlib.Path(temp)
        source = root / "test.cpp"
        source.write_text(TEST, encoding="utf-8")
        binary = root / ("test.exe" if sys.platform == "win32" else "test")
        built = subprocess.run(
            [
                cxx,
                "-std=c++11",
                "-Wall",
                "-Wextra",
                "-Werror",
                f"-I{FACADE}",
                str(source),
                "-o",
                str(binary),
            ],
            capture_output=True,
            text=True,
        )
        if built.returncode:
            print("ESP32 PERIPHERAL FACADE: FAIL (compile)")
            print((built.stdout + built.stderr).strip())
            return 1
        ran = subprocess.run([str(binary)], capture_output=True, text=True)
        if ran.returncode:
            print(f"ESP32 PERIPHERAL FACADE: FAIL (runtime {ran.returncode})")
            print((ran.stdout + ran.stderr).strip())
            return 1
    print("ESP32 PERIPHERAL FACADE: PASS (ADC-DMA, LEDC, RMT bounds/lifecycle)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
