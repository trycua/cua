# Remove only the source-built cua-driver-local product on Windows.
[CmdletBinding()]
param([switch]$Force, [switch]$ValidateOnly)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$TaskName = "cua-driver-local-serve"
$HomeDir = if ($env:CUA_DRIVER_LOCAL_HOME) { $env:CUA_DRIVER_LOCAL_HOME } else { Join-Path $env:USERPROFILE ".cua-driver-local" }
$VisibleBinDir = if ($env:CUA_DRIVER_LOCAL_INSTALL_DIR) { $env:CUA_DRIVER_LOCAL_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\Cua\cua-driver-local\bin" }
$RuntimeDir = Join-Path $env:LOCALAPPDATA "cua-driver-local"
$ReleaseHome = Join-Path $env:USERPROFILE ".cua-driver"
$ReleaseBinDir = Join-Path $env:LOCALAPPDATA "Programs\Cua\cua-driver\bin"
if (-not [IO.Path]::IsPathRooted($HomeDir) -or $HomeDir -eq $env:USERPROFILE -or $HomeDir -eq $ReleaseHome) {
    throw "refusing unsafe or release-owned local home: $HomeDir"
}
if (-not [IO.Path]::IsPathRooted($VisibleBinDir) -or $VisibleBinDir -eq $ReleaseBinDir) {
    throw "refusing unsafe or release-owned local bin path: $VisibleBinDir"
}

function Write-Step([string]$Message) { Write-Host "==> $Message" }
function Test-IsReparsePoint([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    $item = Get-Item -LiteralPath $Path -Force
    return [bool]($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)
}
function Test-LocalLinkTarget([string]$Path) {
    if (-not (Test-IsReparsePoint $Path)) { return $false }
    $item = Get-Item -LiteralPath $Path -Force
    $targetProperty = $item.PSObject.Properties['Target']
    if (-not $targetProperty) { return $false }
    $targets = @($targetProperty.Value)
    foreach ($target in $targets) {
        $prefix = $HomeDir.TrimEnd('\') + '\'
        if ($target -and ($target -eq $HomeDir -or $target.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase))) { return $true }
    }
    return $false
}

if ($ValidateOnly) {
    Write-Output "cli=$(Join-Path $VisibleBinDir 'cua-driver-local.exe')"
    Write-Output "home=$HomeDir"
    Write-Output "runtime=$RuntimeDir"
    Write-Output "task=$TaskName"
    Write-Output "processes=cua-driver-local,cua-driver-uia-local"
    exit 0
}

if (-not $Force) {
    $reply = Read-Host "Remove the local cua-driver identity and its state? [y/N]"
    if ($reply -notmatch '^(y|yes)$') { Write-Step "cancelled"; exit 0 }
}

# The local task may be Highest IL. Fail safely with an actionable message
# instead of falling through into a partial removal.
$previousErrorAction = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
try {
    & schtasks.exe /Query /TN $TaskName 2>$null | Out-Null
    $taskExists = ($LASTEXITCODE -eq 0)
} finally {
    $ErrorActionPreference = $previousErrorAction
}
if ($taskExists) {
    $ErrorActionPreference = 'Continue'
    try {
        & schtasks.exe /End /TN $TaskName 2>$null | Out-Null
        & schtasks.exe /Delete /TN $TaskName /F 2>$null | Out-Null
        $deleteExitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorAction
    }
    if ($deleteExitCode -ne 0) { throw "could not remove $TaskName; rerun from an elevated PowerShell" }
    Write-Step "removed scheduled task $TaskName"
}

Get-Process -Name "cua-driver-local","cua-driver-uia-local" -ErrorAction SilentlyContinue |
    Stop-Process -Force -ErrorAction SilentlyContinue

# The visible bin path is local-only, but still require the junction shape the
# installer creates before removing it.
$LocalBinOwned = $false
if (Test-Path -LiteralPath $VisibleBinDir) {
    if (-not (Test-LocalLinkTarget $VisibleBinDir)) {
        throw "$VisibleBinDir does not point into the local package home; refusing to remove it"
    }
    $LocalBinOwned = $true
    Remove-Item -LiteralPath $VisibleBinDir -Force -Recurse
    Write-Step "removed local bin junction $VisibleBinDir"
}

# Shared skill names are removed only when the reparse target belongs to the
# local package home. Release and hand-managed links are preserved.
$SkillLinks = @(
    (Join-Path $env:USERPROFILE ".claude\skills\cua-driver"),
    (Join-Path $env:USERPROFILE ".agents\skills\cua-driver"),
    (Join-Path $env:USERPROFILE ".openclaw\skills\cua-driver"),
    (Join-Path $env:APPDATA "opencode\skills\cua-driver"),
    (Join-Path $env:USERPROFILE ".gemini\skills\cua-driver"),
    (Join-Path $env:USERPROFILE ".hermes\skills\cua-driver")
)
foreach ($link in $SkillLinks) {
    if (Test-LocalLinkTarget $link) {
        Remove-Item -LiteralPath $link -Force -Recurse
        Write-Step "removed local skill link $link"
    }
}

# Remove Claude MCP entries only when command/args point at the local command
# or package home. A same-named release registration is retained.
$ClaudeJson = Join-Path $env:USERPROFILE ".claude.json"
if (Test-Path -LiteralPath $ClaudeJson) {
    $data = Get-Content -LiteralPath $ClaudeJson -Raw | ConvertFrom-Json
    $changed = $false
    function Remove-LocalServers($servers) {
        if (-not $servers) { return }
        foreach ($property in @($servers.PSObject.Properties)) {
            $server = $property.Value
            $commandProperty = $server.PSObject.Properties['command']
            $argsProperty = $server.PSObject.Properties['args']
            $parts = @()
            if ($commandProperty) { $parts += @($commandProperty.Value) }
            if ($argsProperty) { $parts += @($argsProperty.Value) }
            $joined = ($parts | Where-Object { $_ -is [string] }) -join ' '
            if ($joined.Contains("cua-driver-local") -or $joined.Contains($HomeDir)) {
                $servers.PSObject.Properties.Remove($property.Name)
                $script:changed = $true
            }
        }
    }
    $mcpServersProperty = $data.PSObject.Properties['mcpServers']
    if ($mcpServersProperty) { Remove-LocalServers $mcpServersProperty.Value }
    $projectsProperty = $data.PSObject.Properties['projects']
    if ($projectsProperty) {
        foreach ($project in $projectsProperty.Value.PSObject.Properties) {
            $projectServers = $project.Value.PSObject.Properties['mcpServers']
            if ($projectServers) { Remove-LocalServers $projectServers.Value }
        }
    }
    if ($changed) {
        $backup = "$ClaudeJson.bak-cua-driver-local-uninstall-$(Get-Date -Format yyyyMMddHHmmss)"
        Copy-Item -LiteralPath $ClaudeJson -Destination $backup
        $data | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $ClaudeJson -Encoding UTF8
        Write-Step "removed local Claude MCP registrations (backup: $backup)"
    }
}

# CUA_DRIVER_LOCAL_HOME is caller-controlled. Require the installer-created
# executable marker and remove only known runtime-owned children; never
# recursively delete the arbitrary override root or unrelated files within it.
$LocalHomeMarker = Join-Path $HomeDir "packages\current\cua-driver-local.exe"
if (Test-Path -LiteralPath $LocalHomeMarker) {
    foreach ($child in @("packages", "skills")) {
        $path = Join-Path $HomeDir $child
        if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force -Recurse }
    }
    foreach ($file in @(
        ".installation_recorded", ".telemetry_enabled", ".telemetry_id",
        ".telemetry_install_channel", ".tcc-signing-identity", "config.json",
        "serve.err.log", "serve.out.log", "version_check.json"
    )) {
        $path = Join-Path $HomeDir $file
        if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
    }
    if (@(Get-ChildItem -LiteralPath $HomeDir -Force).Count -eq 0) {
        Remove-Item -LiteralPath $HomeDir -Force
        Write-Step "removed empty local home $HomeDir"
    } else {
        Write-Step "removed local runtime payloads from $HomeDir; preserved unrelated files"
    }
} elseif (Test-Path -LiteralPath $HomeDir) {
    Write-Step "$HomeDir has no cua-driver-local install marker; leaving it untouched"
}
if (Test-Path -LiteralPath $RuntimeDir) {
    Remove-Item -LiteralPath $RuntimeDir -Force -Recurse
    Write-Step "removed $RuntimeDir"
}

# Remove only the exact local PATH entry.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -and $LocalBinOwned) {
    $filtered = @(($userPath -split ';') | Where-Object { $_ -and ($_.TrimEnd('\') -ne $VisibleBinDir.TrimEnd('\')) })
    [Environment]::SetEnvironmentVariable('Path', ($filtered -join ';'), 'User')
}

Write-Step "cua-driver-local uninstalled; release cua-driver was left untouched"
