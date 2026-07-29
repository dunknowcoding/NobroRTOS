#!/usr/bin/env bash
# Cross-MCU CI matrix: one command running every build + test gate.
#   1. host tests for all portable crates
#   2. cross-compilation of the portable core for all 6 MCU families
#   3. maintained standalone port binaries
#   4. board-profile + SDK-manifest validators
# Exit 0 = the whole matrix is green.
set -u
set -o pipefail
cd "$(dirname "$0")/../.." || exit 1
fails=0
total=0
temp_logs=()
CURRENT_BASH="${BASH:-bash}"

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

gate "fail-closed host workspace" \
  python tools/checks/core/check_host_workspace.py

gate "explicit target-only workspace" \
  python tools/checks/core/check_host_workspace.py --target-only \
    --target thumbv7em-none-eabihf

gate "wireless adaptive alloc feature tests" \
  "$CURRENT_BASH" -c 'cd core && cargo test --locked --target "$HOST_TARGET" \
    -p nobro-wireless --features alloc'

gate "capacity-report feature target build" \
  "$CURRENT_BASH" -c 'cd core && cargo check --locked --target thumbv7em-none-eabihf \
    -p nobro-kernel --features capacity-report'

gate "preemption contracts host tests" \
  "$CURRENT_BASH" -c 'cd core && cargo test --locked --target "$HOST_TARGET" \
    -p nobro-kernel --features preemptive -p nobro-admission'

gate "secure symmetric-only build" \
  "$CURRENT_BASH" -c 'cd core && cargo check --locked --target "$HOST_TARGET" \
    -p nobro-secure --no-default-features'

gate "nRF52840 PSP/PendSV target build" \
  "$CURRENT_BASH" -c 'cd core && cargo check --locked --target thumbv7em-none-eabihf \
    -p nobro-kernel --features preemptive && \
    cargo check --locked --target thumbv7em-none-eabihf -p nobro-hal \
    --no-default-features --features platform-nrf52840-rt,board-promicro-nosd,cortex-m-slice && \
    cargo build --locked --target thumbv7em-none-eabihf \
    -p isolation-demo --release && \
    ! cargo check --locked --target thumbv7em-none-eabihf -p nobro-hal \
    --no-default-features --features platform-nrf52840-rt,board-promicro-s140,cortex-m-slice'

gate "board boot slot adapter target build" \
  "$CURRENT_BASH" -c 'cd core && cargo build --locked --release --target thumbv7em-none-eabihf \
    -p boot-slot-demo'

gate "deadline masking" python tools/checks/core/check_timebase_masking.py

gate "accounting semantics" python tools/checks/core/check_accounting_semantics.py

gate "nano kernel build/admission/symbol budgets" \
  python tools/checks/core/check_nano_kernel.py

gate "Python-authored native firmware target build" \
  "$CURRENT_BASH" -c 'python tutorials/rover-python/app.py _work/python-authoring/app.json && \
    python sdk/cli/nobro.py firmware _work/python-authoring/app.json \
      --out _work/python-firmware --build && \
    python sdk/cli/nobro.py budget \
      "$CARGO_TARGET_DIR/thumbv7em-none-eabihf/release/nobro-app-python-rover" \
      --flash-budget 4000 --static-ram-budget 64 --ram-budget 512 \
      --stack-budget 400 --cycle-budget 1200'

gate "task/wire authoring parity + block-authored target build" \
  "$CURRENT_BASH" -c 'python tools/checks/product/check_app_authoring.py && \
    python sdk/cli/nobro.py firmware tutorials/hello-device/app.json \
      --out _work/block-firmware --build'

gate "stable diagnostic registry + generated index" \
  python tools/build/gen_error_codes.py --check

gate "self-contained distribution artifacts" \
  python tools/checks/release/check_distribution_artifacts.py

gate "static budget analyzer" python sdk/cli/nobro.py budget --selftest

gate "flash tool fail-closed parser" python sdk/cli/nobro.py flash --selftest

gate "portability matrix (6 MCU families)" \
  "$CURRENT_BASH" tools/checks/platforms/check_portability.sh

gate "HAL/SAL v2 no_std contracts" \
  "$CURRENT_BASH" -c 'cd core && \
    cargo check --locked -p nobro-hal --no-default-features \
      --features contract-only --target thumbv7em-none-eabihf && \
    cargo check --locked -p nobro-device --target thumbv7em-none-eabihf'

gate "portable adapter conformance" \
  "$CURRENT_BASH" -c 'cd core && \
    cargo test --locked --target "$HOST_TARGET" -p nobro-device \
      -p nobro-adapter-mpu9250-imu --no-default-features \
      -p portable-adapter-conformance && \
    cargo check --locked --target thumbv6m-none-eabi \
      -p nobro-adapter-mpu9250-imu --no-default-features \
      -p portable-adapter-conformance'

