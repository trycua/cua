$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'
# Runs once, at the first logon of the unattended install (from the data disc
# beside Autounattend.xml). Installs cua-spacesd the way the published Windows
# image does (libs/images/windows-2022/install-spacesd.ps1: a scheduled task in
# the desktop session, bootstrap mode, token directory), opens its ports,
# installs a disk guard, seals the image (no token, no per-machine state) and
# powers the guest off. Placeholders are filled in by cua-image (windows::unattend).

$Version = '__VERSION__'
$Port = '__PORT__'
$Urls = @(__URLS__)
$InstallDir = Join-Path $env:ProgramFiles 'Cua\spacesd'
$DataDir = 'C:\ProgramData\cua'
$StateDir = Join-Path $env:ProgramData 'cua\spacesd'
$Log = Join-Path $DataDir 'setup.log'
$Exe = Join-Path $InstallDir 'cua-spacesd.exe'

New-Item -ItemType Directory -Force -Path $InstallDir, $DataDir, $StateDir | Out-Null
Start-Transcript -Path $Log -Append | Out-Null
"cua-spacesd $Version setup starting"

# The network adapter driver loads after the first boot: wait for a route.
for ($i = 0; $i -lt 60; $i++) {
    if (Test-Connection -ComputerName 10.0.2.2 -Count 1 -Quiet -ErrorAction SilentlyContinue) { break }
    Start-Sleep -Seconds 5
}

$ok = $false
foreach ($Url in $Urls) {
    for ($i = 1; $i -le 6 -and -not $ok; $i++) {
        try {
            Invoke-WebRequest -Uri $Url -OutFile $Exe -UseBasicParsing
            if ((Get-Item $Exe).Length -gt 1MB) { $ok = $true; "downloaded $Url" }
        } catch {
            "download of $Url failed: $($_.Exception.Message)"
            # A release without this build (404) is not worth retrying: next URL.
            if ($_.Exception.Response -and [int]$_.Exception.Response.StatusCode -eq 404) { break }
            Start-Sleep -Seconds ($i * 5)
        }
    }
    if ($ok) { break }
}
if (-not $ok) { "cua-spacesd download failed: $($Urls -join ', ')"; Stop-Transcript | Out-Null; exit 1 }
& $Exe --version

# Token directory: no inherited ACEs; SYSTEM, Administrators, the desktop user.
$User = "$env:USERDOMAIN\$env:USERNAME"
& icacls.exe $StateDir /inheritance:r /grant:r "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-544:(OI)(CI)F" "${User}:(OI)(CI)M" | Out-Null
$TokenFile = Join-Path $StateDir 'token'
if (Test-Path $TokenFile) { Remove-Item -Force $TokenFile }

# Machine PATH.
$path = [Environment]::GetEnvironmentVariable('Path', 'Machine')
if (($path -split ';') -notcontains $InstallDir) {
    [Environment]::SetEnvironmentVariable('Path', "$path;$InstallDir", 'Machine')
}

# Firewall: the port and the program (a port rule alone still raises the
# "allow public and private networks" prompt, which blocks the daemon).
foreach ($rule in @(@{ Name = 'cua-spacesd-tcp'; Protocol = 'TCP'; Port = $Port }, @{ Name = 'cua-spacesd-quic'; Protocol = 'UDP'; Port = ([int]$Port + 1) })) {
    Get-NetFirewallRule -Name $rule.Name -ErrorAction SilentlyContinue | Remove-NetFirewallRule
    New-NetFirewallRule -Name $rule.Name -DisplayName $rule.Name -Direction Inbound -Action Allow `
        -Protocol $rule.Protocol -LocalPort $rule.Port -Program $Exe -Profile Any | Out-Null
}

# Launcher: bootstrap mode binds every interface (QEMU's user network reaches
# the guest through its address, not loopback); the first authenticated Init
# from the host installs the token.
$Launcher = Join-Path $InstallDir 'start-spacesd.ps1'
@"
`$ErrorActionPreference = 'Stop'
foreach (`$name in @('CUA_DRIVER_POLICY_FILE', 'CUA_DRIVER_SESSION_POLICY_FILE')) { Remove-Item "Env:`$name" -ErrorAction SilentlyContinue }
& '$Exe' serve --listen 0.0.0.0:$Port --insecure-bootstrap --token-file '$TokenFile'
exit `$LASTEXITCODE
"@ | Set-Content -Path $Launcher -Encoding UTF8

$TaskName = 'cua-spacesd'
$arguments = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$Launcher`""
$Action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $arguments -WorkingDirectory $InstallDir
$Trigger = New-ScheduledTaskTrigger -AtLogOn -User $User
$Principal = New-ScheduledTaskPrincipal -UserId $User -LogonType Interactive -RunLevel Highest
$Settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
    -MultipleInstances IgnoreNew -StartWhenAvailable
Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
Register-ScheduledTask -TaskName $TaskName -Action $Action -Trigger $Trigger -Principal $Principal -Settings $Settings -Force | Out-Null

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

# Prove the daemon runs here, in this session and on every interface.
Start-ScheduledTask -TaskName $TaskName
$listen = $null
for ($i = 0; $i -lt 60 -and -not $listen; $i++) {
    Start-Sleep -Seconds 2
    $listen = Get-NetTCPConnection -State Listen -LocalPort ([int]$Port) -ErrorAction SilentlyContinue
}
if (-not $listen) { "cua-spacesd is not listening on $Port"; Stop-Transcript | Out-Null; exit 1 }
"cua-spacesd listening on $(($listen | ForEach-Object { "$($_.LocalAddress):$($_.LocalPort)" }) -join ', ')"

# Seal: every clone makes its own token and per-machine state; none may ship.
Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
Get-Process -Name 'cua-spacesd' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 3
Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $env:TEMP 'cua-build-*')
$profiles = @(Get-ChildItem -Directory -Force -ErrorAction SilentlyContinue "$env:SystemDrive\Users" | ForEach-Object { $_.FullName })
$profiles += Join-Path $env:SystemRoot 'System32\config\systemprofile'
foreach ($p in $profiles) {
    foreach ($name in @('spacesd', 'env-driver', 'guestd')) {
        $dir = Join-Path $p ".cua\$name"
        if (Test-Path -LiteralPath $dir) { Remove-Item -Recurse -Force -ErrorAction SilentlyContinue -LiteralPath $dir }
    }
}
Get-ChildItem -Force -ErrorAction SilentlyContinue -LiteralPath $StateDir | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
$left = @(Get-ChildItem -Force -ErrorAction SilentlyContinue -LiteralPath $StateDir)
$left += @(Get-ChildItem -Recurse -Force -ErrorAction SilentlyContinue -Filter 'machine-seal.key' -Path ($profiles + $env:ProgramData))
if ($left.Count -gt 0) { "STILL PRESENT after sealing:"; $left | ForEach-Object { $_.FullName }; Stop-Transcript | Out-Null; exit 1 }
"cua-spacesd $Version setup complete; sealed"
# The marker the host waits for before it stops the VM.
Set-Content -Path (Join-Path $DataDir 'sealed') -Value $Version
Stop-Transcript | Out-Null
& shutdown.exe /s /t 10 /f /d p:4:1 /c "cua image build"
