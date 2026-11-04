# Prepares the guest for the Cua Volume mount (run elevated by the
# volume-windows lane of .github/workflows/ci-cua-spacesd.yml on a hosted
# Windows Server 2022 runner; the image build runs it once the Windows mount
# ships, with cua-spacesd's CUA_VOLUME_WINDOWS_PREVIEW turned into the default).
#
# cua-spacesd mounts the volume with Windows' own NFS client (Client for NFS)
# on the drive V: ("Cua Volume"): no third-party driver. This:
#   - installs Client for NFS (Windows Server: the NFS-Client feature; client
#     Windows: ServicesForNFS-ClientOnly), which brings mount.exe/umount.exe;
#   - starts the client now (a fresh install may not run it until a boot)
#     and prefers TCP;
#   - sets EnableLinkedConnections, so the drive cua-spacesd maps (it runs
#     elevated) also shows in the user's unelevated Explorer. Takes effect at
#     the next boot.
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

if (Get-Command Install-WindowsFeature -ErrorAction SilentlyContinue) {
  $r = Install-WindowsFeature -Name NFS-Client
  Write-Output "NFS-Client: success=$($r.Success) restart_needed=$($r.RestartNeeded)"
  if (-not $r.Success) { throw "Install-WindowsFeature NFS-Client failed" }
} else {
  Enable-WindowsOptionalFeature -Online -NoRestart -All `
    -FeatureName ServicesForNFS-ClientOnly, ClientForNFS-Infrastructure | Out-Null
  Write-Output "ServicesForNFS-ClientOnly enabled"
}
$system32 = Join-Path $env:SystemRoot "System32"
foreach ($exe in "mount.exe", "umount.exe") {
  if (-not (Test-Path (Join-Path $system32 $exe))) { throw "Client for NFS did not install $exe" }
}

# The client service may not run until the next boot on a fresh install:
# start it now. Then TCP only (a setting the client reads when it starts, so
# restart it). The UDP path works too (cua-spacesd bridges it); TCP is the
# faster one.
$ErrorActionPreference = "Continue"
foreach ($svc in "NfsRdr", "NfsClnt") {
  $s = Get-Service -Name $svc -ErrorAction SilentlyContinue
  if ($s) {
    Set-Service -Name $svc -StartupType Automatic -ErrorAction SilentlyContinue
    if ($s.Status -ne "Running") { Start-Service -Name $svc -ErrorAction SilentlyContinue }
  }
}
$nfsadmin = Join-Path $system32 "nfsadmin.exe"
if (Test-Path $nfsadmin) {
  & $nfsadmin client config protocol=TCP 2>&1 | ForEach-Object { "nfsadmin: $_" }
  if ($LASTEXITCODE -ne 0) { Write-Output "nfsadmin client config protocol=TCP exited $LASTEXITCODE (the client keeps TCP+UDP)" }
  & $nfsadmin client stop 2>&1 | ForEach-Object { "nfsadmin: $_" }
  & $nfsadmin client start 2>&1 | ForEach-Object { "nfsadmin: $_" }
}
Get-Service -Name NfsRdr, NfsClnt -ErrorAction SilentlyContinue |
  ForEach-Object { "service $($_.Name): $($_.Status) ($($_.StartType))" }
$ErrorActionPreference = "Stop"

$policy = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System"
New-ItemProperty -Path $policy -Name EnableLinkedConnections -Value 1 -PropertyType DWord -Force | Out-Null
Write-Output "EnableLinkedConnections=1"

# The portmapper cua-spacesd starts for the mount needs 127.0.0.1:111.
$busy = Get-NetTCPConnection -State Listen -LocalPort 111 -ErrorAction SilentlyContinue
if ($busy) { Write-Output "warning: TCP 111 is in use by process $($busy.OwningProcess -join ', ')" }
Write-Output "Client for NFS ready"
