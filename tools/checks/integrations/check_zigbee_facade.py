#!/usr/bin/env python3
"""Compile and execute the bounded NiusZigbee CC2530 facade."""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[3]
FACADE = ROOT / "packages" / "arduino" / "src"

FAKE_ZIGBEE = r'''#ifndef CC2530_RADIO_H
#define CC2530_RADIO_H
#include <stddef.h>
#include <stdint.h>
uint32_t micros();
struct CC2530MacStats { uint16_t retransmits; uint16_t noAck; };
struct ZigbeeNwk { static const uint8_t kDefaultRadius = 10; };
class CC2530Radio {
public:
  static const uint8_t kMaxPayload = 125;
  bool fail = false;
  bool begin(uint8_t=11, uint32_t=115200) { return !fail; }
  bool send(const uint8_t *, uint8_t) { return !fail; }
  bool sendWithRetries(const uint8_t *, uint8_t, uint8_t) { return !fail; }
  bool sendApsData(uint16_t,uint16_t,uint16_t,uint16_t,uint16_t,uint8_t,uint16_t,
      uint16_t,uint8_t,const uint8_t*,uint8_t,uint8_t,bool) { return !fail; }
  void poll() {}
  bool ping() { return !fail; }
  bool setChannel(uint8_t) { return !fail; }
  bool getMacStats(CC2530MacStats &stats) { stats={2,1}; return !fail; }
};
#endif
'''

TEST = r'''#include <assert.h>
#include <stdint.h>
static uint32_t now_us = 10;
uint32_t micros() { return now_us++; }
#include <NobroNiusZigbee.h>

int main() {
  CC2530Radio radio;
  nobro::NiusCc2530Adapter link(radio);
  assert(link.begin(15));
  const uint8_t payload[3] = {1,2,3};
  assert(link.sendRaw(payload, sizeof payload, 2));
  assert(link.sendAps(0x1234, 2, 1, 1, 6, 0x104, 1, payload, sizeof payload));
  assert(link.process(20));
  CC2530MacStats stats = {};
  assert(link.macStats(stats) && stats.retransmits == 2);
  link.quiesce();
  assert(!link.sendRaw(payload, sizeof payload));
  assert(link.recover());
  radio.fail = true;
  assert(!link.sendRaw(payload, sizeof payload));
  const nobro::NiusZigbeeDiagnostics diagnostics = link.diagnostics();
  assert(diagnostics.link.tx_accepted == 2 && diagnostics.link.tx_rejected == 2);
  assert(diagnostics.link.recoveries == 1);
  return 0;
}
'''


def main() -> int:
    cxx = next(
        (path for name in ("g++", "clang++", "c++") if (path := shutil.which(name))),
        None,
    )
    if not cxx:
        print("ZIGBEE FACADE: FAIL (no C++ compiler)")
        return 1
    with tempfile.TemporaryDirectory(prefix="nobro-zigbee-facade-") as temp:
        root = pathlib.Path(temp)
        (root / "CC2530Radio.h").write_text(FAKE_ZIGBEE, encoding="utf-8")
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
            print("ZIGBEE FACADE: FAIL (compile)")
            print((built.stdout + built.stderr).strip())
            return 1
        ran = subprocess.run([str(binary)], capture_output=True, text=True)
        if ran.returncode:
            print(f"ZIGBEE FACADE: FAIL (runtime {ran.returncode})")
            print((ran.stdout + ran.stderr).strip())
            return 1
    print("ZIGBEE FACADE: PASS (raw/APS bounds, deadline, lifecycle, recovery)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
