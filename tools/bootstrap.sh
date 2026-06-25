#!/usr/bin/env sh
# NobroRTOS toolchain bootstrap (Linux / macOS).
# Installs the Rust embedded target + llvm-tools and points at a flash tool.
# Safe: it does NOT auto-run the rustup network installer; if rustup is missing it
# tells you where to get it.
set -e

if ! command -v rustup >/dev/null 2>&1; then
    echo "rustup not found. Install it:"
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    echo "then re-run this script."
    exit 1
fi

echo "Adding embedded target + llvm-tools..."
rustup target add thumbv7em-none-eabihf
rustup component add llvm-tools-preview

if command -v JLinkExe >/dev/null 2>&1; then
    echo "J-Link CLI found."
else
    echo "Flash tool: install SEGGER J-Link (JLinkExe), or run 'cargo install probe-rs-tools'."
fi

echo ""
echo "Toolchain ready. Next:"
echo "  cd core && cargo build -p imu-i2c-demo --release"
echo "  python3 ../tools/nobro_hw_eval.py imu        # build + flash + verify on board1"
