# Real-process regression for the installer's shared daemon cleanup
# (_install-common.psm1; #2803, #4388).
#
# `cua-driver update --apply` launches the installer from cua-driver.exe and
# waits for its exit code. Like the shipped CLI, which runs finite commands in
# a wrapped cua-driver.exe child, the stand-in updater is two cua-driver.exe
# processes deep. It runs a Windows PowerShell "installer" that calls
# Stop-CuaDriverDaemonsWithHealth, next to an unrelated cua-driver.exe daemon
# with a child process and a cua-driver-uia.exe worker. Cleanup must stop the
# daemon, its child, and the worker, report no survivors, and leave the
# updater and installer running so the updater exits 0.
#
# The cleanup stops every cua-driver process on the machine and ends the
# cua-driver-serve task. Run this only on a disposable machine such as CI.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-True {
    param(
        [Parameter(Mandatory = $true)][bool]$Condition,
        [Parameter(Mandatory = $true)][string]$Message
    )
    if (-not $Condition) { throw $Message }
}

function Start-StandIn {
    param([string]$Exe, [string]$Arguments = '')
    $start = New-Object System.Diagnostics.ProcessStartInfo($Exe, $Arguments)
    $start.UseShellExecute = $false
    return [System.Diagnostics.Process]::Start($start)
}

$module = Join-Path (Split-Path -Parent $PSScriptRoot) '_install-common.psm1'
$root = Join-Path ([System.IO.Path]::GetTempPath()) ('cua-driver-cleanup-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null

# One stand-in program, four modes:
#   (no arguments)  idle, like a daemon
#   spawn <exe>     start <exe>, then idle (a daemon with a child)
#   run <script>    run <script> in Windows PowerShell, wait, and return its
#                   exit code, as `cua-driver update --apply` does
#   wrap <args>     run itself with <args>, wait, and return its exit code,
#                   as the CLI's telemetry wrapper does
$source = @'
using System;
using System.Diagnostics;
using System.IO;
using System.Threading;

public static class StandIn {
    static int RunAndWait(string file, string arguments) {
        ProcessStartInfo start = new ProcessStartInfo(file, arguments);
        start.UseShellExecute = false;
        using (Process child = Process.Start(start)) {
            child.WaitForExit();
            return child.ExitCode;
        }
    }

    public static int Main(string[] args) {
        if (args.Length > 1 && args[0] == "wrap") {
            string[] quoted = new string[args.Length - 1];
            for (int i = 1; i < args.Length; i++) quoted[i - 1] = "\"" + args[i] + "\"";
            return RunAndWait(Process.GetCurrentProcess().MainModule.FileName, string.Join(" ", quoted));
        }
        if (args.Length == 2 && args[0] == "run") {
            string powershell = Path.Combine(Environment.SystemDirectory, @"WindowsPowerShell\v1.0\powershell.exe");
            return RunAndWait(powershell,
                "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"" + args[1] + "\"");
        }
        if (args.Length == 2 && args[0] == "spawn") {
            ProcessStartInfo start = new ProcessStartInfo(args[1]);
            start.UseShellExecute = false;
            Process.Start(start);
        }
        Thread.Sleep(Timeout.Infinite);
        return 0;
    }
}
'@

$daemon = $null
$worker = $null
$updater = $null
try {
    $sourcePath = Join-Path $root 'StandIn.cs'
    Set-Content -LiteralPath $sourcePath -Value $source -Encoding Ascii
    $driverExe = Join-Path $root 'cua-driver.exe'
    $csc = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
    if (-not (Test-Path -LiteralPath $csc)) {
        $csc = Join-Path $env:WINDIR 'Microsoft.NET\Framework\v4.0.30319\csc.exe'
    }
    & $csc /nologo /target:exe "/out:$driverExe" $sourcePath
    if ($LASTEXITCODE -ne 0) { throw "csc failed with exit $LASTEXITCODE" }
    $uiaExe = Join-Path $root 'cua-driver-uia.exe'
    $childExe = Join-Path $root 'daemon-child.exe'
    Copy-Item -LiteralPath $driverExe -Destination $uiaExe
    Copy-Item -LiteralPath $driverExe -Destination $childExe

    $resultPath = Join-Path $root 'cleanup-result.json'
    $installerPath = Join-Path $root 'installer.ps1'
    $installer = @"
Set-StrictMode -Version Latest
`$ErrorActionPreference = 'Stop'
Import-Module -Name '$($module.Replace("'", "''"))' -Force
`$result = Stop-CuaDriverDaemonsWithHealth
@{
    Survivors = @(`$result.Survivors | Where-Object { `$_ } | ForEach-Object { `$_.Id })
    Stale = [bool]`$result.Stale
} | ConvertTo-Json | Set-Content -LiteralPath '$($resultPath.Replace("'", "''"))'
"@
    Set-Content -LiteralPath $installerPath -Value $installer -Encoding UTF8

    $daemon = Start-StandIn $driverExe "spawn `"$childExe`""
    $worker = Start-StandIn $uiaExe
    $daemonChild = $null
    for ($attempt = 0; $attempt -lt 100 -and -not $daemonChild; $attempt++) {
        $daemonChild = Get-CimInstance -ClassName Win32_Process `
            -Filter "ParentProcessId=$($daemon.Id) AND Name='daemon-child.exe'" `
            -ErrorAction SilentlyContinue
        if (-not $daemonChild) { Start-Sleep -Milliseconds 100 }
    }
    Assert-True ($null -ne $daemonChild) 'daemon stand-in did not start its child'
    $daemonChildId = [int]$daemonChild.ProcessId

    $updater = Start-StandIn $driverExe "wrap run `"$installerPath`""
    Assert-True ($updater.WaitForExit(120000)) 'updater stand-in did not exit within 120 seconds'

    Assert-True ($updater.ExitCode -eq 0) `
        "updater stand-in exited $($updater.ExitCode): cleanup stopped the process tree that launched the installer"
    Assert-True (Test-Path -LiteralPath $resultPath) 'installer stand-in did not finish after cleanup'
    $result = Get-Content -LiteralPath $resultPath -Raw | ConvertFrom-Json
    Assert-True (@($result.Survivors).Count -eq 0) "cleanup reported survivors: $(@($result.Survivors) -join ', ')"
    Assert-True (-not $result.Stale) 'cleanup reported a stale daemon'
    Assert-True ($daemon.WaitForExit(5000)) 'unrelated cua-driver daemon was not stopped'
    Assert-True ($worker.WaitForExit(5000)) 'cua-driver-uia worker was not stopped'
    Assert-True ($null -eq (Get-Process -Id $daemonChildId -ErrorAction SilentlyContinue)) `
        'tree kill did not stop the daemon child process'

    Write-Host 'Windows installer cleanup spared its launcher and stopped the other daemons.'
}
finally {
    foreach ($process in @($daemon, $worker, $updater)) {
        if ($process -and -not $process.HasExited) {
            try { $process.Kill() } catch { }
        }
    }
    Get-Process -Name 'daemon-child' -ErrorAction SilentlyContinue |
        Stop-Process -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
