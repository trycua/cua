# Prints what the build needs to know about the guest (read-only): OS,
# sessions, auto-logon, how computer-server starts, firewall, disk, PATH.
# build-image.sh saves it as inventory.txt next to the build log.
$ErrorActionPreference = "Continue"
function Section($name) { Write-Output ""; Write-Output "== $name" }
Section "os"
Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, BuildNumber, OSArchitecture, TotalVisibleMemorySize | Format-List | Out-String -Width 200
Section "identity"
whoami; whoami /groups | Select-String -Pattern "Mandatory Label|Administrators"
Section "sessions"
query user 2>&1
Section "winlogon"
Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon" |
  Select-Object AutoAdminLogon, DefaultUserName, DefaultDomainName, AutoLogonCount | Format-List | Out-String
Section "computer-server processes"
Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -match "computer.server|computer_server|cua" } |
  Select-Object ProcessId, SessionId, Name, CommandLine | Format-List | Out-String -Width 400
Section "scheduled tasks (non-Microsoft)"
Get-ScheduledTask | Where-Object { $_.TaskPath -notlike "\Microsoft\*" } | ForEach-Object {
  "{0}{1} [{2}] user={3} logon={4} actions={5}" -f $_.TaskPath, $_.TaskName, $_.State, $_.Principal.UserId,
    $_.Principal.LogonType, (($_.Actions | ForEach-Object { "$($_.Execute) $($_.Arguments)" }) -join "; ")
}
Section "run keys and startup"
Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run" -ErrorAction SilentlyContinue | Format-List | Out-String -Width 300
Get-ItemProperty "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run" -ErrorAction SilentlyContinue | Format-List | Out-String -Width 300
Get-ChildItem "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\StartUp", "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Startup" -ErrorAction SilentlyContinue | Select-Object FullName
Section "services (auto, non-Microsoft-ish)"
Get-CimInstance Win32_Service | Where-Object { $_.StartMode -eq "Auto" -and $_.PathName -notmatch "\\Windows\\" } |
  Select-Object Name, State, StartName, PathName | Format-Table -AutoSize | Out-String -Width 300
Section "listening"
Get-NetTCPConnection -State Listen | Sort-Object LocalPort | Select-Object LocalAddress, LocalPort, OwningProcess | Format-Table | Out-String
Section "firewall"
Get-NetFirewallProfile | Select-Object Name, Enabled | Format-Table | Out-String
Section "disk"
Get-Volume | Format-Table | Out-String
Get-Partition | Format-Table | Out-String
Section "path"
[Environment]::GetEnvironmentVariable("Path", "Machine")
Section "tools"
foreach ($t in "python", "cua-driver", "cua-spacesd", "sshd", "curl.exe", "tar.exe") { "{0}: {1}" -f $t, ((Get-Command $t -ErrorAction SilentlyContinue).Source) }
Get-ChildItem "C:\Program Files", "C:\Program Files (x86)", "C:\ProgramData", "C:\" -ErrorAction SilentlyContinue | Select-Object FullName | Out-String -Width 200
Section "display"
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.Screen]::AllScreens | ForEach-Object { "$($_.DeviceName) $($_.Bounds)" }
Section "defender"
Get-MpComputerStatus -ErrorAction SilentlyContinue | Select-Object RealTimeProtectionEnabled, AntivirusEnabled | Format-List | Out-String
exit 0
