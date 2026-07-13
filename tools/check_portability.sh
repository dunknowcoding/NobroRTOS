#!/usr/bin/env bash
# NobroRTOS portable-core cross-MCU compatibility check.
#
# Builds the platform-agnostic crates (kernel, SAL, net, crypto, ML, sensor, power, and
# the embedded-hal sensor adapters) for each supported MCU family's bare-metal Rust
# target. These map to MCU families reachable via the Arduino board packages on the
# matrix (SAMD/RP2040 M0+, SAM M3, nRF52840/UNO-R4 M4F, RP2350 M33, ESP32-C3/C6 RISC-V).
# Requires the targets: rustup target add <t> (skipped with a hint if missing).
set -u
cd "$(dirname "$0")/../core" || exit 1

PORTABLE="-p nobro-sal -p nobro-kernel -p nobro-net -p nobro-crypto -p nobro-ml \
-p nobro-sensor -p nobro-power -p nobro-adapter-ina3221 -p nobro-adapter-bmp280 \
-p nobro-adapter-icm45686 -p nobro-adapter-nn-motion-ai"

TARGETS=(
  "thumbv6m-none-eabi|Cortex-M0+ (SAMD, RP2040)"
  "thumbv7m-none-eabi|Cortex-M3 (Arduino SAM)"
  "thumbv7em-none-eabihf|Cortex-M4F (nRF52840, UNO R4)"
  "thumbv8m.main-none-eabihf|Cortex-M33 (RP2350 / Pico 2)"
  "riscv32imc-unknown-none-elf|RISC-V rv32imc (ESP32-C3)"
  "riscv32imac-unknown-none-elf|RISC-V rv32imac (ESP32-C6)"
)

echo "=== NobroRTOS portable-core cross-MCU compatibility ==="
pass=0
tot=0
for entry in "${TARGETS[@]}"; do
  t="${entry%%|*}"
  d="${entry##*|}"
  tot=$((tot + 1))
  if ! rustup target list --installed | grep -q "^${t}$"; then
    echo "[SKIP] ${t}  - ${d} (run: rustup target add ${t})"
    continue
  fi
  if cargo build ${PORTABLE} --release --target "${t}" >/dev/null 2>&1; then
    echo "[ OK ] ${t}  - ${d}"
    pass=$((pass + 1))
  else
    echo "[FAIL] ${t}  - ${d}"
  fi
done
echo "RESULT: ${pass}/${tot} MCU families"
test "${pass}" -eq "${tot}"
