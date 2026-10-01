# End-to-end `cua-driver update --apply` on Windows (#2803, #4388).
#
# Installs the previous published release with its autostart task, starts
# that release's daemons, and runs its own `cua-driver update --apply`. The
# shipped updater starts
#   powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "iwr -useb https://cua.ai/driver/install.ps1 | iex"
# and waits for it. Windows resolves powershell.exe from the updater's own
# directory first, so a stand-in placed there runs this checkout's
# install.ps1 instead of the copy published from main. Release selection,
# the process tree, the inherited environment, and exit-code propagation
# all belong to the shipped updater.
#
# Needs an elevated interactive session and GH_TOKEN (release lookup). Run it
# only on a disposable machine: it replaces the cua-driver-serve task, stops
# every cua-driver process, and edits the User PATH (restored afterward).

[CmdletBinding()]
param(
    [string]$EvidenceDir = [System.IO.Path]::GetTempPath()
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param(
        [Parameter(Mandatory = $true)][bool]$Condition,
        [Parameter(Mandatory = $true)][string]$Message
    )
    if (-not $Condition) { throw $Message }
}

function Invoke-Native {
    param([string]$FilePath, [string[]]$Arguments)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $output = & $FilePath @Arguments 2>&1 | Out-String
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }
    return [pscustomobject]@{ ExitCode = $exitCode; Output = $output }
}

$scriptsDir = Split-Path -Parent $PSScriptRoot
$repoRoot = (Resolve-Path -LiteralPath (Join-Path $scriptsDir '..\..\..')).Path
$installer = Join-Path $scriptsDir 'install.ps1'
$windowsPowerShell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
$schtasks = Join-Path $env:SystemRoot 'System32\schtasks.exe'
$taskName = 'cua-driver-serve'
$oneLiner = 'iwr -useb https://cua.ai/driver/install.ps1 | iex'

# Update from the newest published, non-withdrawn release below the newest.
$withdrawn = @(Get-Content -LiteralPath (Join-Path $repoRoot '.github/release-state/cua-driver-rs-withdrawn-versions') |
    ForEach-Object { ($_ -replace '#.*', '').Trim() } |
    Where-Object { $_ })
$tags = Invoke-Native 'gh' @('api', '--paginate', 'repos/trycua/cua/releases?per_page=100', '--jq', '.[] | select(.draft == false) | .tag_name')
Assert-True ($tags.ExitCode -eq 0) "could not list releases: $($tags.Output)"
$versions = @($tags.Output -split "`r?`n" |
    Where-Object { $_ -match '^cua-driver-rs-v\d+\.\d+\.\d+$' } |
    ForEach-Object { [version]($_ -replace '^cua-driver-rs-v', '') } |
    Where-Object { $withdrawn -notcontains $_.ToString() } |
    Sort-Object -Descending -Unique)
Assert-True ($versions.Count -ge 2) 'need two published releases to test an update'
$from = $versions[1].ToString()
Write-Host "Updating from cua-driver $from"

$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('cua-driver-update-apply-' + [guid]::NewGuid().ToString('N'))
$savedEnv = @{}
foreach ($name in @('USERPROFILE', 'LOCALAPPDATA', 'CUA_DRIVER_RS_VERSION', 'CUA_DRIVER_RS_HOME',
        'CUA_DRIVER_RS_INSTALL_DIR', 'CUA_DRIVER_RS_TELEMETRY_ENABLED', 'CUA_TELEMETRY_ENABLED',
        'CUA_E2E_CANDIDATE_INSTALLER', 'CUA_E2E_UPDATER_ARGS')) {
    $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name)
}
$savedUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')

try {
    $env:USERPROFILE = Join-Path $testRoot 'profile'
    $env:LOCALAPPDATA = Join-Path $testRoot 'localappdata'
    New-Item -ItemType Directory -Force -Path $env:USERPROFILE, $env:LOCALAPPDATA | Out-Null
    foreach ($name in @('CUA_DRIVER_RS_VERSION', 'CUA_DRIVER_RS_HOME', 'CUA_DRIVER_RS_INSTALL_DIR')) {
        [Environment]::SetEnvironmentVariable($name, $null)
    }
    $env:CUA_DRIVER_RS_TELEMETRY_ENABLED = 'false'
    $env:CUA_TELEMETRY_ENABLED = 'false'

    # 1. Install the previous release with its autostart task.
    $install = Invoke-Native $windowsPowerShell @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $installer,
        '-Release', $from, '-NoPathUpdate')
    $install.Output | Set-Content -LiteralPath (Join-Path $EvidenceDir 'cua-driver-update-from-install.txt')
    Assert-True ($install.ExitCode -eq 0) "installing $from failed with exit $($install.ExitCode): $($install.Output)"
    $binDir = Join-Path $env:LOCALAPPDATA 'Programs\Cua\cua-driver\bin'
    $bin = Join-Path $binDir 'cua-driver.exe'
    $before = Invoke-Native $bin @('--version')
    Assert-True ($before.Output -match [regex]::Escape($from)) "installed binary is not $($from): $($before.Output)"
    Assert-True ((Invoke-Native $schtasks @('/Query', '/TN', $taskName)).ExitCode -eq 0) "installing $from did not register $taskName"

    # 2. Old daemons for the update to replace: the autostart task's daemon
    #    and a manually started one that no task owns.
    Assert-True ((Invoke-Native $schtasks @('/Run', '/TN', $taskName)).ExitCode -eq 0) "could not start $taskName"
    $socket = "\\.\pipe\cua-driver-update-e2e-$PID"
    Start-Process -FilePath $bin -ArgumentList @('serve', '--socket', $socket) -WindowStyle Hidden | Out-Null
    $oldDaemons = @()
    for ($attempt = 0; $attempt -lt 60 -and $oldDaemons.Count -lt 2; $attempt++) {
        Start-Sleep -Milliseconds 500
        $oldDaemons = @(Get-CimInstance -ClassName Win32_Process -Filter "Name='cua-driver.exe'" |
            Where-Object { $_.CommandLine -match '\sserve(\s|$)' } |
            ForEach-Object { Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue })
    }
    Assert-True ($oldDaemons.Count -ge 2) "expected two $from daemons, found $($oldDaemons.Count)"

    # 3. Stand-in powershell.exe in the updater's directory. It records the
    #    updater's arguments and runs this checkout's installer.
    $standInSource = @'
