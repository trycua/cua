param(
    [string]$OutputDirectory = "artifacts\cua-driver\windows"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$repoRoot = (Resolve-Path (Join-Path $scriptDir "..\..\..")).Path
$destination = Join-Path $repoRoot $OutputDirectory
New-Item -ItemType Directory -Force $destination | Out-Null

$rustRoot = Join-Path $repoRoot "libs\cua-driver\rust"
$target = Join-Path $rustRoot "target"
if (Test-Path $target) {
    Get-ChildItem -Path $target -Filter "*.log" -Recurse -ErrorAction SilentlyContinue |
        Copy-Item -Destination $destination -Force
}

Write-Host "Collected Windows driver artifacts in $destination" -ForegroundColor Green
