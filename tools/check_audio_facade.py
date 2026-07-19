#!/usr/bin/env python3
"""Compile and execute the bounded NiusAudio Arduino facade with a fake transport."""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
FACADE = ROOT / "packages" / "arduino" / "src"

FAKE_NIUS = r'''#ifndef NIUS_AUDIO_H
#define NIUS_AUDIO_H
#include <stddef.h>
#include <stdint.h>
uint32_t micros();
class NiusAudioWeActEs8311Board {
public:
  bool control_ok = true;
  bool audio_ok = true;
  bool partial_write = false;
  bool partial_read = false;
  unsigned ends = 0;
  void setControlPins(int8_t, int8_t, int8_t, int8_t) {}
  void setAudioPins(int8_t, int8_t, int8_t, int8_t) {}
  void setFormat(uint32_t, uint8_t, uint8_t) {}
  bool begin() { return control_ok; }
  bool beginAudio() { return audio_ok; }
  void endAudio() { ++ends; }
  size_t writeSamples(const int16_t *, size_t count) {
    return partial_write && count ? count - 1 : count;
  }
  size_t readSamples(int16_t *samples, size_t count) {
    const size_t actual = partial_read && count ? count - 1 : count;
    for (size_t i = 0; i < actual; ++i) samples[i] = static_cast<int16_t>(i + 1);
    return actual;
  }
  const char *lastMessage() const { return "fake"; }
};
#endif
'''

TEST = r'''#include <assert.h>
#include <stdint.h>
static uint32_t now_us = 0;
static uint32_t step_us = 0;
uint32_t micros() { const uint32_t value = now_us; now_us += step_us; return value; }

#include <NobroNiusAudio.h>

int main() {
  NiusAudioWeActEs8311Board codec;
  nobro::NiusEs8311AudioAdapter<2, 4> audio(codec);
  assert(audio.begin() == NOBRO_AUDIO_INVALID_CONFIG);
  const nobro_audio_format_t format = {16000, 4, 1, 16};
  assert(audio.configure(1, 2, 3, 4, 5, 6, 7, 8, format) == NOBRO_AUDIO_OK);
  assert(audio.begin() == NOBRO_AUDIO_OK);
  assert(audio.begin() == NOBRO_AUDIO_INVALID_CONFIG);
  const int16_t frame[5] = {1, 2, 3, 4, 5};
  assert(audio.submit(frame, 4) == NOBRO_AUDIO_OK);
  assert(audio.submit(frame, 4) == NOBRO_AUDIO_OK);
  assert(audio.submit(frame, 4) == NOBRO_AUDIO_BACKPRESSURED);
  assert(audio.submit(frame, 5) == NOBRO_AUDIO_FRAME_TOO_LARGE);
  step_us = 5;
  assert(audio.pump(10) == NOBRO_AUDIO_OK);
  assert(audio.pump(1) == NOBRO_AUDIO_DEADLINE_MISS);
  int16_t captured[4] = {};
  assert(audio.capture(captured, 4, 1) == NOBRO_AUDIO_DEADLINE_MISS);
  assert(captured[0] == 1 && captured[3] == 4);
  audio.quiesce();
  assert(audio.state() == NOBRO_AUDIO_SUSPENDED);
  assert(audio.recover() == NOBRO_AUDIO_OK);
  assert(audio.state() == NOBRO_AUDIO_READY);
  assert(audio.submit(frame, 4) == NOBRO_AUDIO_OK);
  codec.partial_write = true;
  assert(audio.pump(10) == NOBRO_AUDIO_PARTIAL_IO);
  assert(audio.state() == NOBRO_AUDIO_FAULTED);
  const nobro_audio_diagnostics_t diagnostics = audio.diagnostics();
  assert(diagnostics.playback_frames == 2);
  assert(diagnostics.capture_frames == 1);
  assert(diagnostics.backpressure_rejections == 1);
  assert(diagnostics.oversized_rejections == 1);
  assert(diagnostics.partial_transfers == 1);
  assert(diagnostics.transport_errors == 1);
  assert(diagnostics.deadline_misses == 2);
  assert(diagnostics.recoveries == 1);

  NiusAudioWeActEs8311Board capture_codec;
  nobro::NiusEs8311AudioAdapter<1, 4> capture_audio(capture_codec);
  assert(capture_audio.configure(1, 2, 3, 4, 5, 6, 7, 8, format) == NOBRO_AUDIO_OK);
  assert(capture_audio.begin() == NOBRO_AUDIO_OK);
  capture_codec.partial_read = true;
  assert(capture_audio.capture(captured, 4, 10) == NOBRO_AUDIO_PARTIAL_IO);
  assert(capture_audio.state() == NOBRO_AUDIO_FAULTED);

  NiusAudioWeActEs8311Board zero_codec;
  nobro::NiusEs8311AudioAdapter<0, 4> zero(zero_codec);
  zero.quiesce();
  assert(zero.state() == NOBRO_AUDIO_DOWN);
  assert(zero.configure(1, 2, 3, 4, 5, 6, 7, 8, format) == NOBRO_AUDIO_OK);
  assert(zero.begin() == NOBRO_AUDIO_OK);
  assert(zero.submit(frame, 4) == NOBRO_AUDIO_BACKPRESSURED);
  return 0;
}
'''


def compiler() -> str | None:
    for name in ("g++", "clang++", "c++"):
        found = shutil.which(name)
        if found:
            return found
    return None


def main() -> int:
    cxx = compiler()
    if not cxx:
        print("AUDIO FACADE: FAIL (no C++ compiler)")
        return 1
    with tempfile.TemporaryDirectory(prefix="nobro-audio-facade-") as temp:
        root = pathlib.Path(temp)
        (root / "NiusAudio.h").write_text(FAKE_NIUS, encoding="utf-8")
        source = root / "test.cpp"
        source.write_text(TEST, encoding="utf-8")
        binary = root / ("test.exe" if sys.platform == "win32" else "test")
        command = [
            cxx,
            "-std=c++11",
            "-Wall",
            "-Wextra",
            "-Werror",
            f"-I{root}",
            f"-I{FACADE}",
            str(source),
            "-o",
            str(binary),
        ]
        built = subprocess.run(command, capture_output=True, text=True)
        if built.returncode:
            print("AUDIO FACADE: FAIL (compile)")
            print((built.stdout + built.stderr).strip())
            return 1
        ran = subprocess.run([str(binary)], capture_output=True, text=True)
        if ran.returncode:
            print(f"AUDIO FACADE: FAIL (runtime {ran.returncode})")
            print((ran.stdout + ran.stderr).strip())
            return 1
    print("AUDIO FACADE: PASS (bounds, backpressure, deadlines, lifecycle, recovery)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