gate "exactly one nRF board composition" \
  "$CURRENT_BASH" -c 'cd core && \
    cargo check --locked -p nobro-hal --target thumbv7em-none-eabihf \
      --no-default-features --features board-promicro-s140 && \
    cargo check --locked -p nobro-hal --target thumbv7em-none-eabihf \
      --no-default-features --features board-nicenano-s140 && \
    ! cargo check --locked -p nobro-hal --target thumbv7em-none-eabihf \
      --no-default-features --features platform-nrf52840 && \
    ! cargo check --locked -p nobro-hal --target thumbv7em-none-eabihf \
      --no-default-features --features board-promicro-nosd,board-promicro-s140'

gate "reset platform evidence receipts" \
  python tools/checks/platforms/check_platform_tiers.py --begin-receipts cross-mcu

gate "nRF52840 no-SoftDevice HAL target build" \
  python tools/checks/platforms/check_platform_tiers.py --run-gate nrf52840-nosd-target-build

gate "nRF52840 S140 HAL target build" \
  python tools/checks/platforms/check_platform_tiers.py --run-gate nrf52840-s140-target-build

gate "nRF52840 no-SoftDevice USB target build" \
  python tools/checks/platforms/check_platform_tiers.py --run-gate nrf52840-nosd-usb-target-build

gate "nRF52840 S140 USB target build" \
  python tools/checks/platforms/check_platform_tiers.py --run-gate nrf52840-s140-usb-target-build

gate "nRF52840 AI USB application link build" \
  "$CURRENT_BASH" -c 'cd core && \
    cargo build --locked --release --target thumbv7em-none-eabihf \
      -p ai-usb-demo --bin ai_usb_demo --no-default-features \
      --features board-promicro-nosd'

gate "nRF52840 application static budgets" \
  "$CURRENT_BASH" -c 'python sdk/cli/nobro.py budget "$CARGO_TARGET_DIR/thumbv7em-none-eabihf/release/usb_cdc_demo" \
      --flash-budget 31000 --static-ram-budget 2048 --ram-budget 3840 --stack-budget 2048 \
      --cycle-budget 7600 --top 3 && \
    python sdk/cli/nobro.py budget "$CARGO_TARGET_DIR/thumbv7em-none-eabihf/release/usb_cdc_demo_s140" \
      --flash-budget 32000 --static-ram-budget 2048 --ram-budget 3840 --stack-budget 2048 \
      --cycle-budget 7600 --top 3 && \
    python sdk/cli/nobro.py budget "$CARGO_TARGET_DIR/thumbv7em-none-eabihf/release/ai_usb_demo" \
      --flash-budget 30000 --static-ram-budget 2048 --ram-budget 3800 --stack-budget 2048 \
      --cycle-budget 6500 --top 3'

gate "esp32c3 port and USB demo build" \
  python tools/checks/platforms/check_platform_tiers.py --run-gate esp32c3-target-build

gate "esp32s3 port build (required Xtensa toolchain)" \
  python tools/checks/platforms/check_platform_tiers.py --run-gate esp32s3-target-build

# First-party S3 libraries and binaries are strict-linted. Cargo does not lint
# dependency packages in this invocation, so generated PAC code is not
# misrepresented as part of the product lint claim.
gate "esp32s3 first-party strict lint" \
  "$CURRENT_BASH" -c 'cd core/ports/esp32s3 && cargo +esp clippy --locked --release --lib --bins -- -D warnings'

gate "rp2350 port build" \
  python tools/checks/platforms/check_platform_tiers.py --run-gate rp2350-target-build

gate "rp2350 DMA completion provider build" \
  python tools/checks/platforms/check_platform_tiers.py --run-gate rp2350-dma-target-build

gate "USB RA4M1 backend host tests" \
  python tools/checks/platforms/check_platform_tiers.py --run-gate ra4m1-usb-host

gate "USB Serial/JTAG ESP32-C3 backend host tests" \
  python tools/checks/platforms/check_platform_tiers.py --run-gate esp32c3-usb-host

gate "USB Serial/JTAG ESP32-S3 backend host tests" \
  python tools/checks/platforms/check_platform_tiers.py --run-gate esp32s3-usb-host

gate "ra4m1 provider conformance" \
  python tools/checks/platforms/check_platform_tiers.py --run-gate ra4m1-provider-host

gate "ra4m1 port build" \
  python tools/checks/platforms/check_platform_tiers.py --run-gate ra4m1-target-build

gate "ra4m1 event-paced DMA provider build" \
  python tools/checks/platforms/check_platform_tiers.py --run-gate ra4m1-event-dma-target-build

gate "samd21 port build" \
  "$CURRENT_BASH" -c 'cd core/ports/samd21 && CARGO_TARGET_DIR="$PWD/../../../_work/ct-samd" cargo build --locked --release'

gate "Tier-C prebuilt library and link" \
  python tools/build/build_libnobro.py --build

gate "board profiles" python tools/checks/platforms/check_board_profiles.py
gate "board-owned portable firmware generation" \
  python tools/checks/platforms/check_firmware_generation.py
gate "sdk manifest" python tools/checks/release/check_sdk_manifest.py
gate "platform evidence receipts" \
  python tools/checks/platforms/check_platform_tiers.py --assert-receipts cross-mcu

echo "CI MATRIX: $((total - fails))/$total gates green"
test "$fails" -eq 0
