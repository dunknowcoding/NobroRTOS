# Install Rust into AIRON/_work/toolchain (portable, gitignored).
$ErrorActionPreference = "Stop"
$workRoot = "F:\Arduino\driver\AIRON\_work"
$toolchain = Join-Path $workRoot "toolchain"
$env:RUSTUP_HOME = Join-Path $toolchain "rustup"
$env:CARGO_HOME = Join-Path $toolchain "cargo"
$env:PATH = "$env:CARGO_HOME\bin;$env:PATH"
$env:CARGO_TARGET_DIR = Join-Path $workRoot "cargo-target"
$env:DEFMT_LOG = "info"

New-Item -ItemType Directory -Force -Path $env:RUSTUP_HOME, $env:CARGO_HOME, $env:CARGO_TARGET_DIR | Out-Null

$init = Join-Path $workRoot "downloads\rustup-init.exe"
if (-not (Test-Path $init)) {
    New-Item -ItemType Directory -Force -Path (Split-Path $init) | Out-Null
    Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $init
}

if (-not (Test-Path (Join-Path $env:CARGO_HOME "bin\rustc.exe"))) {
    & $init -y --default-toolchain stable --profile minimal
}

rustup target add thumbv7em-none-eabihf
Set-Location "F:\Arduino\driver\AIRON\aion"
cargo build -p mvk-ppi-timestamp --release 2>&1
