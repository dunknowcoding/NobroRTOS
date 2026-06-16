# AIRON workspace cleanup — run after module validation or before commit.
param(
    [switch]$Deep
)

$work = $PSScriptRoot

Write-Host "Cleaning AIRON _work at $work"

$cargoTarget = Join-Path $work "cargo-target"
if (Test-Path $cargoTarget) {
    if ($Deep) {
        Remove-Item -Recurse -Force $cargoTarget
        Write-Host "Removed cargo-target (deep)"
    } else {
        Get-ChildItem $cargoTarget -Recurse -Directory -Filter "deps" -ErrorAction SilentlyContinue |
            ForEach-Object { Remove-Item -Recurse -Force $_.FullName -ErrorAction SilentlyContinue }
        Get-ChildItem $cargoTarget -Recurse -Directory -Filter "incremental" -ErrorAction SilentlyContinue |
            ForEach-Object { Remove-Item -Recurse -Force $_.FullName -ErrorAction SilentlyContinue }
        Write-Host "Trimmed cargo-target deps/incremental"
    }
}

$logs = Join-Path $work "logs"
if (Test-Path $logs) {
    Get-ChildItem $logs -File -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-7) } |
        Remove-Item -Force
}

$artifacts = Join-Path $work "artifacts"
if ($Deep -and (Test-Path $artifacts)) {
    Remove-Item -Recurse -Force $artifacts
}

Write-Host "Done."
