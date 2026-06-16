# Build, flash, and read AIRON Phase 1 EvalReport from board1 RAM (no scope / RTT viewer).
param(
    [int]$WarmupSec = 10,
    [int]$PollTimeoutSec = 30,
    [string]$WorkRoot = "F:\Arduino\driver\AIRON\_work"
)

$ErrorActionPreference = "Stop"
$env:CARGO_TARGET_DIR = Join-Path $WorkRoot "cargo-target"
$env:RUSTUP_HOME = Join-Path $WorkRoot "toolchain\rustup"
$env:CARGO_HOME = Join-Path $WorkRoot "toolchain\cargo"
$env:PATH = "$env:CARGO_HOME\bin;$env:PATH"

$aion = "F:\Arduino\driver\AIRON\aion"
$elf = Join-Path $env:CARGO_TARGET_DIR "thumbv7em-none-eabihf\release\resource_sched_demo"
$hex = Join-Path $WorkRoot "artifacts\resource_sched_demo.hex"
$evalDir = Join-Path $WorkRoot "eval"
$reportJson = Join-Path $evalDir "phase1-report.json"
$jlink = "C:\Program Files\SEGGER\JLink_V944\JLink.exe"

New-Item -ItemType Directory -Force -Path (Split-Path $hex), $evalDir | Out-Null

Write-Host "== Phase 1 eval: build =="
Set-Location $aion
$prevEap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
cargo build -p resource-sched-demo --release 2>&1 | Out-Null
$buildOk = $LASTEXITCODE
cargo objcopy -p resource-sched-demo --release -- -O ihex $hex 2>&1 | Out-Null
$objOk = $LASTEXITCODE
$ErrorActionPreference = $prevEap
if ($buildOk -ne 0) { throw "cargo build failed ($buildOk)" }
if ($objOk -ne 0) { throw "cargo objcopy failed ($objOk)" }

Write-Host "== Phase 1 eval: locate AIRON_EVAL_REPORT =="
$rustNm = Join-Path $env:CARGO_HOME "bin\rust-nm.exe"
if (-not (Test-Path $rustNm)) { throw "rust-nm not found at $rustNm" }
$symLine = & $rustNm -g $elf | Select-String "AIRON_EVAL_REPORT"
if (-not $symLine) {
    throw "Symbol AIRON_EVAL_REPORT not found in ELF"
}
$addrHex = ($symLine.ToString().Split()[0]).Trim()
$addr = [Convert]::ToUInt32($addrHex, 16)
Write-Host "AIRON_EVAL_REPORT @ 0x$($addr.ToString('X8'))"

Write-Host "== Phase 1 eval: flash + run (poll up to ${PollTimeoutSec}s) =="
& "$WorkRoot\flash-board1.ps1" -App resource_sched_demo | Out-Host
Start-Sleep -Seconds $WarmupSec

function Read-EvalWords([uint32]$Address) {
    $memScript = Join-Path $env:TEMP "airon_read_eval.jlink"
    @"
device NRF52840_XXAA
si 1
speed 4000
connect
h
mem32 0x$($Address.ToString('X8')) 18
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
    if ($words.Count -lt 18) { throw "Failed to parse mem32 output:`n$memOut" }
    return $words
}

$deadline = (Get-Date).AddSeconds($PollTimeoutSec)
$words = $null
do {
    $words = Read-EvalWords $addr
    $completed = [Convert]::ToUInt32($words[2], 16)
    if ($completed -eq 1) { break }
    Write-Host "  waiting for eval report (completed=0)..."
    $goScript = Join-Path $env:TEMP "airon_eval_go.jlink"
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

Write-Host "== Phase 1 eval: read RAM report =="

function U32([int]$i) { [Convert]::ToUInt32($words[$i], 16) }

function XorU32([uint32]$a, [uint32]$b) { [uint32]($a -bxor $b) }

$report = [ordered]@{
    magic               = U32 0
    version             = U32 1
    completed           = U32 2
    all_pass            = U32 3
    scene_a_pass        = U32 4
    scene_a_max_jitter  = U32 5
    scene_a_ticks       = U32 6
    scene_a_misses      = U32 7
    scene_a_i2c_reads   = U32 8
    scene_b_pass        = U32 9
    scene_c_pass        = U32 10
    scene_c_max_latency = U32 11
    scene_c_samples     = U32 12
    scene_d_pass        = U32 13
    scene_d_pwm_hz      = U32 14
    scene_d_pin         = U32 15
    scene_d_flash_start = U32 16
    checksum            = U32 17
    report_address      = "0x$($addr.ToString('X8'))"
    evaluated_at        = (Get-Date).ToString("o")
}

$cs = [uint32]0
$cs = XorU32 $cs ([uint32]$report.magic)
$cs = XorU32 $cs ([uint32]$report.version)
$cs = XorU32 $cs ([uint32]$report.completed)
$cs = XorU32 $cs ([uint32]$report.all_pass)
$cs = XorU32 $cs ([uint32]$report.scene_a_pass)
$cs = XorU32 $cs ([uint32]$report.scene_a_max_jitter)
$cs = XorU32 $cs ([uint32]$report.scene_a_ticks)
$cs = XorU32 $cs ([uint32]$report.scene_a_misses)
$cs = XorU32 $cs ([uint32]$report.scene_a_i2c_reads)
$cs = XorU32 $cs ([uint32]$report.scene_b_pass)
$cs = XorU32 $cs ([uint32]$report.scene_c_pass)
$cs = XorU32 $cs ([uint32]$report.scene_c_max_latency)
$cs = XorU32 $cs ([uint32]$report.scene_c_samples)
$cs = XorU32 $cs ([uint32]$report.scene_d_pass)
$cs = XorU32 $cs ([uint32]$report.scene_d_pwm_hz)
$cs = XorU32 $cs ([uint32]$report.scene_d_pin)
$cs = XorU32 $cs ([uint32]$report.scene_d_flash_start)

$report.checksum_ok = ([uint32]$report.checksum -eq $cs)
$report.verdict = if ($report.completed -eq 1 -and $report.all_pass -eq 1 -and $report.checksum_ok) { "PASS" } else { "FAIL" }
$report.partial = ($report.magic -eq 0x41524E31 -and $report.completed -eq 0)

$report | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8 $reportJson

Write-Host ""
Write-Host "=== AIRON Phase 1 Eval: $($report.verdict) ==="
Write-Host "  scene A (PWM+I2C jitter): pass=$($report.scene_a_pass) jitter=$($report.scene_a_max_jitter)us ticks=$($report.scene_a_ticks)"
Write-Host "  scene B (TWIM0 lease):    pass=$($report.scene_b_pass)"
Write-Host "  scene C (radio PPI):      pass=$($report.scene_c_pass) max_lat=$($report.scene_c_max_latency)us samples=$($report.scene_c_samples)"
Write-Host "  scene D (Arduino parity): pass=$($report.scene_d_pass) pwm=$($report.scene_d_pwm_hz)Hz pin=$($report.scene_d_pin) flash=$($report.scene_d_flash_start)"
Write-Host "  report: $reportJson"

if ($report.verdict -ne "PASS") { exit 1 }
