$ErrorActionPreference = 'Continue'
# Runs once, at the first logon of the unattended install (from the data disc
# beside Autounattend.xml). Installs cua-spacesd as a logon task in the desktop
# session, opens its port, and installs a disk guard. Placeholders are filled
# in by cua-image (windows::unattend).

$Version = '__VERSION__'
$Port = '__PORT__'
$Url = '__URL__'
$InstallDir = 'C:\Program Files\cua-spacesd'
$DataDir = 'C:\ProgramData\cua'
$TokenFile = Join-Path $DataDir 'env-token'
$Log = Join-Path $DataDir 'setup.log'

New-Item -ItemType Directory -Force -Path $InstallDir, $DataDir | Out-Null
Start-Transcript -Path $Log -Append | Out-Null

$zip = Join-Path $env:TEMP 'cua-spacesd.zip'
$ok = $false
for ($i = 1; $i -le 8 -and -not $ok; $i++) {
    try {
        Invoke-WebRequest -Uri $Url -OutFile $zip -UseBasicParsing
        Expand-Archive -Path $zip -DestinationPath $InstallDir -Force
        $ok = $true
    } catch { Start-Sleep -Seconds ($i * 5) }
}
$Exe = Join-Path $InstallDir 'cua-spacesd.exe'
if (-not (Test-Path $Exe)) { Write-Host "cua-spacesd download failed: $Url"; exit 1 }

if (!(Test-Path $TokenFile)) {
    $bytes = New-Object byte[] 32
    [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    Set-Content -Path $TokenFile -Value (($bytes | ForEach-Object { $_.ToString('x2') }) -join '') -NoNewline -Encoding ASCII
}

netsh advfirewall firewall add rule name="cua-spacesd $Port" dir=in action=allow protocol=TCP localport=$Port
# A program rule too: a port rule alone still raises the "allow public and private
# networks" prompt, which blocks the daemon until someone clicks it.
netsh advfirewall firewall add rule name="cua-spacesd program" dir=in action=allow program="$Exe" enable=yes profile=any

$StartScript = Join-Path $InstallDir 'start-spacesd.ps1'
@"
`$env:CUA_ENV_TOKEN = (Get-Content -Raw '$TokenFile').Trim()
while (`$true) {
    & '$Exe' --listen '0.0.0.0:$Port'
    Start-Sleep -Seconds 5
}
"@ | Set-Content -Path $StartScript -Encoding UTF8

$VbsWrapper = Join-Path $InstallDir 'start-spacesd-hidden.vbs'
@"
Set objShell = CreateObject("WScript.Shell")
objShell.Run "powershell.exe -NoProfile -ExecutionPolicy Bypass -File ""$StartScript""", 0, False
"@ | Set-Content -Path $VbsWrapper -Encoding ASCII

$UserId = "$env:COMPUTERNAME\$env:USERNAME"
$Action = New-ScheduledTaskAction -Execute 'wscript.exe' -Argument "`"$VbsWrapper`""
$Trigger = New-ScheduledTaskTrigger -AtLogOn -User $UserId
$Principal = New-ScheduledTaskPrincipal -UserId $UserId -LogonType Interactive -RunLevel Highest
$Settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -StartWhenAvailable -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
    -ExecutionTimeLimit (New-TimeSpan -Days 365) -Hidden
Unregister-ScheduledTask -TaskName 'Cua-Spacesd' -Confirm:$false -ErrorAction SilentlyContinue
Register-ScheduledTask -TaskName 'Cua-Spacesd' -Action $Action -Trigger $Trigger `
    -Principal $Principal -Settings $Settings -Force | Out-Null

# Disk guard: a full disk silently breaks the guest (registry saves, logs, the
# daemon itself). Every 5 minutes: below 3 GB free, trim old logs and write
# C:\ProgramData\cua\disk-low.txt, which cua-spacesd and `cua doctor` can read.
$GuardScript = Join-Path $InstallDir 'disk-guard.ps1'
@'
$free = (Get-PSDrive -Name C).Free
$flag = 'C:\ProgramData\cua\disk-low.txt'
if ($free -lt 3GB) {
    Get-ChildItem 'C:\ProgramData\cua','C:\Windows\Temp','C:\Windows\Logs\CBS' -Include *.log,*.etl,*.cab -Recurse -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTime -lt (Get-Date).AddHours(-1) } | Remove-Item -Force -ErrorAction SilentlyContinue
    ('{0} free: {1:N0} MB' -f (Get-Date -Format o), ((Get-PSDrive -Name C).Free / 1MB)) | Set-Content $flag
} elseif (Test-Path $flag) { Remove-Item $flag -Force }
'@ | Set-Content -Path $GuardScript -Encoding UTF8
$GuardAction = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$GuardScript`""
$GuardTrigger = New-ScheduledTaskTrigger -Once -At (Get-Date) -RepetitionInterval (New-TimeSpan -Minutes 5)
Unregister-ScheduledTask -TaskName 'Cua-DiskGuard' -Confirm:$false -ErrorAction SilentlyContinue
Register-ScheduledTask -TaskName 'Cua-DiskGuard' -Action $GuardAction -Trigger $GuardTrigger `
    -User 'SYSTEM' -RunLevel Highest -Force | Out-Null

Start-Process wscript.exe -ArgumentList "`"$VbsWrapper`""
Write-Host "cua-spacesd $Version setup complete"
Stop-Transcript | Out-Null
