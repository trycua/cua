# Installs cua-spacesd into the Windows image (run by build-image.sh in the
# build guest, as the auto-logon desktop user, elevated).
#
#   install-spacesd.ps1 -Fetch http://10.0.2.2:PORT
#
# Layout (the Windows equivalents of the Linux image's paths):
#   C:\Program Files\Cua\spacesd\cua-spacesd.exe   on the machine PATH
#   C:\ProgramData\cua-image\manifest.json         the image's claims
#   C:\ProgramData\cua-image\fixtures\             WinForms conformance
#       fixtures (grid, form) the doctor launches
#   C:\ProgramData\cua\spacesd\                    token directory: SYSTEM,
#       Administrators and the desktop user only (no inheritance; the
#       Windows form of root 0700 + owner 0600). The first Init writes
#       `token` here (bootstrap mode) and a restart reads it back.
#
# cua-spacesd runs as the scheduled task `cua-spacesd` in the desktop user's
# interactive session (capture, UI Automation and input need that session; a
# service would run in session 0), highest run level, restarted on failure.
# It starts in bootstrap mode: until the first authenticated Init installs a
# token, only GetCapabilities, Health and Init answer. start-spacesd.ps1
# binds 0.0.0.0:3211 explicitly: without a token the default bind is loopback
# only, which the host (QEMU user networking, KubeVirt) cannot reach. It also
# keeps the base's Cua Driver user policy off cua-spacesd. Inbound TCP 3211 (gRPC,
# gRPC-Web, /mcp, /viewer) and UDP 3212 (QUIC media) are allowed.
param(
  [Parameter(Mandatory = $true)][string]$Fetch,
  [string]$TaskName = "cua-spacesd"
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$installDir = Join-Path $env:ProgramFiles "Cua\spacesd"
$exe = Join-Path $installDir "cua-spacesd.exe"
$imageDir = Join-Path $env:ProgramData "cua-image"
$stateDir = Join-Path $env:ProgramData "cua\spacesd"
$tokenFile = Join-Path $stateDir "token"
$user = "$env:USERDOMAIN\$env:USERNAME"

Write-Output "installing as $user"
New-Item -ItemType Directory -Force -Path $installDir, $imageDir, $stateDir | Out-Null
Invoke-WebRequest -UseBasicParsing "$Fetch/cua-spacesd.exe" -OutFile $exe
Invoke-WebRequest -UseBasicParsing "$Fetch/start-spacesd.ps1" -OutFile (Join-Path $installDir "start-spacesd.ps1")
Invoke-WebRequest -UseBasicParsing "$Fetch/manifest.json" -OutFile (Join-Path $imageDir "manifest.json")
$fixtures = Join-Path $imageDir "fixtures"
New-Item -ItemType Directory -Force -Path $fixtures | Out-Null
foreach ($f in @("fixturelog.ps1", "grid.ps1", "form.ps1")) {
  Invoke-WebRequest -UseBasicParsing "$Fetch/fixtures/$f" -OutFile (Join-Path $fixtures $f)
}
& $exe build-info
if ($LASTEXITCODE -ne 0) { throw "cua-spacesd build-info failed ($LASTEXITCODE)" }

# Cua Volume is not mounted in Windows guests yet: the image leaves Client
# for NFS off, so cua-spacesd reports volume.mount unsupported and nothing
# attaches it (enable-volume.ps1 is what will turn it on).

# Machine PATH.
$path = [Environment]::GetEnvironmentVariable("Path", "Machine")
if (($path -split ";") -notcontains $installDir) {
  [Environment]::SetEnvironmentVariable("Path", "$path;$installDir", "Machine")
}
$env:Path = "$env:Path;$installDir"

# Token directory: no inherited ACEs; SYSTEM, Administrators, the desktop user.
& icacls.exe $stateDir /inheritance:r /grant:r "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-544:(OI)(CI)F" "${user}:(OI)(CI)M" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "icacls $stateDir failed" }
if (Test-Path $tokenFile) { Remove-Item -Force $tokenFile }

# Firewall.
foreach ($rule in @(
    @{ Name = "cua-spacesd-tcp"; Protocol = "TCP"; Port = 3211 },
    @{ Name = "cua-spacesd-quic"; Protocol = "UDP"; Port = 3212 })) {
  Get-NetFirewallRule -Name $rule.Name -ErrorAction SilentlyContinue | Remove-NetFirewallRule
  New-NetFirewallRule -Name $rule.Name -DisplayName $rule.Name -Direction Inbound -Action Allow `
    -Protocol $rule.Protocol -LocalPort $rule.Port -Program $exe -Profile Any | Out-Null
}

# Cua Driver policy variables the base sets (start-spacesd.ps1 clears the
# user and session layers for cua-spacesd).
foreach ($scope in "User", "Machine") {
  foreach ($name in "CUA_DRIVER_POLICY_FILE", "CUA_DRIVER_MANAGED_POLICY_FILE", "CUA_DRIVER_SESSION_POLICY_FILE") {
    $value = [Environment]::GetEnvironmentVariable($name, $scope)
    if ($value) { Write-Output "base sets $scope $name=$value" }
  }
}

# Scheduled task in the interactive session, through start-spacesd.ps1 (no
# console window; binds 0.0.0.0:3211 in bootstrap mode).
$launcher = Join-Path $installDir "start-spacesd.ps1"
$arguments = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$launcher`""
$action = New-ScheduledTaskAction -Execute "powershell.exe" -Argument $arguments -WorkingDirectory $installDir
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
  -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
  -MultipleInstances IgnoreNew -StartWhenAvailable
foreach ($old in @("cua-guestd", "cua-env-driver")) {
  if (Get-ScheduledTask -TaskName $old -ErrorAction SilentlyContinue) {
    Stop-ScheduledTask -TaskName $old -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $old -Confirm:$false
  }
}
Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Principal $principal `
  -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName $TaskName

$deadline = (Get-Date).AddSeconds(90)
while ((Get-Date) -lt $deadline) {
  $listen = Get-NetTCPConnection -State Listen -LocalPort 3211 -ErrorAction SilentlyContinue
  if ($listen) { break }
  Start-Sleep -Seconds 2
}
if ($listen -and -not ($listen | Where-Object { $_.LocalAddress -in @("0.0.0.0", "::") })) {
  throw "cua-spacesd listens on loopback only ($(($listen | ForEach-Object { $_.LocalAddress }) -join ', '))"
}
if (-not $listen) {
  Get-ScheduledTaskInfo -TaskName $TaskName | Format-List | Out-String | Write-Output
  throw "cua-spacesd is not listening on 3211"
}
$proc = Get-CimInstance Win32_Process -Filter "Name='cua-spacesd.exe'"
$proc | Select-Object ProcessId, SessionId, CommandLine | Format-List | Out-String -Width 300 | Write-Output
if ($proc | Where-Object { $_.SessionId -eq 0 }) { throw "cua-spacesd runs in session 0" }
Write-Output "cua-spacesd listening on $(($listen | ForEach-Object { "$($_.LocalAddress):$($_.LocalPort)" }) -join ', ')"
