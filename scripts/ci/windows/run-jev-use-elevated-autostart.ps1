# Run the jev-use deterministic proof on the default Windows user install:
# install.ps1 registers the cua-driver-serve autostart task at
# RunLevel=Highest, so on an administrator account the resident daemon is
# elevated. MCP clients such as the jev-use runners spawn `cua-driver mcp`,
# which on Windows hosts its own in-process runtime at the client's token; on
# this administrator runner that runtime is elevated too. Either elevated
# Driver must launch its isolated browser with a derived standard-user token
# (#4234). This script checks that posture from outside the Driver before
# trusting the proof result:
#
#   - the autostart task is registered at RunLevel=Highest and every
#     `cua-driver serve` daemon it starts runs at High (or higher) integrity;
#   - the process that launched each isolated browser is an elevated
#     cua-driver.exe (read while the browser is alive); and
#   - every isolated browser process runs at Medium (or lower) integrity,
#     below its launching Driver, with Administrators not enabled.
#
# Inputs (environment):
#   CUA_DRIVER_BIN      installed cua-driver.exe
#   JEV_USE_PROOF_DIR   new evidence directory for verify_setup.py
#   JEV_USE_LOG_DIR     existing directory for daemon and token diagnostics
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

foreach ($name in @("CUA_DRIVER_BIN", "JEV_USE_PROOF_DIR", "JEV_USE_LOG_DIR")) {
    if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($name))) {
        throw "$name is required"
    }
}
$driver = $env:CUA_DRIVER_BIN
$proofDir = $env:JEV_USE_PROOF_DIR
$logDir = $env:JEV_USE_LOG_DIR
if (-not (Test-Path -LiteralPath $driver -PathType Leaf)) {
    throw "installed Driver not found: $driver"
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$exampleDir = Join-Path $repoRoot "libs\cua-driver\examples\jev-use"
$python = Join-Path $exampleDir ".venv\Scripts\python.exe"
if (-not (Test-Path -LiteralPath $python -PathType Leaf)) {
    throw "locked example environment not found: $python"
}

Add-Type -TypeDefinition @"
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Principal;

public sealed class CuaTokenPosture {
    public int Pid;
    public bool Elevated;
    public int ElevationType;
    public bool AdministratorsEnabled;
    public uint IntegrityRid;

    const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
    const uint TOKEN_QUERY = 0x0008;
    const int TokenGroups = 2;
    const int TokenElevationType = 18;
    const int TokenElevation = 20;
    const int TokenIntegrityLevel = 25;
    const uint SE_GROUP_ENABLED = 0x4;
    const uint SE_GROUP_USE_FOR_DENY_ONLY = 0x10;

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CloseHandle(IntPtr handle);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true)]
    static extern bool GetTokenInformation(IntPtr token, int cls, IntPtr info, int length, out int needed);
    [DllImport("advapi32.dll")]
    static extern IntPtr GetSidSubAuthorityCount(IntPtr sid);
    [DllImport("advapi32.dll")]
    static extern IntPtr GetSidSubAuthority(IntPtr sid, uint index);

    static IntPtr Information(IntPtr token, int cls) {
        int needed;
        GetTokenInformation(token, cls, IntPtr.Zero, 0, out needed);
        if (needed <= 0) {
            throw new Win32Exception(Marshal.GetLastWin32Error(), "GetTokenInformation size " + cls);
        }
        IntPtr buffer = Marshal.AllocHGlobal(needed);
        if (!GetTokenInformation(token, cls, buffer, needed, out needed)) {
            int error = Marshal.GetLastWin32Error();
            Marshal.FreeHGlobal(buffer);
            throw new Win32Exception(error, "GetTokenInformation " + cls);
        }
        return buffer;
    }

    public static CuaTokenPosture Read(int pid) {
        IntPtr process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if (process == IntPtr.Zero) {
            throw new Win32Exception(Marshal.GetLastWin32Error(), "OpenProcess " + pid);
        }
        IntPtr token;
        try {
            if (!OpenProcessToken(process, TOKEN_QUERY, out token)) {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "OpenProcessToken " + pid);
            }
        } finally {
            CloseHandle(process);
        }
        var posture = new CuaTokenPosture { Pid = pid };
        try {
            IntPtr buffer = Information(token, TokenElevation);
            try { posture.Elevated = Marshal.ReadInt32(buffer) != 0; } finally { Marshal.FreeHGlobal(buffer); }

            buffer = Information(token, TokenElevationType);
            try { posture.ElevationType = Marshal.ReadInt32(buffer); } finally { Marshal.FreeHGlobal(buffer); }

            buffer = Information(token, TokenIntegrityLevel);
            try {
                IntPtr sid = Marshal.ReadIntPtr(buffer);
                byte count = Marshal.ReadByte(GetSidSubAuthorityCount(sid));
                posture.IntegrityRid = (uint)Marshal.ReadInt32(GetSidSubAuthority(sid, (uint)(count - 1)));
            } finally { Marshal.FreeHGlobal(buffer); }

            buffer = Information(token, TokenGroups);
            try {
                int groupCount = Marshal.ReadInt32(buffer);
                int entrySize = IntPtr.Size * 2;
                for (int index = 0; index < groupCount; index++) {
                    IntPtr entry = IntPtr.Add(buffer, IntPtr.Size + index * entrySize);
                    var sid = new SecurityIdentifier(Marshal.ReadIntPtr(entry));
                    uint attributes = (uint)Marshal.ReadInt32(IntPtr.Add(entry, IntPtr.Size));
                    if (sid.Value == "S-1-5-32-544"
                        && (attributes & SE_GROUP_USE_FOR_DENY_ONLY) == 0
                        && (attributes & SE_GROUP_ENABLED) != 0) {
                        posture.AdministratorsEnabled = true;
                    }
                }
            } finally { Marshal.FreeHGlobal(buffer); }
        } finally {
            CloseHandle(token);
        }
        return posture;
    }

    public override string ToString() {
        return String.Format(
            "pid={0} integrity=0x{1:x4} elevated={2} elevation_type={3} administrators_enabled={4}",
            Pid, IntegrityRid, Elevated, ElevationType, AdministratorsEnabled);
    }
}
"@

