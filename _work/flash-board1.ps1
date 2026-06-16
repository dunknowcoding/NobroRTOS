# Flash AIRON MVK to board1 (ProMicro no-SD @ 0x1000). Does NOT reflash bootloader.
param(
    [string]$Hex = "F:\Arduino\driver\AIRON\_work\artifacts\mvk_ppi_timestamp.hex",
    [uint32]$Base = 0x1000
)

$ErrorActionPreference = "Stop"
$jlink = "C:\Program Files\SEGGER\JLink_V944\JLink.exe"
if (-not (Test-Path $jlink)) { throw "JLink not found at $jlink" }
if (-not (Test-Path $Hex)) { throw "HEX not found: $Hex — run cargo objcopy first" }

$script = Join-Path $env:TEMP "airon_flash_board1.jlink"
@"
device NRF52840_XXAA
si 1
speed 4000
connect
r
h
loadfile $Hex $Base
r
g
exit
"@ | Set-Content -Encoding ascii $script

Write-Host "Flashing $Hex -> 0x$($Base.ToString('X')) via J-Link (board1)..."
& $jlink -NoGui 1 -CommanderScript $script
Write-Host "Done. Use J-Link RTT Viewer or probe-rs attach for defmt logs."
