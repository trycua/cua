<#
.SYNOPSIS
Cua uninstaller for Windows: removes what install.ps1 and the Cua Spaces app put on this PC.

.DESCRIPTION
    irm https://cua.ai/uninstall.ps1 | iex
    & ([scriptblock]::Create((irm https://cua.ai/uninstall.ps1))) -DryRun
    & ([scriptblock]::Create((irm https://cua.ai/uninstall.ps1))) -Yes -KeepData

By default it removes the Cua Spaces app (through its own uninstaller), the cua
CLI and its user PATH entry, the host service (`cua host remove --uninstalling`),
your agents' Cua Spaces skills and MCP entries (`cua agents remove`: the `cua`
server and the Cua Spaces skills; Cua Driver's `cua-driver` entries and other
tools' stay), launch at login, your sign-in and device key (Windows Credential
Manager), and Cua's settings and caches. It never deletes a Space or its data:
that needs -DeleteSpaces.

.PARAMETER KeepData
Keep %USERPROFILE%\.cua (settings, the Spaces registry, caches, the Cua Volume
cache, device key). Programs, services, integrations and your sign-in are still removed.
.PARAMETER DeleteSpaces
Also delete the Spaces on this PC and their data (Space disks, the local Cua
Volume files, the Keyvault). Running Spaces are listed and stopped first. This
cannot be undone; you are asked to type "delete" unless -Yes is given.
.PARAMETER All
Also run the Cua Driver uninstaller, and remove Cua Driver's skill and MCP
server from your agents.
.PARAMETER Prefix
Also remove what install.ps1 -Prefix put there (Prefix\bin).
.PARAMETER NoModifyPath
Do not edit the user PATH.
.PARAMETER DryRun
Print what would be removed; change nothing. It lists the agent skills and MCP
entries one by one.
.PARAMETER Yes
Accept every prompt (non-interactive).
#>
[CmdletBinding()]
param(
    [switch]$KeepData,
    [switch]$DeleteSpaces,
    [switch]$All,
    [string]$Prefix = '',
    [switch]$NoModifyPath,
    [switch]$DryRun,
    [switch]$Yes
)

# Windows PowerShell 5.1 started from PowerShell 7 inherits pwsh's module path
# and cannot load its own utility modules; see install.ps1.
if ($PSVersionTable.PSEdition -eq 'Desktop') {
    $desktopModules = Join-Path $PSHOME 'Modules'
    $modulePaths = @($desktopModules) + @(($env:PSModulePath -split ';') | Where-Object {
            $_ -and ($_ -notmatch '\\PowerShell\\') -and ($_.TrimEnd('\') -ne $desktopModules.TrimEnd('\'))
        })
    $env:PSModulePath = $modulePaths -join ';'
}

$DriverUrl = if ($env:CUA_UNINSTALL_DRIVER_URL) { $env:CUA_UNINSTALL_DRIVER_URL } else { 'https://cua.ai/driver/uninstall.ps1' }
# Settings and local state inside the cua home, removed unless -KeepData.
# Spaces and their data (sandboxes, vmm, images, volume\data, the Keyvault,
# trajectories, spaces.json) are not on this list: only -DeleteSpaces removes them.
$DisposableNames = @('cache', 'cache.json', 'daemon.json', 'daemon.starting', 'fleet-gc.lock', 'config', 'config.toml',
    'env-tokens.json', 'do_target.json', 'host_consented', 'host', 'spacesd', 'telemetry', 'skills', 'bin', 'runtimes',
    'spaces-install-mode', 'volume\cache')
# The sign-in goes with the uninstall even with -KeepData.
$SessionFiles = @('credentials.json', 'spaces-session.json', 'spaces-credentials.json')
$script:Failed = $false

function Write-Info([string]$Message) { Write-Host "cua-uninstall: $Message" }
function Write-Warn([string]$Message) { Write-Warning "cua-uninstall: $Message" }
function Stop-Uninstall([string]$Message) { throw "cua-uninstall: error: $Message" }

function Test-Interactive {
    if ($env:CUA_UNINSTALL_NONINTERACTIVE -eq '1') { return $false }
    try { return [Environment]::UserInteractive -and -not [Console]::IsInputRedirected } catch { return $false }
}

# Asks a yes/no question; the default is no. -Yes accepts.
function Confirm-Step([string]$Question) {
    if ($Yes) { return $true }
    if (-not (Test-Interactive)) { return $false }
    $answer = Read-Host "$Question [y/N]"
    return ($answer -match '^(y|yes)$')
}

function Get-CuaHome {
    if ($env:CUA_HOME) { return $env:CUA_HOME }
    return (Join-Path $HOME '.cua')
}

function Get-LocalAppData {
    if ($env:LOCALAPPDATA) { return $env:LOCALAPPDATA }
    return (Join-Path $HOME 'AppData\Local')
}

function Get-RoamingAppData {
    if ($env:APPDATA) { return $env:APPDATA }
    return (Join-Path $HOME 'AppData\Roaming')
}

# A path this script may delete: not a drive root, the profile or a system or
# program folder.
function Test-SafePath([string]$Path) {
    if (-not $Path -or -not [IO.Path]::IsPathRooted($Path)) { return $false }
    if ($Path -match '(^|[\\/])\.\.([\\/]|$)') { return $false }
    $full = [IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    $guards = @($HOME, $env:USERPROFILE, (Get-LocalAppData), (Get-RoamingAppData), $env:ProgramFiles, ${env:ProgramFiles(x86)},
        $env:SystemRoot, $env:SystemDrive, (Join-Path (Get-LocalAppData) 'Programs')) | Where-Object { $_ }
    foreach ($guard in $guards) {
        if ($full -ieq ([IO.Path]::GetFullPath($guard).TrimEnd('\', '/'))) { return $false }
    }
    return ($full.Length -gt 3 -and -not ($full -match '^[A-Za-z]:$'))
}

# Removes a file or folder when it exists; says so when it cannot.
function Remove-CuaPath([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return }
    if (-not (Test-SafePath $Path)) { Write-Warn "not touching $Path"; return }
    if ($DryRun) { Write-Host "[dry-run] remove $Path"; return }
    Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $Path) {
        Write-Warn "could not remove $Path (is a program using it? close it and run this again)"
        $script:Failed = $true
    } else {
        Write-Info "removed $Path"
    }
}

# Removes a folder only when it is empty.
function Remove-EmptyDirectory([string]$Path) {
    if ($DryRun -or -not (Test-Path -LiteralPath $Path -PathType Container) -or -not (Test-SafePath $Path)) { return }
    if (-not (Get-ChildItem -LiteralPath $Path -Force -ErrorAction SilentlyContinue)) {
        Remove-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    }
}

# Runs a program with no input and kills it after Seconds. Returns ExitCode and Output.
function Invoke-Bounded([string]$File, [string[]]$Arguments, [int]$Seconds) {
    $out = Join-Path ([IO.Path]::GetTempPath()) ('cua-uninstall-' + [Guid]::NewGuid().ToString('N') + '.out')
    $err = "$out.err"
    try {
        $params = @{ FilePath = $File; PassThru = $true; NoNewWindow = $true; RedirectStandardOutput = $out; RedirectStandardError = $err }
        if ($Arguments -and $Arguments.Count -gt 0) {
            $params.ArgumentList = @($Arguments | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } })
        }
        $process = Start-Process @params
        $null = $process.Handle
        if (-not $process.WaitForExit($Seconds * 1000)) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
            return [pscustomobject]@{ ExitCode = 124; Output = '' }
        }
        $text = if (Test-Path -LiteralPath $out) { Get-Content -Raw -LiteralPath $out } else { '' }
        return [pscustomobject]@{ ExitCode = $process.ExitCode; Output = "$text" }
    } catch {
        return [pscustomobject]@{ ExitCode = 127; Output = '' }
    } finally {
        Remove-Item -LiteralPath $out, $err -Force -ErrorAction SilentlyContinue
    }
}

# A cua.exe that Cua put there: `cua --version` says `cua X.Y.Z`.
function Test-CuaCli([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $false }
    $result = Invoke-Bounded $Path @('--version') 10
    return ($result.ExitCode -eq 0 -and $result.Output -match '^cua \d+\.\d+\.\d+')
}

function Get-CuaCliCandidates {
    $list = @()
    if ($Prefix) { $list += (Join-Path $Prefix 'bin\cua.exe') }
    $list += (Join-Path (Get-LocalAppData) 'Programs\cua\bin\cua.exe')
    return @($list | Select-Object -Unique)
}

function Find-Cua {
    $candidates = @()
    $candidates += Get-CuaCliCandidates
    $onPath = Get-Command cua -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($onPath) { $candidates += $onPath.Source }
    $candidates += (Join-Path (Get-LocalAppData) 'Programs\Cua Spaces\resources\native\cua.exe')
    if ($env:ProgramFiles) { $candidates += (Join-Path $env:ProgramFiles 'Cua Spaces\resources\native\cua.exe') }
    foreach ($candidate in $candidates) {
        if (Test-CuaCli $candidate) { return $candidate }
    }
    return $null
}

# Runs the cua CLI for a removal step (bounded, no usage telemetry).
function Invoke-Cua([string]$Label, [int]$Seconds, [string[]]$Arguments, [hashtable]$Environment = @{}) {
    if ($DryRun) { Write-Host "[dry-run] cua $($Arguments -join ' ')"; return }
    Write-Info $Label
    $saved = @{}
    $vars = @{ CUA_TELEMETRY = '0'; DO_NOT_TRACK = '1' }
    foreach ($k in $Environment.Keys) { $vars[$k] = $Environment[$k] }
    foreach ($k in $vars.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $vars[$k]) }
    try {
        $result = Invoke-Bounded $script:Cua $Arguments $Seconds
        if ($result.ExitCode -ne 0) { Write-Warn "'cua $($Arguments -join ' ')' did not finish; continuing" }
        return $result
    } finally {
        foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
    }
}

# Whether this cua removes only Cua Spaces' entries from agents (`cua agents
# remove`: its own `cua` server and skills) and can preview that (--dry-run).
# Older builds removed every entry cua ever wrote, Cua Driver's `cua-driver`
# server and skill included, and had neither flag. The new flag in its help is
# the test.
function Test-CuaScopesAgentsRemove {
    if (-not $script:Cua) { return $false }
    $result = Invoke-Bounded $script:Cua @('agents', 'remove', '--help') 20
    return ($result.ExitCode -eq 0 -and "$($result.Output)" -match '--include-cua-driver')
}

# What a cua that is too old to leave Cua Driver's entries alone costs: its
# `agents remove` would strip them, so the step is skipped.
$OldCuaAgents = 'this cua is older than the version that removes only Cua Spaces'' entries from your agents: its ''cua agents remove'' would also take Cua Driver''s skill and MCP server out of Claude Code, Codex, Cursor and your other agents. So the agent step is skipped, and Cua Spaces'' ''cua'' MCP server and skills (cua-spaces, cua-volume, cua-sandboxes, gui-automation) stay in your agents. To remove them later, install the latest cua (& ([scriptblock]::Create((irm https://cua.ai/install.ps1))) -CliOnly -Yes) and run: cua agents remove'

# Removes the Cua Spaces skills and MCP entries (the `cua` server, `cua-local`
# for Local) from the agents. Cua Driver's `cua-driver` skill and server, and
# every other tool's entries, stay, unless -All asked for Cua Driver to go.
# With -DryRun it runs the same command with --dry-run, so the list is what the
# real run removes.
function Remove-AgentIntegrations {
    $agentArgs = @('agents', 'remove', '--agents', 'all')
    if ($All) { $agentArgs += '--include-cua-driver' }
    if (-not (Test-CuaScopesAgentsRemove)) {
        if ($DryRun) {
            Write-Host "[dry-run] cua $($agentArgs -join ' '): skipped, $OldCuaAgents"
        } else {
            Write-Warn $OldCuaAgents
            if ($DeleteSpaces) { Write-Warn '-DeleteSpaces also deletes the record cua keeps of them (agent-setup.json), so remove them by hand' }
            $script:Failed = $true
        }
        return
    }
    if ($DryRun) {
        Write-Host "[dry-run] cua $($agentArgs -join ' ') --dry-run:"
        $plan = Invoke-Bounded $script:Cua ($agentArgs + '--dry-run') 60
        # Whatever it listed, even when one agent's config could not be read:
        # a real run removes the rows that did not fail.
        foreach ($row in @("$($plan.Output)" -split "`r?`n" | Where-Object { $_.Trim() -and $_ -notlike 'Dry run:*' })) { Write-Host "[dry-run] $row" }
        if ($plan.ExitCode -ne 0) {
            Write-Warn "'cua $($agentArgs -join ' ') --dry-run' reported a problem (see above), so the list may be incomplete"
        }
        return
    }
    $null = Invoke-Cua 'removing the Cua Spaces skills and MCP entries from your agents' 60 ($agentArgs + '--yes')
}

# The cua homes: the main one and one per environment (~/.cua/envs/staging).
function Get-CuaHomes {
    $home1 = Get-CuaHome
    if (-not (Test-Path -LiteralPath $home1 -PathType Container)) { return @() }
    $list = @($home1)
    $envs = Join-Path $home1 'envs'
    if (Test-Path -LiteralPath $envs -PathType Container) {
        $list += @(Get-ChildItem -LiteralPath $envs -Directory -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName })
    }
    return $list
}

# "ID STATUS" rows for each local Space, from `cua sandbox ls --local`.
function Get-LocalSpaces {
    if (-not $script:Cua) { return @() }
    $result = Invoke-Bounded $script:Cua @('sandbox', 'ls', '--local', '--no-size') 30
    if ($result.ExitCode -ne 0) { return @() }
    $rows = @()
    $lines = @($result.Output -split "`r?`n" | Where-Object { $_.Trim() })
    for ($i = 1; $i -lt $lines.Count; $i++) {
        $cells = @($lines[$i].Trim() -split '\s+')
        if ($cells.Count -ge 2 -and $cells[0] -ne 'No') { $rows += [pscustomobject]@{ Id = $cells[0]; Status = $cells[1] } }
    }
    return $rows
}

# --- the Windows-specific steps (small, so tests can replace them) ---------------

function Stop-CuaSpacesApp {
    $running = @(Get-Process -Name 'Cua Spaces' -ErrorAction SilentlyContinue)
    if ($running.Count -eq 0) { return }
    if ($DryRun) { Write-Host '[dry-run] quit Cua Spaces'; return }
    Write-Info 'quitting Cua Spaces'
    foreach ($p in $running) { $null = $p.CloseMainWindow() }
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Process -Name 'Cua Spaces' -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }
    Get-Process -Name 'Cua Spaces' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}

# The Cua Spaces app's entry in Add or remove programs (per user, or per machine).
function Get-AppUninstallEntry {
    $roots = @('HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall', 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
        'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall')
    foreach ($root in $roots) {
        if (-not (Test-Path $root)) { continue }
        foreach ($key in Get-ChildItem $root -ErrorAction SilentlyContinue) {
            $props = Get-ItemProperty $key.PSPath -ErrorAction SilentlyContinue
            if ($props.DisplayName -eq 'Cua Spaces') {
                return [pscustomobject]@{
                    Key = $key.PSChildName
                    InstallLocation = $props.InstallLocation
                    UninstallString = $props.UninstallString
                    QuietUninstallString = $props.QuietUninstallString
                    WindowsInstaller = ($props.WindowsInstaller -eq 1)
                }
            }
        }
    }
    return $null
}

# Runs the app's own uninstaller silently: it also stops the host service and
# removes launch at login and the CLI the app put on PATH.
function Invoke-AppUninstaller($Entry) {
    if ($Entry.WindowsInstaller) {
        $proc = Start-Process -FilePath 'msiexec.exe' -ArgumentList @('/x', $Entry.Key, '/qn', '/norestart') -Wait -PassThru
        return $proc.ExitCode
    }
    $command = if ($Entry.QuietUninstallString) { $Entry.QuietUninstallString } else { "$($Entry.UninstallString) /S" }
    if ($command -notmatch '^\s*"?([^"]+?\.exe)"?\s*(.*)$') { return 1 }
    $start = @{ FilePath = $Matches[1]; Wait = $true; PassThru = $true }
    if ($Matches[2]) { $start.ArgumentList = $Matches[2] }
    $proc = Start-Process @start
    # The NSIS uninstaller hands over to a copy of itself: wait for the folder to go.
    $deadline = (Get-Date).AddSeconds(90)
    while ($Entry.InstallLocation -and (Test-Path -LiteralPath $Entry.InstallLocation) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 500
    }
    return $proc.ExitCode
}

function Remove-AppRegistryKey {
    Remove-Item -Path 'HKCU:\Software\ai.cua.spaces.desktop' -Recurse -Force -ErrorAction SilentlyContinue
}

function Remove-HostTask {
    foreach ($task in @('Cua\SpacesdHost', 'Cua\EnvDriverHost')) {
        $null = & schtasks.exe /End /TN $task 2>&1
        $null = & schtasks.exe /Delete /TN $task /F 2>&1
    }
}

function Remove-RunValue([string]$Name) {
    foreach ($key in @('HKCU:\Software\Microsoft\Windows\CurrentVersion\Run', 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run')) {
        if (Get-ItemProperty -Path $key -Name $Name -ErrorAction SilentlyContinue) {
            Remove-ItemProperty -Path $key -Name $Name -ErrorAction SilentlyContinue
        }
    }
}

function Get-UserPath { return [Environment]::GetEnvironmentVariable('Path', 'User') }
function Set-UserPath([string]$Value) { [Environment]::SetEnvironmentVariable('Path', $Value, 'User') }

# The targets Windows Credential Manager holds, `cmdkey /list` style.
function Get-CredentialTargets {
    $targets = @()
    if (-not (Get-Command cmdkey.exe -ErrorAction SilentlyContinue)) { return $targets }
    foreach ($line in @(& cmdkey.exe /list 2>$null)) {
        if ($line -match 'Target:\s*(?:LegacyGeneric:target=|Domain:target=)?(.+?)\s*$') { $targets += $Matches[1] }
    }
    return $targets
}

function Remove-CredentialTarget([string]$Target) {
    $null = & cmdkey.exe "/delete:$Target" 2>&1
    return ($LASTEXITCODE -eq 0)
}

# --- the removal steps --------------------------------------------------------

# Credential Manager: the session and device key are `cua-cli[.name].run.cua.ai`
# (`.staging.run.cua.ai` for staging); the Keyvault master key is `<id>.Cua Keyvault`.
function Select-CuaCredentialTargets([string[]]$Targets) {
    $picked = @()
    foreach ($target in $Targets) {
        $isSession = $target -match '^cua-cli\.(staging\.)?run\.cua\.ai$'
        $isOther = $target -match '^cua-cli\..+\.(staging\.)?run\.cua\.ai$'
        $isKeyvault = $target -match '\.Cua Keyvault( Generation)?$'
        if ($isSession) { $picked += $target }
        elseif ($isOther -and -not $KeepData) { $picked += $target }
        elseif ($isKeyvault -and $DeleteSpaces) { $picked += $target }
    }
    return $picked
}

function Remove-CuaCredentials {
    $targets = @(Select-CuaCredentialTargets @(Get-CredentialTargets))
    foreach ($target in $targets) {
        if ($DryRun) { Write-Host "[dry-run] remove the Credential Manager entry $target"; continue }
        if (Remove-CredentialTarget $target) { Write-Info "removed the Credential Manager entry $target" }
        else { Write-Warn "could not remove the Credential Manager entry $target; delete it in Control Panel, Credential Manager"; $script:Failed = $true }
    }
}

function Remove-CuaCli {
    foreach ($exe in Get-CuaCliCandidates) {
        if (-not (Test-Path -LiteralPath $exe)) { continue }
        if (Test-CuaCli $exe) {
            Remove-CuaPath $exe
            $bin = Split-Path -Parent $exe
            Remove-EmptyDirectory $bin
            Remove-EmptyDirectory (Split-Path -Parent $bin)
            if (-not $NoModifyPath) { Remove-FromUserPath $bin }
        } else {
            Write-Info "leaving $exe (not the Cua CLI)"
        }
    }
}

# Takes a folder off the user PATH once it holds no cua.exe any more.
function Remove-FromUserPath([string]$Dir) {
    if (Test-Path -LiteralPath (Join-Path $Dir 'cua.exe')) { return }
    $current = Get-UserPath
    if (-not $current) { return }
    $parts = @($current -split ';' | Where-Object { $_ })
    $kept = @($parts | Where-Object { $_.TrimEnd('\') -ine $Dir.TrimEnd('\') })
    if ($kept.Count -eq $parts.Count) { return }
    if ($DryRun) { Write-Host "[dry-run] remove $Dir from the user PATH"; return }
    Set-UserPath ($kept -join ';')
    Write-Info "removed $Dir from your user PATH (new terminals pick it up)"
}

function Remove-CuaState {
    $cuaHome = Get-CuaHome
    if (-not (Test-Path -LiteralPath $cuaHome -PathType Container)) { return }
    if ($DeleteSpaces) {
        if (($env:CUA_HOME) -and -not $script:HomeRecognised) {
            Write-Warn "CUA_HOME=$cuaHome has no Cua files in it; leaving it alone"
            return
        }
        Remove-CuaPath $cuaHome
        return
    }
    foreach ($h in Get-CuaHomes) {
        foreach ($name in $SessionFiles) { Remove-CuaPath (Join-Path $h $name) }
        if (-not $KeepData) {
            foreach ($name in $DisposableNames) { Remove-CuaPath (Join-Path $h $name) }
        }
        Remove-EmptyDirectory (Join-Path $h 'volume')
    }
    if ($KeepData) { Write-Info "kept $cuaHome"; return }
    if ($DryRun) { Write-Host "[dry-run] keep the Spaces and their data in $cuaHome (-DeleteSpaces removes them)"; return }
    $envs = Join-Path $cuaHome 'envs'
    if (Test-Path -LiteralPath $envs) {
        Get-ChildItem -LiteralPath $envs -Directory -ErrorAction SilentlyContinue | ForEach-Object { Remove-EmptyDirectory $_.FullName }
        Remove-EmptyDirectory $envs
    }
    Remove-EmptyDirectory $cuaHome
    if (Test-Path -LiteralPath $cuaHome) {
        $left = (Get-ChildItem -LiteralPath $cuaHome -Force | ForEach-Object { $_.Name }) -join ' '
        Write-Info "kept in ${cuaHome}: $left (Spaces and their data; -DeleteSpaces removes them)"
    }
}

# A CUA_HOME that was set by hand is only deleted when Cua files are in it,
# checked before any step removes some of them.
$script:HomeRecognised = $true
function Test-HomeRecognised {
    if (-not $env:CUA_HOME) { return }
    $script:HomeRecognised = $false
    foreach ($name in @('host', 'sandboxes', 'vmm', 'daemon.json', 'config.toml', 'credentials.json', 'spaces.json', 'volume')) {
        if (Test-Path -LiteralPath (Join-Path $env:CUA_HOME $name)) { $script:HomeRecognised = $true }
    }
}

function Remove-CuaApp($Entry) {
    if ($DryRun) {
        if ($Entry) { Write-Host "[dry-run] run the Cua Spaces uninstaller ($($Entry.InstallLocation))" }
        Write-Host '[dry-run] remove the host scheduled task and launch at login'
    } else {
        if ($Entry) {
            Write-Info 'removing Cua Spaces (its own uninstaller, silent)'
            $code = Invoke-AppUninstaller $Entry
            if ($code -ne 0 -and $code -ne 3010) { Write-Warn "the Cua Spaces uninstaller exited with $code"; $script:Failed = $true }
            if ($Entry.InstallLocation) { Remove-CuaPath $Entry.InstallLocation }
        }
        Remove-HostTask
        Remove-RunValue 'ai.cua.spaces.desktop'
        Remove-RunValue 'CuaSpacesdHost'
        Remove-AppRegistryKey
    }
    if (-not $KeepData) { Remove-CuaPath (Join-Path (Get-RoamingAppData) 'Cua Spaces') }
}

function Invoke-OtherUninstaller([string]$Name, [string]$Url) {
    if ($DryRun) { Write-Host "[dry-run] download $Url"; Write-Host "[dry-run] run the $Name uninstaller"; return }
    if ($Url -notmatch '^https://' -and $Url -notmatch '^http://(127\.0\.0\.1|localhost)[:/]') {
        Stop-Uninstall "refusing a non-HTTPS download: $Url"
    }
    $script = Join-Path ([IO.Path]::GetTempPath()) ("cua-$Name-uninstall-" + [Guid]::NewGuid().ToString('N') + '.ps1')
    try {
        Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $script
        Write-Info "running the $Name uninstaller"
        $shell = (Get-Process -Id $PID).Path
        if (-not $shell) { $shell = 'powershell.exe' }
        & $shell -NoProfile -ExecutionPolicy Bypass -File $script
        if ($LASTEXITCODE -ne 0) { Write-Warn "the $Name uninstaller did not finish"; $script:Failed = $true }
    } catch {
        Write-Warn "${Name}: $($_.Exception.Message)"
        $script:Failed = $true
    } finally {
        Remove-Item -LiteralPath $script -Force -ErrorAction SilentlyContinue
    }
}

function Invoke-CuaUninstall {
    $script:Cua = Find-Cua
    $entry = Get-AppUninstallEntry
    $cuaHome = Get-CuaHome
    $found = @()
    if ($entry) { $found += "Cua Spaces app: $($entry.InstallLocation)" }
    foreach ($exe in Get-CuaCliCandidates) { if ((Test-Path -LiteralPath $exe) -and (Test-CuaCli $exe)) { $found += "cua CLI: $exe" } }
    if (Test-Path -LiteralPath $cuaHome) { $found += "cua state: $cuaHome" }
    if (@(Select-CuaCredentialTargets @(Get-CredentialTargets)).Count -gt 0) { $found += 'Credential Manager entries: cua-cli' }
    $appData = Join-Path (Get-RoamingAppData) 'Cua Spaces'
    if (Test-Path -LiteralPath $appData) { $found += "app settings: $appData" }
    if ($found.Count -eq 0 -and -not $All) {
        Write-Info 'Cua does not look installed here; nothing to remove'
        return
    }
    if ($found.Count -gt 0) {
        Write-Host 'Found on this PC:'
        $found | ForEach-Object { Write-Host "  $_" }
        if ($script:Cua) { Write-Host "  (the removal steps run the cua at $($script:Cua))" }
    }
    if ($DryRun) { Write-Info 'dry run: nothing will be changed' }
    Test-HomeRecognised
    $spaces = @(Get-LocalSpaces)

    if (-not $DryRun) {
        Write-Host ''
        Write-Host 'This removes Cua Spaces, the cua CLI, the host service, Cua Spaces'' skills and MCP entry in your'
        Write-Host 'agents, launch at login and your sign-in.'
        if (-not $All) { Write-Host 'Cua Driver''s skill and MCP server in your agents, and other tools'' entries, stay.' }
        if ($KeepData) { Write-Host "It keeps $cuaHome and the Cua Volume cache (-KeepData)." }
        elseif ($DeleteSpaces) { Write-Host 'It also deletes Cua''s settings and caches.' }
        else { Write-Host 'It also deletes Cua''s settings and caches, but keeps your Spaces and their data.' }
        if ($All) { Write-Host 'It also runs the Cua Driver uninstaller, and removes Cua Driver''s skill and MCP server from your agents (-All).' }
        if (-not $Yes -and -not (Test-Interactive)) {
            Stop-Uninstall 'no terminal to ask on; rerun with -Yes (or -DryRun to see what would be removed)'
        }
        if (-not (Confirm-Step 'Continue?')) { Write-Host 'Cancelled; nothing was removed.'; return }
        if ($DeleteSpaces) {
            Write-Host ''
            Write-Host '-DeleteSpaces DELETES the Spaces on this PC and their data:'
            $spaces | ForEach-Object { Write-Host "  $($_.Id) $($_.Status)" }
            Write-Host "  ${cuaHome}: Space disks, the local Cua Volume files, the Keyvault. This cannot be undone."
            if (-not $Yes) {
                $answer = Read-Host 'Type "delete" to delete them, or anything else to cancel'
                if ($answer -ne 'delete') { Write-Host 'Cancelled; nothing was removed.'; return }
            }
        }
    }

    Stop-CuaSpacesApp
    if ($script:Cua) {
        if ($spaces.Count -gt 0) {
            Write-Host ''
            Write-Host 'Spaces on this PC:'
            $spaces | ForEach-Object { Write-Host "  $($_.Id) $($_.Status)" }
        }
        foreach ($space in $spaces) {
            if ($space.Status -in @('stopped', 'missing')) { continue }
            if ($DryRun) { Write-Host "[dry-run] cua spaces stop $($space.Id)" }
            else { Write-Info "stopping $($space.Id) (its disk is kept)"; $null = Invoke-Bounded $script:Cua @('spaces', 'stop', $space.Id) 90 }
        }
        if ($DeleteSpaces) {
            foreach ($space in $spaces) {
                if ($DryRun) { Write-Host "[dry-run] cua sandbox rm $($space.Id) --force" }
                else { Write-Info "deleting Space $($space.Id)"; $null = Invoke-Bounded $script:Cua @('sandbox', 'rm', $space.Id, '--force') 120 }
            }
        }
        foreach ($h in Get-CuaHomes) {
            if (Test-Path -LiteralPath (Join-Path $h 'host\config.json')) {
                $null = Invoke-Cua "turning off hosting ($h)" 150 @('host', 'remove', '--force', '--uninstalling') @{ CUA_HOME = $h }
            }
        }
        Remove-AgentIntegrations
        foreach ($h in Get-CuaHomes) { $null = Invoke-Cua 'stopping the cua daemon' 30 @('daemon', 'stop') @{ CUA_HOME = $h } }
        $null = Invoke-Cua 'signing out (cua auth logout)' 60 @('auth', 'logout')
        if (Test-Path -LiteralPath (Join-Path $cuaHome 'envs\staging')) {
            $null = Invoke-Cua 'signing out of staging' 60 @('auth', 'logout') @{ CUA_ENV = 'staging' }
        }
    }
    foreach ($h in Get-CuaHomes) {
        foreach ($name in $SessionFiles) { Remove-CuaPath (Join-Path $h $name) }
    }
    Remove-CuaApp $entry
    Remove-CuaCli
    if ($All) { Invoke-OtherUninstaller 'cua-driver' $DriverUrl }
    Remove-CuaCredentials
    Remove-CuaState

    if ($DryRun) { Write-Info 'dry run finished; nothing was changed'; return }
    if (-not $All -and (Get-Command cua-driver -ErrorAction SilentlyContinue)) {
        Write-Info 'Cua Driver is separate and was kept; run this with -All to remove it too'
    }
    if ($script:Failed) { Write-Info 'finished, but some steps need your attention (see the warnings above)'; $global:LASTEXITCODE = 1; return }
    Write-Info 'done. To install again: irm https://cua.ai/install.ps1 | iex'
}

# Dot-sourcing with CUA_UNINSTALL_NO_RUN=1 loads the functions for tests.
if ($env:CUA_UNINSTALL_NO_RUN -ne '1') {
    if ($KeepData -and $DeleteSpaces) { Stop-Uninstall '-KeepData and -DeleteSpaces contradict each other' }
    Invoke-CuaUninstall
}
