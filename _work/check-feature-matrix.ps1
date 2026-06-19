# Software-only AIRON feature matrix checks. This does not flash or probe hardware.
param(
    [switch]$Clean
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repo = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$aion = Join-Path $repo "aion"
$target = Join-Path $PSScriptRoot "cargo-target"

$env:CARGO_TARGET_DIR = $target

$checks = @(
    @("check", "--workspace"),
    @("check", "-p", "sal-adapter-demo", "--no-default-features", "--features", "board-nicenano-s140"),
    @("check", "-p", "resource-sched-demo", "--no-default-features", "--features", "board-nicenano-s140"),
    @("check", "-p", "mvk-ppi-timestamp", "--no-default-features", "--features", "board-nicenano-s140"),
    @("check", "-p", "imu-i2c-demo", "--no-default-features", "--features", "board-nicenano-s140")
)

Push-Location $aion
try {
    foreach ($check in $checks) {
        Write-Host "cargo $($check -join ' ')"
        & cargo @check
        if ($LASTEXITCODE -ne 0) {
            throw "cargo $($check -join ' ') failed with exit code $LASTEXITCODE"
        }
    }
} finally {
    Pop-Location
}

if ($Clean -and (Test-Path -LiteralPath $target)) {
    $resolvedTarget = (Resolve-Path -LiteralPath $target).Path
    $resolvedWork = (Resolve-Path -LiteralPath $PSScriptRoot).Path
    if (-not $resolvedTarget.StartsWith($resolvedWork, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove outside _work: $resolvedTarget"
    }
    Remove-Item -LiteralPath $resolvedTarget -Recurse -Force
    Write-Host "Removed $resolvedTarget"
}
