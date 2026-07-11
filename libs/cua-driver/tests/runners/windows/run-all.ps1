# Canonical Windows Rust harness runner.
#
# Run from libs/cua-driver in an interactive RDP/console session:
#   .\tests\runners\windows\run-all.ps1
#   .\tests\runners\windows\run-all.ps1 -RequireGui

param(
    [switch]$NoBuild,
    [switch]$RequireGui
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..\..\..\..")
$canonicalRunner = Join-Path $repoRoot "scripts\ci\windows\run-rust-e2e.ps1"

if (-not (Test-Path $canonicalRunner)) {
    throw "Canonical Windows E2E runner not found: $canonicalRunner"
}

& $canonicalRunner -Suite all -NoBuild:$NoBuild -RequireGui:$RequireGui
exit $LASTEXITCODE