$HighIntegrity = [uint32]0x3000
$MediumIntegrity = [uint32]0x2000
$postureLog = Join-Path $logDir "token-posture.txt"
function Write-Posture([string]$line) {
    Write-Host $line
    Add-Content -Encoding utf8 -LiteralPath $postureLog -Value $line
}

# 1. The installer registered the default autostart task at RunLevel=Highest.
#    install.ps1 reports a registration failure without failing the install,
#    so assert the task itself.
$task = Get-ScheduledTask -TaskName "cua-driver-serve" -ErrorAction SilentlyContinue
if ($null -eq $task) { throw "install.ps1 did not register the cua-driver-serve autostart task" }
Write-Posture "[task] cua-driver-serve run_level=$($task.Principal.RunLevel) logon_type=$($task.Principal.LogonType) user=$($task.Principal.UserId)"
if ([string]$task.Principal.RunLevel -ne "Highest") {
    throw "the autostart task must run at RunLevel=Highest, got $($task.Principal.RunLevel)"
}

$sampler = $null
$state = [hashtable]::Synchronized(@{ Stop = $false; Browsers = [System.Collections.ArrayList]::Synchronized([System.Collections.ArrayList]::new()) })
try {
    Write-Host "[driver] $(& $driver --version)"
    # 2. Start the daemon exactly as the installer's hint tells users to
    #    without logging on again, then wait until it answers.
    & $driver autostart kick
    if ($LASTEXITCODE -ne 0) { throw "cua-driver autostart kick failed with exit code $LASTEXITCODE" }
    $deadline = (Get-Date).AddSeconds(90)
    while ($true) {
        & $driver status *> (Join-Path $logDir "daemon-status.txt")
        if ($LASTEXITCODE -eq 0) { break }
        if ((Get-Date) -gt $deadline) {
            Get-Content (Join-Path $logDir "daemon-status.txt") -ErrorAction SilentlyContinue
            throw "the autostart daemon did not become ready within 90 seconds"
        }
        Start-Sleep -Milliseconds 500
    }
    Get-Content (Join-Path $logDir "daemon-status.txt")
    & $driver autostart status *>> (Join-Path $logDir "daemon-status.txt")

    # 3. The daemon is elevated: this is the default posture on an
    #    administrator account, and the reason this job exists.
    $daemons = @(Get-CimInstance Win32_Process -Filter "Name='cua-driver.exe'" |
        Where-Object { $_.CommandLine -match '\sserve(\s|$)' })
    if ($daemons.Count -eq 0) { throw "no cua-driver serve daemon is running after autostart kick" }
    $daemonPostures = @{}
    foreach ($daemon in $daemons) {
        $posture = [CuaTokenPosture]::Read([int]$daemon.ProcessId)
        $daemonPostures[[int]$daemon.ProcessId] = $posture
        Write-Posture "[daemon] $posture session=$($daemon.SessionId) command=$($daemon.CommandLine)"
        if ($posture.IntegrityRid -lt $HighIntegrity) {
            throw "the autostart daemon must be elevated on this administrator account: $posture"
        }
    }

    # 4. Sample every isolated browser process while the proof runs; the
    #    Driver closes its isolated browser when each agent's session ends.
    $sampler = Start-ThreadJob -ArgumentList $state -ScriptBlock {
        param($state)
        $seen = @{}
        while (-not $state.Stop) {
            foreach ($process in @(Get-CimInstance Win32_Process -Filter "Name='chrome.exe' OR Name='msedge.exe'" -ErrorAction SilentlyContinue)) {
                $processId = [int]$process.ProcessId
                if ($seen.ContainsKey($processId) -or [string]::IsNullOrEmpty($process.CommandLine)) { continue }
                if ($process.CommandLine -notmatch '--user-data-dir') { continue }
                $seen[$processId] = $true
                $record = [ordered]@{
                    Pid = $processId
                    ParentPid = [int]$process.ParentProcessId
                    Main = ($process.CommandLine -notmatch '\s--type=')
                    Name = $process.Name
                    Parent = $null
                    ParentName = $null
                    ParentPosture = $null
                    Posture = $null
                    Error = $null
                }
                try { $record.Posture = [CuaTokenPosture]::Read($processId) } catch { $record.Error = $_.Exception.Message }
                if ($record.Main) {
                    $parent = Get-CimInstance Win32_Process -Filter "ProcessId=$($record.ParentPid)" -ErrorAction SilentlyContinue
                    $record.Parent = if ($parent) { "$($parent.Name) $($parent.CommandLine)" } else { "exited" }
                    if ($parent) {
                        $record.ParentName = $parent.Name
                        try { $record.ParentPosture = [CuaTokenPosture]::Read($record.ParentPid) } catch { }
                    }
                }
                [void]$state.Browsers.Add([pscustomobject]$record)
            }
            Start-Sleep -Milliseconds 200
        }
    }

    Push-Location $exampleDir
    try {
        & $python verify_mcp_tools.py
        if ($LASTEXITCODE -ne 0) { throw "verify_mcp_tools.py failed with exit code $LASTEXITCODE" }
        & $python verify_setup.py --typescript --output-dir $proofDir
        if ($LASTEXITCODE -ne 0) { throw "verify_setup.py failed with exit code $LASTEXITCODE" }
    } finally {
        Pop-Location
    }

    $state.Stop = $true
    $sampler | Wait-Job -Timeout 30 | Out-Null
    $sampler | Receive-Job -ErrorAction Continue

    # 5. Elevated Drivers launched the isolated browsers with standard-user
    #    tokens.
    $browsers = @($state.Browsers)
    foreach ($browser in $browsers) {
        Write-Posture "[browser] name=$($browser.Name) main=$($browser.Main) parent=$($browser.ParentPid)$(if ($browser.Parent) { " parent_process=[$($browser.Parent)] parent_token=[$($browser.ParentPosture)]" }) $(if ($browser.Posture) { $browser.Posture } else { "pid=$($browser.Pid) unreadable: $($browser.Error)" })"
    }
    $mains = @($browsers | Where-Object { $_.Main -and $null -ne $_.Posture })
    if ($mains.Count -lt 2) {
        throw "expected an isolated browser main process for each of the two agents, observed $($mains.Count)"
    }
    foreach ($browser in $browsers | Where-Object { $null -ne $_.Posture }) {
        $posture = $browser.Posture
        if ($posture.AdministratorsEnabled -or $posture.IntegrityRid -gt $MediumIntegrity) {
            throw "an isolated browser process ran with a privileged token: $posture"
        }
    }
    foreach ($browser in $mains) {
        $launcher = $browser.ParentPosture
        if ($browser.ParentName -ne "cua-driver.exe" -or $null -eq $launcher) {
            throw "isolated browser $($browser.Pid) was not launched by a readable cua-driver.exe: [$($browser.Parent)]"
        }
        if ($launcher.IntegrityRid -lt $HighIntegrity) {
            throw "the Driver that launched isolated browser $($browser.Pid) is not elevated, so this run does not prove the elevated path: $launcher"
        }
        if ($browser.Posture.IntegrityRid -ge $launcher.IntegrityRid) {
            throw "isolated browser $($browser.Posture) is not below its launching Driver $launcher"
        }
    }
    Write-Posture "[verdict] elevated Driver launched $($mains.Count) isolated browsers at Medium integrity or lower; autostart daemon elevated"
} finally {
    $state.Stop = $true
    if ($null -ne $sampler) { $sampler | Remove-Job -Force -ErrorAction SilentlyContinue }
    & $driver stop *> $null
}
