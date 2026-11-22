<#
Cheap permanent diagnostics for the Windows E2E lanes.

  -Action Start    record the job start time and launch a background sampler
  -Action Stop     stop the sampler (its CSV stays in the artifact directory)
  -Action Failure  dump System event log entries since job start and the top
                   processes by working set, and print a short summary

The sampler writes one CSV row every 10 s: memory commit, available memory,
page file usage and the foreground window's title, PID and process name.
Everything lands in artifacts\cua-driver\windows so the lane's existing
artifact upload carries it. Diagnostics never fail the job.
#>
param(
    [Parameter(Mandatory = $true)][ValidateSet("Start", "Stop", "Failure", "Sample")][string]$Action,
    [string]$OutputDirectory = "artifacts\cua-driver\windows",
    [int]$IntervalSeconds = 10
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Continue"
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$repoRoot = (Resolve-Path (Join-Path $scriptDir "..\..\..")).Path
$out = Join-Path $repoRoot $OutputDirectory
$stateDir = Join-Path $env:RUNNER_TEMP "cua-e2e-diagnostics"
$startFile = Join-Path $stateDir "start-time.txt"
$pidFile = Join-Path $stateDir "sampler.pid"
$csv = Join-Path $out "e2e-sampler.csv"

function Get-ForegroundInfo {
    if (-not ("CuaDiag.Fg" -as [type])) {
        Add-Type -Namespace CuaDiag -Name Fg -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr h, System.Text.StringBuilder s, int n);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint pid);
'@
    }
    $h = [CuaDiag.Fg]::GetForegroundWindow()
    if ($h -eq [IntPtr]::Zero) { return @("", 0, "") }
    $sb = New-Object System.Text.StringBuilder 256
    [void][CuaDiag.Fg]::GetWindowText($h, $sb, 256)
    $fgPid = [uint32]0
    [void][CuaDiag.Fg]::GetWindowThreadProcessId($h, [ref]$fgPid)
    $name = (Get-Process -Id $fgPid -ErrorAction SilentlyContinue).ProcessName
    return @($sb.ToString(), $fgPid, $name)
}

# Read-only page file state. TempPageFile is the documented Win32_PageFileUsage
# property mapped to HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\
# Memory Management\TempPageFile: true means Windows created a temporary page
# file because of a configuration problem at boot, which is what the
# "Windows created a temporary paging file" notice reports.
function Get-PageFileState {
    $mm = "HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management"
    $reg = Get-ItemProperty $mm -ErrorAction SilentlyContinue
    $usage = @(Get-CimInstance Win32_PageFileUsage -ErrorAction SilentlyContinue |
        ForEach-Object { "{0} alloc={1}MB temp={2}" -f $_.Name, $_.AllocatedBaseSize, $_.TempPageFile })
    $cs = Get-CimInstance Win32_ComputerSystem -ErrorAction SilentlyContinue
    # The registry value exists only after Windows created a temporary page
    # file, and strict mode throws on a missing property, so read it only when
    # present. Win32_PageFileUsage.TempPageFile (usage=[...]) stays the source
    # of truth.
    $regTemp = if ($reg -and $reg.PSObject.Properties['TempPageFile']) { $reg.TempPageFile } else { '<absent>' }
    $regPaging = if ($reg -and $reg.PSObject.Properties['PagingFiles']) { $reg.PagingFiles -join ';' } else { '' }
    "PagingFiles=[$regPaging] TempPageFile=$regTemp " +
        "AutomaticManagedPagefile=$($cs.AutomaticManagedPagefile) usage=[$($usage -join '; ')]"
}

