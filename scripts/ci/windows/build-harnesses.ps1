# Build all repo-local Windows harness apps from source.
param(
    [ValidateSet("none", "wpf", "winui3", "webview", "electron", "tauri")]
    [string]$Skip = "none",
    [ValidateSet("wpf", "winui3", "webview", "electron", "tauri")]
    [string[]]$Targets = @("wpf", "winui3", "webview", "electron", "tauri")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$repoRoot = (Resolve-Path (Join-Path $scriptDir "..\..\..")).Path
$fixtureBuild = Join-Path $repoRoot "libs\cua-driver\tests\fixtures\build\windows.ps1"

& $fixtureBuild -Skip $Skip -Targets $Targets
if ($LASTEXITCODE -ne 0) {
    throw "Windows harness build failed with exit code $LASTEXITCODE"
}
