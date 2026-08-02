#!/usr/bin/env python3
"""Compile and execute the bounded NiusDisplay Arduino facade."""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[3]
FACADE = ROOT / "packages" / "arduino" / "src"

FAKE_NIUS = r'''#ifndef NIUS_DISPLAY_H
#define NIUS_DISPLAY_H
#include <stdint.h>
typedef int16_t nd_coord;
typedef uint32_t nd_color;
#define ND_OK 0
#define NIUS_RGB(r,g,b) ((nd_color)(((uint32_t)(r)<<16)|((uint32_t)(g)<<8)|(b)))
uint32_t micros();
class NiusSurface {
public:
  bool is_ready = true;
  unsigned pixels = 0;
  int flush_result = ND_OK;
  bool ready() const { return is_ready; }
  nd_coord width() const { return 2; }
  nd_coord height() const { return 2; }
  void drawPixel(nd_coord, nd_coord, nd_color) { ++pixels; }
  int display() { return flush_result; }
};
#endif
'''

TEST = r'''#include <assert.h>
#include <stdint.h>
static uint32_t now_us = 10;
uint32_t micros() { return now_us++; }
#include <NobroNiusDisplay.h>

int main() {
  NiusSurface surface;
  nobro::NiusDisplayAdapter<1> display(surface);
  assert(display.begin() == NOBRO_DISPLAY_OK);
  const uint8_t frame[8] = {0xf8,0, 0x07,0xe0, 0,0x1f, 0xff,0xff};
  nobro_display_receipt_t receipt = {};
  const nobro_display_region_t region = {0,0,2,2};
  assert(display.submit(region, frame, sizeof frame, 100, &receipt) == NOBRO_DISPLAY_OK);
  assert(receipt.version == NOBRO_DISPLAY_RECEIPT_VERSION);
  assert(display.submit(region, frame, sizeof frame, 100, &receipt) == NOBRO_DISPLAY_BACKPRESSURED);
  const uint32_t sequence = receipt.sequence;
  assert(display.pump() == NOBRO_DISPLAY_OK);
  assert(surface.pixels == 4);
  assert(display.receipt(sequence, &receipt));
  assert(receipt.status == NOBRO_DISPLAY_FRAME_COMPLETE);
  assert(receipt.completed_us != 0);
  assert(display.submit(region, frame, 7, 100, &receipt) == NOBRO_DISPLAY_INVALID_PAYLOAD);
  assert(display.submit(region, frame, 8, 100, &receipt) == NOBRO_DISPLAY_OK);
  assert(display.cancel(receipt.sequence) == NOBRO_DISPLAY_OK);
  assert(display.receipt(receipt.sequence, &receipt));
  assert(receipt.status == NOBRO_DISPLAY_FRAME_CANCELLED);
  display.quiesce();
  assert(display.state() == NOBRO_DISPLAY_SUSPENDED);
  assert(display.recover() == NOBRO_DISPLAY_OK);
  display.release();
  assert(display.state() == NOBRO_DISPLAY_DOWN);
  return 0;
}
'''


def main() -> int:
    cxx = next((path for name in ("g++", "clang++", "c++")
                if (path := shutil.which(name))), None)
    if not cxx:
        print("DISPLAY FACADE: FAIL (no C++ compiler)")
        return 1
    with tempfile.TemporaryDirectory(prefix="nobro-display-facade-") as temp:
        root = pathlib.Path(temp)
        (root / "NiusDisplay.h").write_text(FAKE_NIUS, encoding="utf-8")
        source = root / "test.cpp"
        source.write_text(TEST, encoding="utf-8")
        binary = root / ("test.exe" if sys.platform == "win32" else "test")
        built = subprocess.run([
            cxx, "-std=c++11", "-Wall", "-Wextra", "-Werror",
            f"-I{root}", f"-I{FACADE}", str(source), "-o", str(binary),
        ], capture_output=True, text=True)
        if built.returncode:
            print("DISPLAY FACADE: FAIL (compile)")
            print((built.stdout + built.stderr).strip())
            return 1
        ran = subprocess.run([str(binary)], capture_output=True, text=True)
        if ran.returncode:
            print(f"DISPLAY FACADE: FAIL (runtime {ran.returncode})")
            print((ran.stdout + ran.stderr).strip())
            return 1
    print("DISPLAY FACADE: PASS (bounds, receipts, backpressure, cancellation, lifecycle)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
