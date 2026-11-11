# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

<#
.SYNOPSIS
Removes Cua Spaces Local (scripts\install-local.ps1) and only that.

.DESCRIPTION
  scripts\uninstall-local.ps1 [-Prefix DIR] [-Purge]

Removes the Local app folder (with its bundled `cua-local`), the Start Menu
shortcut, the user PATH entry and the cua-local:// scheme the installer added
(each only when it still points into the Local app), after stopping the
processes running from the Local folder. The sign-in and state stay
(%USERPROFILE%\.cua-local, %APPDATA%\Cua Spaces Local) unless -Purge is given.

It never touches the released Cua Spaces, its `cua`, %USERPROFILE%\.cua, its Run
entry or scheme, and refuses a folder at the Local path that is not the Local
one.
#>
[CmdletBinding()]
param(
    [string]$Prefix = '',
    [switch]$Purge
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $ScriptDir '_local-common.ps1')

function Stop-Uninstall([string]$Message) { Write-Err $Message; exit 1 }

if (-not $Prefix) { $Prefix = $env:LOCALAPPDATA }
if (-not [System.IO.Path]::IsPathRooted($Prefix)) { Stop-Uninstall "-Prefix must be an absolute path (got: $Prefix)" }
$InstallDir = Join-Path (Resolve-RealPath (Join-Path $Prefix 'Programs')) $Local.AppDirName
$BinDir = Join-Path $InstallDir 'bin'
$ExePath = Join-Path $InstallDir $Local.ExeName
$ShortcutPath = Join-Path $env:APPDATA ('Microsoft\Windows\Start Menu\Programs\' + $Local.ShortcutName)

$roots = Get-ReleasedRoots $env:LOCALAPPDATA $env:USERPROFILE $env:ProgramFiles
$hit = Find-ReleasedOverlap $InstallDir $roots
if ($hit) { Stop-Uninstall "refusing to remove ${InstallDir}: it is, holds or is inside $hit, which the released Cua Spaces owns." }

$state = Get-InstallState $InstallDir
if ($state -eq 'foreign') {
    Stop-Uninstall "refusing to touch ${InstallDir}: it is not a Cua Spaces Local install (no $($Local.MarkerName) with the Local app id)."
}

# Stop what runs from the Local folder (the app, its daemon): nothing else.
if ($state -ne 'absent') {
    Write-Step 'stopping Cua Spaces Local'
    $n = Stop-ProcessesUnder $InstallDir
    if ($n -gt 0) { Write-Host "  stopped $n process(es)" }
}

# The user PATH entry: only exactly this install's bin folder.
$userPath = Get-UserPath
if ($userPath -and (Test-PathListHas $userPath $BinDir)) {
    Set-UserPath (Remove-PathListEntry $userPath $BinDir)
    Write-Host "  removed $BinDir from the user PATH"
}

# The scheme, when it is still the Local app's.
$command = Get-LocalProtocolCommand
if ($command -and (Test-ContainsText $command $ExePath)) {
    Remove-Item -Path "HKCU:\Software\Classes\$($Local.UrlScheme)" -Recurse -Force
    Write-Host "  removed the $($Local.UrlScheme):// scheme"
} elseif ($command) {
    Write-Warn "left HKCU\Software\Classes\$($Local.UrlScheme): it does not point into $InstallDir"
}

# The Start Menu shortcut, when it points into the Local app.
if (Test-Path -LiteralPath $ShortcutPath) {
    $target = (New-Object -ComObject WScript.Shell).CreateShortcut($ShortcutPath).TargetPath
    if ($target -and (Test-PathInside $target $InstallDir)) {
        Remove-Item -LiteralPath $ShortcutPath -Force
        Write-Host '  removed the Start Menu shortcut'
    } else {
        Write-Warn "left ${ShortcutPath}: it does not point into $InstallDir"
    }
}

if ($state -ne 'absent') {
    Remove-Item -LiteralPath $InstallDir -Recurse -Force
    Write-Host "  removed $InstallDir"
}

if ($Purge) {
    $state2 = @(
        (Join-Path $env:APPDATA $Local.UserDataDirName),
        (Join-Path $env:USERPROFILE $Local.HomeDirName)
    )
    foreach ($d in $state2) {
        # The names are Local's; the released app's folders are never in this list.
        if ((Find-ReleasedOverlap $d $roots) -or -not (Test-Path -LiteralPath $d)) { continue }
        Remove-Item -LiteralPath $d -Recurse -Force
        Write-Host "  removed $d"
    }
} else {
    Write-Host "  kept your state: $(Join-Path $env:USERPROFILE $Local.HomeDirName) and $(Join-Path $env:APPDATA $Local.UserDataDirName) (-Purge removes them)"
}
Write-Ok 'Cua Spaces Local is uninstalled.'
