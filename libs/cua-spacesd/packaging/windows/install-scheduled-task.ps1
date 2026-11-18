# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Registers cua-spacesd as a scheduled task in the interactive session of
# the current user (desktop capture and UI Automation need that session;
# a Windows service would run in session 0). Restarts on failure.
#
#   powershell -ExecutionPolicy Bypass -File install-scheduled-task.ps1 `
#     -Exe "C:\Program Files\cua\cua-spacesd.exe"
param(
  [Parameter(Mandatory = $true)][string]$Exe,
  [string]$TaskName = "cua-spacesd",
  [string]$TokenFile = "$env:USERPROFILE\.cua\spacesd\token"
)
$ErrorActionPreference = "Stop"
if (-not (Test-Path $Exe)) { throw "cua-spacesd not found at $Exe" }
$action = New-ScheduledTaskAction -Execute $Exe -Argument "serve --token-file `"$TokenFile`""
$trigger = New-ScheduledTaskTrigger -AtLogOn -User "$env:USERDOMAIN\$env:USERNAME"
$principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
  -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
  -MultipleInstances IgnoreNew
# Tasks under the older names (cua-guestd, cua-env-driver) are replaced by
# this one.
foreach ($old in @("cua-guestd", "cua-env-driver")) {
  if (Get-ScheduledTask -TaskName $old -ErrorAction SilentlyContinue) {
    Stop-ScheduledTask -TaskName $old -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $old -Confirm:$false
  }
}
Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Principal $principal `
  -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName $TaskName
Write-Host "Registered and started scheduled task '$TaskName' ($Exe)."
