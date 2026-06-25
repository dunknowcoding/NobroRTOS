# NobroRTOS toolchain bootstrap (Windows).
# Installs the Rust embedded target + llvm-tools and points at a flash tool.
# Safe: it does NOT auto-run the rustup network installer; if rustup is missing it
# tells you where to get it.
$ErrorActionPreference = "Stop"
function Have($n) { $null -ne (Get-Command $n -ErrorAction SilentlyContinue) }

if (-not (Have rustup)) {
    Write-Host "rustup not found. Install it from https://rustup.rs and re-run this script." -ForegroundColor Yellow
    exit 1
}

Write-Host "Adding embedded target + llvm-tools..."
rustup target add thumbv7em-none-eabihf
rustup component add llvm-tools-preview

if (Have JLink) {
    Write-Host "J-Link CLI found." -ForegroundColor Green
} else {
    Write-Host "Flash tool: install SEGGER J-Link (JLink.exe), or run 'cargo install probe-rs-tools'." -ForegroundColor Yellow
    Write-Host "  (probe-rs on Windows needs the J-Link bound to WinUSB via Zadig, which disables JLink.exe.)"
}

Write-Host ""
Write-Host "Toolchain ready. Next:" -ForegroundColor Green
Write-Host "  cd core; cargo build -p imu-i2c-demo --release"
Write-Host "  python ..\tools\nobro_hw_eval.py imu        # build + flash + verify on board1"