using System;
using System.Diagnostics;
using System.IO;

public static class PowerShellStandIn {
    public static int Main(string[] args) {
        File.WriteAllLines(Environment.GetEnvironmentVariable("CUA_E2E_UPDATER_ARGS"), args);
        string powershell = Path.Combine(Environment.SystemDirectory, @"WindowsPowerShell\v1.0\powershell.exe");
        string installer = Environment.GetEnvironmentVariable("CUA_E2E_CANDIDATE_INSTALLER");
        ProcessStartInfo start = new ProcessStartInfo(powershell,
            "-NoProfile -ExecutionPolicy Bypass -File \"" + installer + "\"");
        start.UseShellExecute = false;
        using (Process child = Process.Start(start)) {
            child.WaitForExit();
            return child.ExitCode;
        }
    }
}
'@
    $standInPath = Join-Path $testRoot 'PowerShellStandIn.cs'
    Set-Content -LiteralPath $standInPath -Value $standInSource -Encoding Ascii
    $csc = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
    $compile = Invoke-Native $csc @('/nologo', '/target:exe', "/out:$(Join-Path $binDir 'powershell.exe')", $standInPath)
    Assert-True ($compile.ExitCode -eq 0) "could not build the powershell.exe stand-in: $($compile.Output)"
    $env:CUA_E2E_CANDIDATE_INSTALLER = $installer
    $env:CUA_E2E_UPDATER_ARGS = Join-Path $testRoot 'updater-args.txt'

    # 4. Run the shipped updater. Its release lookup is an unauthenticated
    #    GitHub API call, so retry only that failure, before anything installs.
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        $update = Invoke-Native $bin @('update', '--apply')
        if ($update.ExitCode -eq 0 -or $update.Output -notmatch 'Could not reach GitHub') { break }
        Start-Sleep -Seconds 30
    }
    $update.Output | Set-Content -LiteralPath (Join-Path $EvidenceDir 'cua-driver-update-apply.txt')
    Write-Host $update.Output
    Assert-True ($update.ExitCode -eq 0) "cua-driver $from update --apply exited $($update.ExitCode)"

    $updaterArgs = @(Get-Content -LiteralPath $env:CUA_E2E_UPDATER_ARGS)
    Assert-True ($updaterArgs -contains $oneLiner) "the updater did not run the published installer one-liner: $($updaterArgs -join ' ')"
    Assert-True ($update.Output -match 'New version available: (\S+)') 'the updater did not select a newer release'
    $to = $Matches[1]
    foreach ($expected in @(
            "cua-driver-rs $to installed.",
            'Stopping any previous cua-driver processes',
            'Registering auto-start (cua-driver autostart enable)',
            "Auto-start: 'cua-driver-serve' is registered at RunLevel=Highest.",
            "Installed cua-driver $to.")) {
        Assert-True ($update.Output.Contains($expected)) "update --apply output is missing: $expected"
    }
    $after = Invoke-Native $bin @('--version')
    Assert-True ($after.Output -match [regex]::Escape($to)) "the updated binary is not $($to): $($after.Output)"
    $action = (Get-ScheduledTask -TaskName $taskName -ErrorAction Stop).Actions | Select-Object -First 1
    Assert-True ($action.Arguments -like "*$bin*") "$taskName does not launch $($bin): $($action.Arguments)"
    foreach ($daemon in $oldDaemons) {
        $daemon.Refresh()
        Assert-True $daemon.HasExited "the $from daemon (pid $($daemon.Id)) is still running"
    }

    Write-Host "cua-driver $from update --apply installed $to, re-registered $taskName, and stopped the old daemons."
}
finally {
    Invoke-Native $schtasks @('/End', '/TN', $taskName) | Out-Null
    Invoke-Native $schtasks @('/Delete', '/TN', $taskName, '/F') | Out-Null
    Get-Process -Name 'cua-driver', 'cua-driver-uia' -ErrorAction SilentlyContinue |
        Stop-Process -Force -ErrorAction SilentlyContinue
    [Environment]::SetEnvironmentVariable('Path', $savedUserPath, 'User')
    foreach ($name in $savedEnv.Keys) {
        [Environment]::SetEnvironmentVariable($name, $savedEnv[$name])
    }
}
