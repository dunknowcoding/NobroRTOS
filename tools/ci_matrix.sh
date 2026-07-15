#!/usr/bin/env bash
# Cross-MCU CI matrix: one command running every build + test gate.
#   1. host tests for all portable crates
#   2. cross-compilation of the portable core for all 6 MCU families
#   3. maintained standalone port binaries
#   4. board-profile + SDK-manifest validators
# Exit 0 = the whole matrix is green.
set -u
set -o pipefail
cd "$(dirname "$0")/.." || exit 1
fails=0
total=0
temp_logs=()

cleanup() {
  rm -f "${temp_logs[@]}"
}
trap cleanup EXIT INT TERM

gate() {
  total=$((total + 1))
  local name="$1"; shift
  local log
  log="$(mktemp)"
  temp_logs+=("$log")
  if "$@" >"$log" 2>&1; then
    echo "[ OK ] $name"
  else
    echo "[FAIL] $name"
    cat "$log"
    fails=$((fails + 1))
  fi
  rm -f "$log"
}

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/_work/ct-ci}"
HOST_TARGET="${HOST_TARGET:-$(rustc -vV | sed -n 's/^host: //p' | tr -d '\r')}"
export HOST_TARGET

gate "host tests (portable crates)" \
  bash -c 'cd core && cargo test --locked -p nobro-admission -p nobro-kernel -p nobro-sal -p nobro-net -p nobro-crypto \
    -p nobro-ml -p nobro-sensor -p nobro-power -p nobro-control \
    --target "$HOST_TARGET"'

gate "capacity-report feature target build" \
  bash -c 'cd core && cargo check --locked --target thumbv7em-none-eabihf \
    -p nobro-kernel --features capacity-report'

gate "preemption contracts host tests" \
  bash -c 'cd core && cargo test --locked --target "$HOST_TARGET" \
    -p nobro-kernel --features preemptive -p nobro-admission'

gate "nRF52840 PSP/PendSV target build" \
  bash -c 'cd core && cargo check --locked --target thumbv7em-none-eabihf \
    -p nobro-kernel --features preemptive && \
    cargo check --locked --target thumbv7em-none-eabihf -p nobro-hal \
    --no-default-features --features platform-nrf52840-rt,board-promicro-nosd,cortex-m-slice && \
    ! cargo check --locked --target thumbv7em-none-eabihf -p nobro-hal \
    --no-default-features --features platform-nrf52840-rt,board-nicenano-s140,cortex-m-slice'

gate "deadline masking" python tools/check_timebase_masking.py

gate "accounting semantics" python tools/check_accounting_semantics.py

gate "nano kernel build/admission/symbol budgets" \
  python tools/check_nano_kernel.py

gate "static budget analyzer" python tools/static_budget.py --selftest

gate "portability matrix (6 MCU families)" bash tools/check_portability.sh

gate "reset platform evidence receipts" \
  python tools/check_platform_tiers.py --begin-receipts cross-mcu

gate "nRF52840 HAL target build" \
  python tools/check_platform_tiers.py --run-gate nrf52840-target-build

gate "nRF52840 USB target build" \
  python tools/check_platform_tiers.py --run-gate nrf52840-usb-target-build

gate "nRF52840 USB application link builds" \
  bash -c 'cd core && \
    cargo build --locked --release --target thumbv7em-none-eabihf \
      -p usb-cdc-demo --bin usb_cdc_demo --no-default-features \
      --features board-promicro-nosd && \
    cargo build --locked --release --target thumbv7em-none-eabihf \
      -p usb-cdc-demo --bin usb_cdc_demo_s140 --no-default-features \
      --features board-nicenano-s140 && \
    cargo build --locked --release --target thumbv7em-none-eabihf \
      -p ai-usb-demo --bin ai_usb_demo --no-default-features \
      --features board-promicro-nosd'

gate "nRF52840 application static budgets" \
  bash -c 'python tools/static_budget.py "$CARGO_TARGET_DIR/thumbv7em-none-eabihf/release/usb_cdc_demo" \
      --flash-budget 30000 --static-ram-budget 2048 --ram-budget 3800 --stack-budget 2048 --top 3 && \
    python tools/static_budget.py "$CARGO_TARGET_DIR/thumbv7em-none-eabihf/release/usb_cdc_demo_s140" \
      --flash-budget 30000 --static-ram-budget 2048 --ram-budget 3800 --stack-budget 2048 --top 3 && \
    python tools/static_budget.py "$CARGO_TARGET_DIR/thumbv7em-none-eabihf/release/ai_usb_demo" \
      --flash-budget 30000 --static-ram-budget 2048 --ram-budget 3800 --stack-budget 2048 --top 3'

gate "esp32c3 port and USB demo build" \
  python tools/check_platform_tiers.py --run-gate esp32c3-target-build

gate "esp32s3 port build (required Xtensa toolchain)" \
  python tools/check_platform_tiers.py --run-gate esp32s3-target-build

gate "rp2350 port build" \
  python tools/check_platform_tiers.py --run-gate rp2350-target-build

gate "USB RA4M1 backend host tests" \
  python tools/check_platform_tiers.py --run-gate ra4m1-usb-host

gate "USB Serial/JTAG ESP32-C3 backend host tests" \
  python tools/check_platform_tiers.py --run-gate esp32c3-usb-host

gate "USB Serial/JTAG ESP32-S3 backend host tests" \
  python tools/check_platform_tiers.py --run-gate esp32s3-usb-host

gate "ra4m1 provider conformance" \
  python tools/check_platform_tiers.py --run-gate ra4m1-provider-host

gate "ra4m1 port build" \
  python tools/check_platform_tiers.py --run-gate ra4m1-target-build

gate "samd21 port build" \
  bash -c 'cd core/ports/samd21 && CARGO_TARGET_DIR="$PWD/../../../_work/ct-samd" cargo build --locked --release'

gate "Tier-C prebuilt library and link" \
  python tools/build_libnobro.py --build

gate "board profiles" python tools/check_board_profiles.py
gate "sdk manifest" python tools/check_sdk_manifest.py
gate "platform evidence receipts" \
  python tools/check_platform_tiers.py --assert-receipts cross-mcu

echo "CI MATRIX: $((total - fails))/$total gates green"
test "$fails" -eq 0
