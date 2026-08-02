#!/usr/bin/env python3
"""Compile and execute the bounded NiusThread Arduino facade."""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[3]
FACADE = ROOT / "packages" / "arduino" / "src"

FAKE_THREAD = r'''#ifndef THREAD_H
#define THREAD_H
#include <stddef.h>
#include <stdint.h>
uint32_t micros();
class ThreadClass {
public:
  enum Status : int8_t { THREAD_OK=0, THREAD_NOT_STARTED=-2, THREAD_BAD_PARAM=-3,
    THREAD_FAILURE=-4 };
  enum Role : uint8_t { ROLE_DISABLED=0, ROLE_DETACHED=1, ROLE_CHILD=2,
    ROLE_ROUTER=3, ROLE_LEADER=4 };
  enum LinkMode : uint8_t { LINK_ROUTER=0, LINK_MED=1, LINK_SED=2 };
  bool available = false;
  bool attached = false;
  bool fail = false;
  unsigned processed = 0;
  static ThreadClass *active;
  static Status begin() { active->available = !active->fail; return active->fail ? THREAD_FAILURE : THREAD_OK; }
  static void end() { active->available = false; active->attached = false; }
  static Status setNetwork(const char *, uint8_t, uint16_t, const uint8_t[16], const uint8_t[8]=nullptr) { return active->fail ? THREAD_FAILURE : THREAD_OK; }
  static Status setLinkMode(LinkMode, uint32_t=1000) { return active->fail ? THREAD_FAILURE : THREAD_OK; }
  static Status start() { return active->fail ? THREAD_FAILURE : THREAD_OK; }
  static Status stop() { active->attached = false; return THREAD_OK; }
  static void process() { ++active->processed; }
  static bool isAttached() { return active->attached; }
  static Status coapStart(uint16_t=5683) { return active->fail ? THREAD_FAILURE : THREAD_OK; }
  static Status coapPost(const char *, const char *, const char *) { return active->fail ? THREAD_FAILURE : THREAD_OK; }
  static Role role() { return active->attached ? ROLE_ROUTER : ROLE_DETACHED; }
  static uint16_t rloc16() { return 0x1234; }
};
#endif
'''

TEST = r'''#include <assert.h>
#include <stdint.h>
static uint32_t now_us = 10;
uint32_t micros() { return now_us++; }
#include <NobroNiusThread.h>
ThreadClass *ThreadClass::active = nullptr;

int main() {
  ThreadClass thread;
  ThreadClass::active = &thread;
  nobro::NiusThreadAdapter<8, 8, 16> mesh(thread);
  uint8_t key[16] = {};
  assert(mesh.begin("nobro", 15, 0x1234, key) == NOBRO_MESH_OK);
  assert(mesh.state() == NOBRO_MESH_ATTACHING);
  assert(mesh.process(20) == NOBRO_MESH_OK);
  thread.attached = true;
  assert(mesh.process(20) == NOBRO_MESH_OK);
  assert(mesh.state() == NOBRO_MESH_ATTACHED && mesh.rloc16() == 0x1234);
  assert(mesh.coapStart() == NOBRO_MESH_OK);
  assert(mesh.coapPost("fd00::1", "status", "hello") == NOBRO_MESH_OK);
  assert(mesh.coapPost("fd00::1", "status", "too-long-text") == NOBRO_MESH_TOO_LARGE);
  now_us = 30;
  assert(mesh.process(20) == NOBRO_MESH_DEADLINE_MISS);
  mesh.quiesce();
  assert(mesh.state() == NOBRO_MESH_SUSPENDED);
  assert(mesh.recover() == NOBRO_MESH_OK);
  mesh.release();
  assert(mesh.state() == NOBRO_MESH_DOWN);
  const nobro_mesh_diagnostics_t diagnostics = mesh.diagnostics();
  assert(diagnostics.process_calls == 3 && diagnostics.messages_accepted == 1);
  assert(diagnostics.deadline_misses == 1 && diagnostics.recoveries == 1);
  return 0;
}
'''


def main() -> int:
    cxx = next(
        (path for name in ("g++", "clang++", "c++") if (path := shutil.which(name))),
        None,
    )
    if not cxx:
        print("THREAD FACADE: FAIL (no C++ compiler)")
        return 1
    with tempfile.TemporaryDirectory(prefix="nobro-thread-facade-") as temp:
        root = pathlib.Path(temp)
        (root / "Thread.h").write_text(FAKE_THREAD, encoding="utf-8")
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
            print("THREAD FACADE: FAIL (compile)")
            print((built.stdout + built.stderr).strip())
            return 1
        ran = subprocess.run([str(binary)], capture_output=True, text=True)
        if ran.returncode:
            print(f"THREAD FACADE: FAIL (runtime {ran.returncode})")
            print((ran.stdout + ran.stderr).strip())
            return 1
    print("THREAD FACADE: PASS (bounds, process deadline, CoAP, lifecycle, recovery)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
