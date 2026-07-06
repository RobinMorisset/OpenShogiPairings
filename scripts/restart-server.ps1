# Restart the API server and wait until it answers.
#
# The running `osp-server` does NOT hot-reload, so it must be restarted before
# browser-verifying any backend change. `cargo run` recompiles first if needed;
# this polls /api/health until the (possibly freshly compiled) server is up.
#
# Usage (from anywhere):  powershell -File scripts/restart-server.ps1

$repo = Split-Path -Parent $PSScriptRoot

Get-Process osp-server -ErrorAction SilentlyContinue | Stop-Process -Force
Set-Location $repo
Start-Process -FilePath "cargo" -ArgumentList "run", "-p", "osp-server" -WindowStyle Hidden

Write-Host "waiting for osp-server on :3000 ..."
for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Milliseconds 500
    try {
        Invoke-WebRequest "http://127.0.0.1:3000/api/health" -UseBasicParsing -TimeoutSec 2 | Out-Null
        Write-Host "server up on http://127.0.0.1:3000"
        exit 0
    } catch {}
}
Write-Host "server did not come up in time"
exit 1
