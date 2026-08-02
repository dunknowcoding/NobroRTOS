#!/usr/bin/env python3
"""Compile and execute the bounded NiusCam facade against a fake camera."""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[3]
FACADE = ROOT / "packages" / "arduino" / "src"

FREERTOS = r'''#ifndef FREERTOS_H
#define FREERTOS_H
typedef int portMUX_TYPE;
#define portMUX_INITIALIZER_UNLOCKED 0
inline void portENTER_CRITICAL(portMUX_TYPE *) {}
inline void portEXIT_CRITICAL(portMUX_TYPE *) {}
#endif
'''

NIUSCAM = r'''#ifndef NIUSCAM_H
#define NIUSCAM_H
#include <stddef.h>
#include <stdint.h>
namespace NiusCam {
struct Result { bool value; bool ok() const { return value; } };
struct BoardProfile {};
struct Config { static Config Balanced() { return Config(); } };
class Frame {
public:
  Frame() : bytes_(0) {}
  explicit Frame(size_t bytes) : bytes_(bytes) {}
  Frame(Frame &&other) : bytes_(other.bytes_) { other.bytes_ = 0; }
  Frame &operator=(Frame &&other) { bytes_=other.bytes_; other.bytes_=0; return *this; }
  Frame(const Frame &) = delete;
  Frame &operator=(const Frame &) = delete;
  explicit operator bool() const { return bytes_ != 0; }
  size_t size() const { return bytes_; }
  void release() { bytes_ = 0; }
private:
  size_t bytes_;
};
class Camera {
public:
  bool ready = false;
  bool fail_begin = false;
  bool fail_recover = false;
  size_t next_bytes = 32;
  Result begin(const BoardProfile &, const Config &) { ready=!fail_begin; return {!fail_begin}; }
  bool isReady() const { return ready; }
  Frame capture() { return ready ? Frame(next_bytes) : Frame(); }
  Result recover() { ready=!fail_recover; return {!fail_recover}; }
};
}
#endif
'''

TEST = r'''#include <assert.h>
#include <NobroNiusCam.h>

int main() {
  NiusCam::Camera camera;
  NiusCam::BoardProfile board;
  nobro::NiusCamAdapter first(camera, 64, 2, 96, 1, 7);
  nobro::NiusCamAdapter duplicate(camera, 64, 2, 96, 1, 8);
  assert(first.begin(board));
  assert(first.mounted() && first.generation() == 1);
  assert(!duplicate.begin(board));
  NiusCam::Frame frame = first.capture(10, 20);
  assert(frame && first.diagnostics().frames_accepted == 1);
  assert(!first.capture(10, 20));
  first.release(frame);
  assert(!first.capture(21, 20));
  camera.next_bytes = 80;
  assert(!first.capture(10, 20));
  first.quiesce();
  assert(first.quiesced() && !first.capture(10, 20));
  assert(first.recover() && first.generation() == 2);
  camera.next_bytes = 16;
  first.resetWindow();
  NiusCam::Frame recovered = first.capture(10, 20);
  assert(recovered);
  first.release(recovered);
  assert(first.diagnostics().recoveries == 1);
  return 0;
}
'''


def main() -> int:
    cxx = next(
        (path for name in ("g++", "clang++", "c++") if (path := shutil.which(name))),
        None,
    )
    if not cxx:
        print("CAMERA FACADE: FAIL (no C++ compiler)")
        return 1
    with tempfile.TemporaryDirectory(prefix="nobro-camera-facade-") as temp:
        root = pathlib.Path(temp)
        (root / "freertos").mkdir()
        (root / "freertos" / "FreeRTOS.h").write_text(FREERTOS, encoding="utf-8")
        (root / "NiusCam.h").write_text(NIUSCAM, encoding="utf-8")
        source = root / "test.cpp"
        source.write_text(TEST, encoding="utf-8")
        binary = root / ("test.exe" if sys.platform == "win32" else "test")
        built = subprocess.run(
            [cxx, "-std=c++11", "-Wall", "-Wextra", "-Werror", f"-I{root}",
             f"-I{FACADE}", str(source), "-o", str(binary)],
            capture_output=True,
            text=True,
        )
        if built.returncode:
            print("CAMERA FACADE: FAIL (compile)")
            print((built.stdout + built.stderr).strip())
            return 1
        ran = subprocess.run([str(binary)], capture_output=True, text=True)
        if ran.returncode:
            print(f"CAMERA FACADE: FAIL (runtime {ran.returncode})")
            print((ran.stdout + ran.stderr).strip())
            return 1
    print("CAMERA FACADE: PASS (ownership, bounds, deadline, recovery)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
