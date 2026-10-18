# Prints the state of the Windows NFS client for the Cua Volume mount:
# services, client settings, mounts, drives, who listens on the portmapper
# and NFS ports, and the NFS client event logs. Read-only; never fails.
# Used by the volume-windows lane of .github/workflows/ci-cua-spacesd.yml
# and by hand in a guest (elevated PowerShell).
$ErrorActionPreference = "Continue"
function Section($name) { Write-Output ""; Write-Output "== $name" }
Section "services"
Get-Service -Name NfsRdr, NfsClnt, WebClient, LanmanWorkstation -ErrorAction SilentlyContinue |
  Format-Table Name, Status, StartType -AutoSize | Out-String -Width 200
Section "features"
if (Get-Command Get-WindowsFeature -ErrorAction SilentlyContinue) {
  Get-WindowsFeature -Name NFS-Client | Format-Table Name, InstallState -AutoSize | Out-String
}
Section "nfsadmin client"
& nfsadmin.exe client 2>&1 | ForEach-Object { "$_" }
Section "registry"
Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\ClientForNFS\CurrentVersion\Default" -ErrorAction SilentlyContinue |
  Format-List | Out-String -Width 200
Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System" -Name EnableLinkedConnections -ErrorAction SilentlyContinue |
  Format-List EnableLinkedConnections | Out-String
Section "mount"
& mount.exe 2>&1 | ForEach-Object { "$_" }
Section "drives"
Get-PSDrive -PSProvider FileSystem | Format-Table Name, Root, DisplayRoot -AutoSize | Out-String -Width 200
& net.exe use 2>&1 | ForEach-Object { "$_" }
Section "listening on 111 and 2049"
Get-NetTCPConnection -LocalPort 111, 2049 -ErrorAction SilentlyContinue |
  Format-Table LocalAddress, LocalPort, RemoteAddress, RemotePort, State, OwningProcess -AutoSize | Out-String
Get-NetUDPEndpoint -LocalPort 111, 2049 -ErrorAction SilentlyContinue |
  Format-Table LocalAddress, LocalPort, OwningProcess -AutoSize | Out-String
& netstat.exe -ano 2>&1 | Select-String ":111 |:2049 " | ForEach-Object { "$_" }
Section "network providers"
(Get-ItemProperty "HKLM:\SYSTEM\CurrentControlSet\Control\NetworkProvider\Order" -ErrorAction SilentlyContinue).ProviderOrder
Section "NFS event logs"
$logs = Get-WinEvent -ListLog "*NFS*" -ErrorAction SilentlyContinue | Where-Object { $_.RecordCount -gt 0 }
foreach ($log in $logs) {
  "-- $($log.LogName)"
  Get-WinEvent -LogName $log.LogName -MaxEvents 40 -ErrorAction SilentlyContinue |
    Format-Table TimeCreated, Id, LevelDisplayName, Message -Wrap | Out-String -Width 300
}
exit 0