function Write-Sample {
    $c = Get-Counter -Counter '\Memory\Committed Bytes', '\Memory\Commit Limit', '\Memory\Available MBytes' -ErrorAction SilentlyContinue
    $v = @{}
    foreach ($s in $c.CounterSamples) { $v[($s.Path -split '\\')[-1]] = $s.CookedValue }
    $pf = @(Get-CimInstance Win32_PageFileUsage -ErrorAction SilentlyContinue)
    $pfCurrent = ($pf | Measure-Object -Property CurrentUsage -Sum).Sum
    $pfAlloc = ($pf | Measure-Object -Property AllocatedBaseSize -Sum).Sum
    $fg = Get-ForegroundInfo
    [pscustomobject]@{
        time_utc           = (Get-Date).ToUniversalTime().ToString("o")
        committed_mb       = [math]::Round($v['committed bytes'] / 1MB)
        commit_limit_mb    = [math]::Round($v['commit limit'] / 1MB)
        available_mb       = $v['available mbytes']
        pagefile_used_mb   = $pfCurrent
        pagefile_alloc_mb  = $pfAlloc
        fg_title           = $fg[0]
        fg_pid             = $fg[1]
        fg_process         = $fg[2]
    } | Export-Csv -Path $csv -Append -NoTypeInformation -Encoding utf8
}

switch ($Action) {
    "Start" {
        New-Item -ItemType Directory -Force $out, $stateDir | Out-Null
        (Get-Date).ToUniversalTime().ToString("o") | Set-Content $startFile
        Write-Host "Page file state at job start: $(Get-PageFileState)"
        $proc = Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList @(
            "-NoProfile", "-File", $MyInvocation.MyCommand.Definition,
            "-Action", "Sample", "-OutputDirectory", $OutputDirectory, "-IntervalSeconds", $IntervalSeconds
        )
        $proc.Id | Set-Content $pidFile
        Write-Host "Diagnostics sampler started (pid $($proc.Id)), writing $csv"
    }
    "Sample" {
        New-Item -ItemType Directory -Force $out | Out-Null
        while ($true) {
            try { Write-Sample } catch { }
            Start-Sleep -Seconds $IntervalSeconds
        }
    }
    "Stop" {
        if (Test-Path $pidFile) {
            Stop-Process -Id ([int](Get-Content $pidFile)) -Force -ErrorAction SilentlyContinue
        }
        if (Test-Path $csv) { Write-Host "Sampler rows: $(@(Import-Csv $csv).Count)" }
    }
    "Failure" {
        New-Item -ItemType Directory -Force $out | Out-Null
        $since = if (Test-Path $startFile) { [datetime]::Parse((Get-Content $startFile)).ToUniversalTime() } else { (Get-Date).AddHours(-2) }
        $events = @(Get-WinEvent -FilterHashtable @{ LogName = "System"; StartTime = $since.ToLocalTime() } -ErrorAction SilentlyContinue |
            Select-Object TimeCreated, Id, ProviderName, LevelDisplayName, Message)
        $events | Export-Csv (Join-Path $out "system-events.csv") -NoTypeInformation -Encoding utf8
        $procs = Get-Process | Sort-Object WorkingSet64 -Descending | Select-Object -First 15 Id, ProcessName,
            @{ n = "working_set_mb"; e = { [math]::Round($_.WorkingSet64 / 1MB) } },
            @{ n = "private_mb"; e = { [math]::Round($_.PrivateMemorySize64 / 1MB) } }
        $procs | Export-Csv (Join-Path $out "top-processes.csv") -NoTypeInformation -Encoding utf8
        Write-Host "::group::E2E failure diagnostics"
        Write-Host "Page file state at failure: $(Get-PageFileState)"
        Write-Host "System events since $($since.ToString('o')): $($events.Count)"
        $events | Where-Object { $_.ProviderName -match "Resource-Exhaustion|Wininit|MMC|Memory|Kernel-Power|Ntfs|disk" -or $_.LevelDisplayName -in "Error", "Warning" } |
            Select-Object -First 25 | ForEach-Object {
                $m = ($_.Message -replace "\s+", " ")
                if ($m.Length -gt 200) { $m = $m.Substring(0, 200) }
                Write-Host ("{0:o} id={1} {2}: {3}" -f $_.TimeCreated, $_.Id, $_.ProviderName, $m)
            }
        Write-Host "Top processes by working set:"
        $procs | Format-Table -AutoSize | Out-String | Write-Host
        if (Test-Path $csv) {
            Write-Host "Last sampler rows:"
            Import-Csv $csv | Select-Object -Last 5 | Format-Table -AutoSize | Out-String | Write-Host
        }
        Write-Host "::endgroup::"
    }
}
exit 0
