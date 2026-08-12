# Run the full test + type-check loop.
#
# Handles the one environment quirk (Node isn't on this shell's PATH, so the
# frontend type-check needs it prepended).
#
# Usage (from anywhere):  powershell -File scripts/check.ps1

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot

Write-Host "== cargo test ==" -ForegroundColor Cyan
Set-Location $repo
cargo test

Write-Host "== svelte-check ==" -ForegroundColor Cyan
$env:Path = "C:\Program Files\nodejs;" + $env:Path
Set-Location (Join-Path $repo "frontend")
npm run check

Write-Host "== eslint ==" -ForegroundColor Cyan
npm run lint
