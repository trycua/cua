# Verify that the runner is inside an interactive user session, not Session 0.
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$sessionId = (Get-Process -Id $PID).SessionId
if ($sessionId -eq 0) {
    throw "The test runner is in Session 0. Start it from the active console/RDP user session."
}

$sessionName = if ($env:SESSIONNAME) { $env:SESSIONNAME } else { "unknown" }
Write-Host "Interactive Windows session verified: id=$sessionId name=$sessionName" -ForegroundColor Green
