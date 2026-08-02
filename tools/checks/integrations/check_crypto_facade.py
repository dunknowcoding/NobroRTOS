#!/usr/bin/env python3
"""Compile and execute the bounded NiusCrypto Arduino facade."""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[3]
FACADE = ROOT / "packages" / "arduino" / "src"

FAKE_NIUS = r'''#ifndef NIUS_CRYPTO_H
#define NIUS_CRYPTO_H
#include <stddef.h>
#include <stdint.h>
#define NIUS_SHA256_BYTES 32
#define NIUS_AES128_KEY 16
#define NIUS_GCM_IV 12
#define NIUS_GCM_TAG 16
uint32_t micros();
namespace ncrypto {
enum class CryptoStatus : int8_t { Ok=0, HardwareMissing=-1, NotStarted=-2,
  BadParam=-3, InternalError=-4, Unsupported=-5, AuthFailed=-6 };
class CryptoEngine {
public:
  enum class Prefer : uint8_t { Auto=0, CC310, OnChip };
  bool begin_ok = true;
  bool hardware = true;
  CryptoStatus next = CryptoStatus::Ok;
  bool started = false;
  bool begin(Prefer = Prefer::Auto) { started = begin_ok; return started; }
  void end() { started = false; }
  const char *backendName() const { return started ? "fake" : "none"; }
  bool isHardwareAccelerated() const { return hardware; }
  CryptoStatus random(uint8_t *out, size_t len) {
    for (size_t i=0; i<len; ++i) out[i] = static_cast<uint8_t>(i+1);
    return take();
  }
  CryptoStatus sha256(const uint8_t *, size_t, uint8_t out[32]) {
    for (size_t i=0; i<32; ++i) out[i] = static_cast<uint8_t>(i);
    return take();
  }
  CryptoStatus aesGcmEncrypt(const uint8_t[16], const uint8_t[12], const uint8_t *,
      size_t, const uint8_t *in, uint8_t *out, size_t len, uint8_t tag[16]) {
    for (size_t i=0; i<len; ++i) out[i] = in[i];
    for (size_t i=0; i<16; ++i) tag[i] = static_cast<uint8_t>(i);
    return take();
  }
  CryptoStatus aesGcmDecrypt(const uint8_t[16], const uint8_t[12], const uint8_t *,
      size_t, const uint8_t *in, uint8_t *out, size_t len, const uint8_t[16]) {
    for (size_t i=0; i<len; ++i) out[i] = in[i];
    return take();
  }
private:
  CryptoStatus take() { CryptoStatus value=next; next=CryptoStatus::Ok; return value; }
};
}
#endif
'''

TEST = r'''#include <assert.h>
#include <stdint.h>
static uint32_t clock_us = 10;
uint32_t micros() { return clock_us++; }
#include <NobroNiusCrypto.h>

int main() {
  ncrypto::CryptoEngine engine;
  nobro::NiusCryptoAdapter crypto(engine);
  assert(crypto.begin(32) == NOBRO_CRYPTO_OK);
  uint8_t data[32] = {};
  nobro_crypto_receipt_t receipt = {};
  assert(crypto.random(data, 8, 10, 20, &receipt) == NOBRO_CRYPTO_OK);
  assert(receipt.hardware_accelerated == 1 && receipt.input_bytes == 8);
  assert(crypto.sha256(data, 33, data, 10, 20, &receipt) == NOBRO_CRYPTO_TOO_LARGE);
  assert(crypto.sha256(data, 8, data, 21, 20, &receipt) == NOBRO_CRYPTO_DEADLINE_MISS);
  engine.next = ncrypto::CryptoStatus::Unsupported;
  assert(crypto.sha256(data, 8, data, 10, 30, &receipt) == NOBRO_CRYPTO_UNSUPPORTED);
  crypto.quiesce();
  assert(crypto.state() == NOBRO_CRYPTO_SUSPENDED);
  assert(crypto.recover() == NOBRO_CRYPTO_OK);
  crypto.release();
  assert(crypto.state() == NOBRO_CRYPTO_DOWN);
  const nobro_crypto_diagnostics_t diagnostics = crypto.diagnostics();
  assert(diagnostics.completed == 1 && diagnostics.deadline_misses == 1);
  assert(diagnostics.recoveries == 1 && diagnostics.rejected == 3);
  return 0;
}
'''


def main() -> int:
    cxx = next(
        (path for name in ("g++", "clang++", "c++") if (path := shutil.which(name))),
        None,
    )
    if not cxx:
        print("CRYPTO FACADE: FAIL (no C++ compiler)")
        return 1
    with tempfile.TemporaryDirectory(prefix="nobro-crypto-facade-") as temp:
        root = pathlib.Path(temp)
        (root / "NiusCrypto.h").write_text(FAKE_NIUS, encoding="utf-8")
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
            print("CRYPTO FACADE: FAIL (compile)")
            print((built.stdout + built.stderr).strip())
            return 1
        ran = subprocess.run([str(binary)], capture_output=True, text=True)
        if ran.returncode:
            print(f"CRYPTO FACADE: FAIL (runtime {ran.returncode})")
            print((ran.stdout + ran.stderr).strip())
            return 1
    print("CRYPTO FACADE: PASS (bounds, deadlines, receipts, lifecycle, recovery)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
