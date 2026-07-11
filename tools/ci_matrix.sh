#!/usr/bin/env bash
# Cross-MCU CI matrix (M93): one command running every build + test gate.
#   1. host tests for all portable crates (includes the conformance suite)
#   2. cross-compilation of the portable core for all 6 MCU families
#   3. the ESP32-C3 and RP2350 port binaries
#   4. board-profile + SDK-manifest validators
# Exit 0 = the whole matrix is green.
set -u
cd "$(dirname "$0")/.." || exit 1
fails=0
total=0

gate() {
  total=$((total + 1))
  local name="$1"; shift
  if "$@" >/dev/null 2>&1; then
    echo "[ OK ] $name"
  else
    echo "[FAIL] $name"
    fails=$((fails + 1))
  fi
}

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/_work/ct-ci}"
HOST_TARGET="${HOST_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
export HOST_TARGET

gate "host tests (portable crates)" \
  bash -c 'cd core && cargo test -p nobro-kernel -p nobro-sal -p nobro-net -p nobro-crypto \
    -p nobro-ml -p nobro-sensor -p nobro-power -p nobro-control -p nobro-conformance \
    --target "$HOST_TARGET"'

gate "portability matrix (6 MCU families)" bash tools/check_portability.sh

gate "esp32c3 port build" \
  bash -c 'cd core/ports/esp32c3 && CARGO_TARGET_DIR="$PWD/../../../_work/ct-c3" cargo build --release'

# The Xtensa port needs the espup toolchain; skip (not fail) where it is absent.
if rustup toolchain list 2>/dev/null | grep -q "^esp"; then
  gate "esp32s3 port build (xtensa)" \
    bash -c 'cd core/ports/esp32s3 && CARGO_TARGET_DIR="$PWD/../../../_work/ct-s3" cargo +esp build --release'
else
  echo "SKIP esp32s3 port build (espup toolchain not installed)"
fi

gate "rp2350 port build" \
  bash -c 'cd core/ports/rp2350 && CARGO_TARGET_DIR="$PWD/../../../_work/ct-rp" cargo build --release'

gate "board profiles" python tools/check_board_profiles.py
gate "sdk manifest" python tools/check_sdk_manifest.py

echo "CI MATRIX: $((total - fails))/$total gates green"
test "$fails" -eq 0
