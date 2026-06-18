# Build, flash, and read AIRON Phase 2 SalEvalReport from board1 RAM (no NiusIMU hardware).
param(
    [int]$WarmupSec = 8,
    [int]$PollTimeoutSec = 30,
    [string]$WorkRoot = ""
)

$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($WorkRoot)) {
    $WorkRoot = $PSScriptRoot
}
$WorkRoot = (Resolve-Path $WorkRoot).Path
$projectRoot = Split-Path -Parent $WorkRoot
$env:CARGO_TARGET_DIR = Join-Path $WorkRoot "cargo-target"
$env:RUSTUP_HOME = Join-Path $WorkRoot "toolchain\rustup"
$env:CARGO_HOME = Join-Path $WorkRoot "toolchain\cargo"
$env:PATH = "$env:CARGO_HOME\bin;$env:PATH"

$aion = Join-Path $projectRoot "aion"
$elf = Join-Path $env:CARGO_TARGET_DIR "thumbv7em-none-eabihf\release\sal_adapter_demo"
$hex = Join-Path $WorkRoot "artifacts\sal_adapter_demo.hex"
$evalDir = Join-Path $WorkRoot "eval"
$reportJson = Join-Path $evalDir "phase2-report.json"
$jlink = "C:\Program Files\SEGGER\JLink_V944\JLink.exe"

New-Item -ItemType Directory -Force -Path (Split-Path $hex), $evalDir | Out-Null

Write-Host "== Phase 2 eval: build =="
Set-Location $aion
$prevEap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
cargo build -p sal-adapter-demo --release 2>&1 | Out-Null
$buildOk = $LASTEXITCODE
cargo objcopy -p sal-adapter-demo --release -- -O ihex $hex 2>&1 | Out-Null
$objOk = $LASTEXITCODE
$ErrorActionPreference = $prevEap
if ($buildOk -ne 0) { throw "cargo build failed ($buildOk)" }
if ($objOk -ne 0) { throw "cargo objcopy failed ($objOk)" }

Write-Host "== Phase 2 eval: locate AIRON_SAL_EVAL_REPORT =="
$rustNm = Join-Path $env:CARGO_HOME "bin\rust-nm.exe"
if (-not (Test-Path $rustNm)) { throw "rust-nm not found at $rustNm" }
$symLine = & $rustNm -g $elf | Select-String "AIRON_SAL_EVAL_REPORT"
if (-not $symLine) {
    throw "Symbol AIRON_SAL_EVAL_REPORT not found in ELF"
}
$addrHex = ($symLine.ToString().Split()[0]).Trim()
$addr = [Convert]::ToUInt32($addrHex, 16)
Write-Host "AIRON_SAL_EVAL_REPORT @ 0x$($addr.ToString('X8'))"

Write-Host "== Phase 2 eval: flash + run (poll up to ${PollTimeoutSec}s) =="
& "$WorkRoot\flash-board1.ps1" -App sal_adapter_demo | Out-Host
Start-Sleep -Seconds $WarmupSec

function Read-EvalWords([uint32]$Address) {
    $memScript = Join-Path $env:TEMP "airon_read_sal_eval.jlink"
    @"
device NRF52840_XXAA
si 1
speed 4000
connect
h
mem32 0x$($Address.ToString('X8')) 9
exit
"@ | Set-Content -Encoding ascii $memScript
    $memOut = & $jlink -NoGui 1 -CommanderScript $memScript 2>&1 | Out-String
    $words = New-Object System.Collections.Generic.List[string]
    foreach ($line in ($memOut -split "`n")) {
        if ($line -match '^\s*(200[0-9A-Fa-f]+)\s*=') {
            $rhs = ($line -split '=', 2)[1]
            foreach ($tok in ($rhs.Trim() -split '\s+')) {
                if ($tok -match '^[0-9A-Fa-f]{8}$') { [void]$words.Add($tok.ToUpper()) }
            }
        }
    }
    if ($words.Count -lt 9) { throw "Failed to parse mem32 output:`n$memOut" }
    return $words
}

$deadline = (Get-Date).AddSeconds($PollTimeoutSec)
$words = $null
do {
    $words = Read-EvalWords $addr
    $completed = [Convert]::ToUInt32($words[2], 16)
    if ($completed -eq 1) { break }
    Write-Host "  waiting for sal eval report (completed=0)..."
    $goScript = Join-Path $env:TEMP "airon_sal_eval_go.jlink"
    @"
device NRF52840_XXAA
si 1
speed 4000
connect
g
exit
"@ | Set-Content -Encoding ascii $goScript
    & $jlink -NoGui 1 -CommanderScript $goScript 2>&1 | Out-Null
    Start-Sleep -Seconds 2
} while ((Get-Date) -lt $deadline)

Write-Host "== Phase 2 eval: read RAM report =="

function U32([int]$i) { [Convert]::ToUInt32($words[$i], 16) }
function XorU32([uint32]$a, [uint32]$b) { [uint32]($a -bxor $b) }

$report = [ordered]@{
    magic              = U32 0
    version            = U32 1
    completed          = U32 2
    all_pass           = U32 3
    servo_steps        = U32 4
    servo_readback_ok  = U32 5
    imu_samples        = U32 6
    imu_plausible      = U32 7
    checksum           = U32 8
    report_address     = "0x$($addr.ToString('X8'))"
    evaluated_at       = (Get-Date).ToString("o")
    note               = "sensor-stub (no external NiusIMU)"
}

$cs = [uint32]0
$cs = XorU32 $cs ([uint32]$report.magic)
$cs = XorU32 $cs ([uint32]$report.version)
$cs = XorU32 $cs ([uint32]$report.completed)
$cs = XorU32 $cs ([uint32]$report.all_pass)
$cs = XorU32 $cs ([uint32]$report.servo_steps)
$cs = XorU32 $cs ([uint32]$report.servo_readback_ok)
$cs = XorU32 $cs ([uint32]$report.imu_samples)
$cs = XorU32 $cs ([uint32]$report.imu_plausible)

$report.checksum_ok = ([uint32]$report.checksum -eq $cs)
$report.verdict = if ($report.completed -eq 1 -and $report.all_pass -eq 1 -and $report.checksum_ok) { "PASS" } else { "FAIL" }

$report | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8 $reportJson

Write-Host ""
Write-Host "=== AIRON Phase 2 Eval: $($report.verdict) ==="
Write-Host "  robo-servo steps/readback: $($report.servo_steps)/$($report.servo_readback_ok)"
Write-Host "  sensor-stub imu/plausible: $($report.imu_samples)/$($report.imu_plausible)"
Write-Host "  report: $reportJson"

if ($report.verdict -ne "PASS") { exit 1 }
