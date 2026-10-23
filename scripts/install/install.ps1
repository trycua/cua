<#
.SYNOPSIS
Cua installer for Windows: the `cua` CLI, the Cua Spaces app and agent extras.

.DESCRIPTION
    irm https://cua.ai/install.ps1 | iex
    & ([scriptblock]::Create((irm https://cua.ai/install.ps1))) -Select cua-driver
    & ([scriptblock]::Create((irm https://cua.ai/install.ps1))) -CliOnly -Yes
    & ([scriptblock]::Create((irm https://cua.ai/install.ps1))) -Pair K7QM-4XPB-2RHD

At a console it first says what Cua Spaces is, then shows a numbered list of
what to add to the command-line tools (the `cua` program: always installed,
except with -AppOnly). Items:
    spaces      the Cua Spaces app (the Electron app's setup.exe; on by default)
    cua-driver  Cua Driver, which lets AI agents use apps while you keep
                working: installs it and adds its skill and MCP server to
                your agents (off by default)
The list does not offer hosting. The app's first run asks how to use this
computer (-Mode host|client preselects the answer). For scripts, `-Select host`
and `-Only host` still run `cua host setup`, which lets your other devices use
this computer through the relay.
Set CUA_INSTALL_NONINTERACTIVE=1 to never prompt. Add -Verbose to see every
step (manifest, downloads, signature checks).

Reads release-artifacts.json (see scripts/install/README.md), downloads the
artifacts for this machine, verifies their sha256 (and a minisign or cosign
signature when available), installs `cua` to %LOCALAPPDATA%\Programs\cua\bin
(or -Prefix\bin) and adds that directory to your user PATH (-NoModifyPath
skips it), then runs `cua auth login` when a user is at the console (not
with -CliOnly, -Pair or -NoOnboarding).
-AppOnly, -Mode and the `spaces` item install the Cua Spaces app: its signed
setup.exe for this machine's architecture, run silently (/S) after checking sha256,
cosign (when available) and its Authenticode signature (signer Cua AI, Inc.).

.PARAMETER Select
Preselect items (comma list: spaces, cua-driver, host); the list still shows.
.PARAMETER Only
Install exactly these items plus the CLI, without the checklist.
.PARAMETER CliOnly
Install only the `cua` CLI (same as -Only cli), without prompts or sign-in.
.PARAMETER AppOnly
Install only the Cua Spaces app (no CLI).
.PARAMETER Mode
Preselect the app's first-run choice: host or client (implies spaces).
.PARAMETER Pair
Add this machine to your account with a one-time code from the Cua Spaces app
(Add a machine): installs the CLI and runs `cua host setup --pair <code>`, with
no sign-in and no checklist. A computer that already has a cua with
`host setup --pair` uses it instead of downloading again; -Prefix, -Version or
CUA_INSTALL_REINSTALL=1 install anyway.
.PARAMETER Relay
Relay for -Pair (default https://relay.cua.ai).
.PARAMETER Version
Install this CLI release (default: latest).
.PARAMETER Prefix
Install the CLI under Prefix\bin.
.PARAMETER ModifyPath
Kept for older commands: the CLI directory is added to the user PATH by default.
.PARAMETER NoModifyPath
Do not add the CLI directory to the user PATH.
.PARAMETER NoOnboarding
Do not run `cua auth login` afterwards.
.PARAMETER NoSkills
With cua-driver, add only its MCP server to your agents, not its skill (by default both are added).
.PARAMETER RequireSignature
Fail unless a signature verifies.
.PARAMETER Installer
App installer to use: nsis (default, the Electron app's setup.exe) or msi (used only when a release has one).
.PARAMETER DryRun
Print what would happen; change nothing.
.PARAMETER Yes
Accept the selection and every prompt.
#>
[CmdletBinding()]
param(
    [string]$Select = '',
    [string]$Only = '',
    [switch]$CliOnly,
    [switch]$AppOnly,
    [ValidateSet('host', 'client', '')][string]$Mode = '',
    [string]$Pair = '',
    [string]$Relay = '',
    [string]$Version = '',
    [string]$Prefix = '',
    [switch]$ModifyPath,
    [switch]$NoModifyPath,
    [switch]$NoOnboarding,
    [switch]$NoSkills,
    [switch]$RequireSignature,
    [ValidateSet('msi', 'nsis')][string]$Installer = 'nsis',
    [switch]$DryRun,
    [switch]$Yes
)

# Windows PowerShell 5.1 started from PowerShell 7 (a pwsh terminal, or
# `cua-driver update --apply` launched from one) inherits pwsh's
# PSModulePath. Autoload then finds PowerShell 7's Core-only copies of
# Microsoft.PowerShell.Utility / .Security / .Archive first, fails to load
# them, and Get-FileHash, Get-AuthenticodeSignature and Expand-Archive are
# "not recognized". Put this edition's own modules first and drop the
# PowerShell 7 module paths before any of them is used.
if ($PSVersionTable.PSEdition -eq 'Desktop') {
    $desktopModules = Join-Path $PSHOME 'Modules'
    $modulePaths = @($desktopModules) + @(($env:PSModulePath -split ';') | Where-Object {
            $_ -and ($_ -notmatch '\\PowerShell\\') -and ($_.TrimEnd('\') -ne $desktopModules.TrimEnd('\'))
        })
    $env:PSModulePath = $modulePaths -join ';'
}

$Repo = if ($env:CUA_INSTALL_REPO) { $env:CUA_INSTALL_REPO } else { 'trycua/cua' }
$BaseUrl = if ($env:CUA_INSTALL_BASE_URL) { $env:CUA_INSTALL_BASE_URL } else { "https://github.com/$Repo/releases/download" }
$LatestTag = 'cua-install-latest'
$MinisignPubkey = $env:CUA_INSTALL_MINISIGN_PUBKEY
$CuaItems = @('spaces', 'cua-driver', 'host')
$script:Picked = $false
# Cua Spaces ships for Windows as the Electron app's NSIS setup.exe
# (cd-cua-spaces.yml). Set this to $false to skip a selected `spaces` with a note.
$script:SpacesSupported = $true
$script:SigNoted = $false
$script:DidCli = $null
$script:DidApp = $false
$script:DidDriver = $false
$script:SignedIn = $false
$script:PathHint = $null

function Get-Sha256Hex([string]$Path) {
    # .NET directly: Get-FileHash is missing when the Utility module is not
    # loaded (e.g. under `cua-driver update --apply` on older drivers).
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($Path)
    try { return ([System.BitConverter]::ToString($sha.ComputeHash($stream)) -replace '-', '').ToLowerInvariant() }
    finally { $stream.Dispose(); $sha.Dispose() }
}

# Write-Step: one plain line per thing the installer does. Write-Info: detail
# for -Verbose (manifest URLs, file names, signature checks).
function Write-Step([string]$Message) { Write-Host $Message }
function Write-Info([string]$Message) { Write-Verbose "cua-install: $Message" }
function Stop-Install([string]$Message) { throw "cua-install: error: $Message" }

# ---------------------------------------------------------------- telemetry
# Three anonymous events about this install (cua_installer_step):
# installer_started, then installer_completed or installer_failed with the
# stage that was running and a fixed reason word. The same switches as every
# Cua program (DO_NOT_TRACK, CUA_TELEMETRY, `[telemetry] enabled = "off"` in
# $CUA_HOME\config.toml, off in CI) and the same anonymous install id. Never a
# name, a path, a URL or an error message; nothing with -DryRun. Best effort:
# a send never fails the install and waits at most 3 seconds.
$script:TelKeyProd = 'phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14'
$script:TelKeyStaging = 'phc_xDNLpzypNLVGmzQeBZEgZDZK7Ea6yYAmAnGZ6hMtHNKG'
$script:TelCiVars = @('CI', 'CONTINUOUS_INTEGRATION', 'GITHUB_ACTIONS', 'GITLAB_CI', 'JENKINS_URL', 'CIRCLECI', 'BUILDKITE',
    'TF_BUILD', 'TEAMCITY_VERSION', 'TRAVIS', 'APPVEYOR', 'BITBUCKET_BUILD_NUMBER', 'CODEBUILD_BUILD_ID', 'DRONE', 'HUDSON_URL')
$script:Tel = $null
$script:TelStage = 'other'
$script:TelVersion = ''

# 'on', 'off' or $null for a switch word.
function ConvertTo-TelSwitch([string]$Value) {
    if ($null -eq $Value) { return $null }
    switch -Regex ($Value.Trim().ToLowerInvariant()) {
        '^(0|false|no|off|disable|disabled)$' { return 'off' }
        '^(1|true|yes|on|enable|enabled)$' { return 'on' }
    }
    return $null
}

function Get-TelConfigSwitch([string]$CuaHome) {
    $f = Join-Path $CuaHome 'config.toml'
    if (-not (Test-Path -LiteralPath $f)) { return $null }
    $section = ''
    foreach ($line in (Get-Content -LiteralPath $f -ErrorAction SilentlyContinue)) {
        if ($line -match '^\s*\[\s*([^\]]+?)\s*\]') { $section = $Matches[1]; continue }
        if ($section -eq 'telemetry' -and $line -match '^\s*enabled\s*=\s*"?([^"#\s]*)"?') { return (ConvertTo-TelSwitch $Matches[1]) }
    }
    return $null
}

# Whether install events may be sent here (the rules of cua-telemetry's config.rs).
function Test-TelemetryEnabled([string]$CuaHome) {
    $dnt = "$env:DO_NOT_TRACK".Trim().ToLowerInvariant()
    if ($dnt -and $dnt -ne '0' -and $dnt -ne 'false') { return $false }
    $sw = ConvertTo-TelSwitch $env:CUA_TELEMETRY
    if ($sw) { return ($sw -eq 'on') }
    if ((ConvertTo-TelSwitch $env:CUA_TELEMETRY_ENABLED) -eq 'off') { return $false }
    if ((ConvertTo-TelSwitch $env:CUA_TELEMETRY_DISABLED) -eq 'on') { return $false }
    $sw = Get-TelConfigSwitch $CuaHome
    if ($sw) { return ($sw -eq 'on') }
    return -not (Test-TelCi)
}

function Test-TelCi {
    foreach ($v in $script:TelCiVars) {
        $x = "$([Environment]::GetEnvironmentVariable($v))".Trim().ToLowerInvariant()
        if ($x -and $x -ne 'false' -and $x -ne '0') { return $true }
    }
    return $false
}

function New-TelHex {
    $b = New-Object byte[] 16
    [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($b)
    return (($b | ForEach-Object { $_.ToString('x2') }) -join '')
}

# Decides, shows the notice, and loads or creates the anonymous install id.
function Initialize-CuaTelemetry {
    $script:Tel = $null
    if ($DryRun) { return }
    $key = if ($env:CUA_POSTHOG_KEY) { $env:CUA_POSTHOG_KEY } elseif ("$env:CUA_ENV".Trim().ToLowerInvariant() -eq 'staging') { $script:TelKeyStaging } else { $script:TelKeyProd }
    if (-not $key) { return }
    $cuaHome = if ($env:CUA_HOME) { $env:CUA_HOME } else { Join-Path $HOME '.cua' }
    if (-not (Test-TelemetryEnabled $cuaHome)) { return }
    $endpoint = if ($env:CUA_TELEMETRY_ENDPOINT) { $env:CUA_TELEMETRY_ENDPOINT } else { 'https://eu.i.posthog.com/batch/' }
    if ((ConvertTo-TelSwitch $env:CUA_TELEMETRY_FORBID_NETWORK) -eq 'on' -and $endpoint -notmatch '^http://(127\.0\.0\.1|localhost|\[::1\])[:/]') { return }
    $dir = Join-Path $cuaHome 'telemetry'
    $idFile = Join-Path $dir 'install_id'
    $id = $null
    try { $id = (Get-Content -Raw -LiteralPath $idFile -ErrorAction Stop).Trim() } catch { }
    if ($id -notmatch '^[0-9a-f]{32}$') {
        $id = New-TelHex
        try {
            New-Item -ItemType Directory -Force -Path $dir | Out-Null
            Set-Content -LiteralPath $idFile -Value $id -NoNewline
        } catch { Write-Verbose "could not store the telemetry id: $_" }
    }
    $script:Tel = @{ Key = $key; Endpoint = $endpoint; Id = $id; Session = (New-TelHex); Ci = (Test-TelCi); Ended = $false }
    Write-Host 'Cua sends anonymous data about this install (whether it worked, and where it stopped if not). Never names, file paths or anything you type.'
    Write-Host 'To turn it off, set DO_NOT_TRACK=1. Details: https://cua.ai/docs/cua-sdk/concepts/telemetry'
}

# The fixed reason word for an install error message.
function Get-CuaInstallErrorKind([string]$Message) {
    switch -Wildcard ($Message) {
        '*checksum mismatch*' { return 'checksum' }
        '*signature*' { return 'signature' }
        '*release identity*' { return 'signature' }
        '*unsupported CPU*' { return 'unsupported_platform' }
        '*cannot create*' { return 'permission_denied' }
        '*ermission*' { return 'permission_denied' }
        '*Access*denied*' { return 'permission_denied' }
        '*cancelled*' { return 'cancelled' }
        '*download failed*' { return 'network' }
        '*unsupported release manifest*' { return 'network' }
        '*Unable to connect*' { return 'network' }
        '*could not be resolved*' { return 'network' }
        '*timed out*' { return 'network' }
        '*No such host*' { return 'network' }
        '*404*' { return 'network' }
    }
    return 'other'
}

# The cua_installer_step payload (one batch body).
function New-CuaInstallerStepBody($Sel, [string]$Step, [string]$Stage = 'none', [string]$Kind = 'none') {
    $failed = $Step -eq 'installer_failed'
    if (-not $failed) { $Stage = 'none'; $Kind = 'none' }
    $outcome = if (-not $failed) { 'ok' } elseif ($Kind -eq 'cancelled') { 'cancelled' } else { 'error' }
    $items = @()
    if ($Sel) {
        if ($Sel.cli) { $items += 'cli' }
        if ($Sel.spaces) { $items += 'spaces' }
        if ($Sel['cua-driver']) { $items += 'cua_driver' }
        if ($Sel.host) { $items += 'host' }
    }
    $pv = if ($script:TelVersion) { $script:TelVersion } elseif ($Version) { $Version.TrimStart('v') } else { '' }
    if ($pv -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') { $pv = '0.0.0' }
    $isWin = (-not (Test-Path variable:IsWindows)) -or $IsWindows
    $os = if ($isWin) { 'windows' } elseif ($IsMacOS) { 'macos' } elseif ($IsLinux) { 'linux' } else { 'other' }
    $major = [int][Environment]::OSVersion.Version.Major
    if ($major -gt 999 -or $major -lt 0) { $major = 0 }
    $archRaw = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } elseif ($env:PROCESSOR_ARCHITECTURE) { $env:PROCESSOR_ARCHITECTURE } else { [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() }
    $arch = switch -Regex ($archRaw) { '^(AMD64|x64|X64|x86_64)$' { 'x86_64' } '^(ARM64|Arm64|aarch64)$' { 'arm64' } default { 'other' } }
    $u = New-TelHex
    $uuid = '{0}-{1}-4{2}-8{3}-{4}' -f $u.Substring(0, 8), $u.Substring(8, 4), $u.Substring(13, 3), $u.Substring(17, 3), $u.Substring(20, 12)
    $props = [ordered]@{
        telemetry_schema_version = 1; event_version = 1; product = 'cli'; product_version = $pv
        os_family = $os; os_major = $major; arch = $arch; is_ci = [bool]$script:Tel.Ci
        is_synthetic = ((ConvertTo-TelSwitch $env:CUA_TELEMETRY_SYNTHETIC) -eq 'on'); process_session_id = $script:Tel.Session
        sample_rate = 1; '$process_person_profile' = $false; '$geoip_disable' = $true
        '$lib' = 'cua-installer'; '$lib_version' = '1.0.0'
        step = $Step; outcome = $outcome; install_stage = $Stage; error_kind = $Kind; installer = 'ps1'
        items = $(if ($items.Count) { $items -join '+' } else { 'none' })
    }
    $event = [ordered]@{ event = 'cua_installer_step'; distinct_id = $script:Tel.Id; uuid = $uuid
        timestamp = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ'); properties = $props }
    return (ConvertTo-Json -Depth 6 -Compress ([ordered]@{ api_key = $script:Tel.Key; batch = @($event) }))
}

function Send-CuaInstallerStep($Sel, [string]$Step, [string]$Stage = 'none', [string]$Kind = 'none') {
    if (-not $script:Tel -or $script:Tel.Ended) { return }
    if ($Step -ne 'installer_started') { $script:Tel.Ended = $true }
    try {
        $body = New-CuaInstallerStepBody $Sel $Step $Stage $Kind
        if ((ConvertTo-TelSwitch $env:CUA_TELEMETRY_DEBUG) -eq 'on') { Write-Host "[cua telemetry] $body" }
        Invoke-RestMethod -Method Post -Uri $script:Tel.Endpoint -ContentType 'application/json' -Body $body -TimeoutSec 3 | Out-Null
    } catch { Write-Verbose "install telemetry not sent: $_" }
}

function Get-CuaPlatform {
    param([string]$Arch = $env:CUA_INSTALL_ARCH)
    if (-not $Arch) {
        $Arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
        if (-not $Arch) { $Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() }
    }
    switch -Regex ($Arch) {
        '^(AMD64|x64|x86_64)$' { return 'windows-x64' }
        '^(ARM64|Arm64|aarch64)$' { return 'windows-arm64' }
        default { Stop-Install "unsupported CPU architecture: $Arch" }
    }
}

function Test-Interactive {
    try { return [Environment]::UserInteractive -and -not [Console]::IsInputRedirected } catch { return $false }
}

function Confirm-Step([string]$Question, [bool]$Default = $true) {
    # Answered once the checklist was confirmed.
    if ($Yes -or $script:Picked -or -not (Test-Interactive)) { return $Default }
    $hint = if ($Default) { '[Y/n]' } else { '[y/N]' }
    $answer = Read-Host "$Question $hint"
    if ($answer -match '^(y|yes)$') { return $true }
    if ($answer -match '^(n|no)$') { return $false }
    return $Default
}

# The item ids in a comma list (cli dropped), or an error.
function ConvertTo-CuaItems([string]$List) {
    $out = @()
    foreach ($item in @($List -split '[,\s]+' | Where-Object { $_ })) {
        if ($item -eq 'cli') { continue }
        if ($CuaItems -notcontains $item) { Stop-Install "unknown item '$item' (valid: cli, spaces, cua-driver, host)" }
        if ($out -notcontains $item) { $out += $item }
    }
    return $out
}

# The selection before the checklist: defaults, legacy switches, -Select/-Only.
function Get-CuaSelection {
    param([string]$SelectList = $Select, [string]$OnlyList = $Only, [bool]$Cli = [bool]$CliOnly,
        [bool]$App = [bool]$AppOnly, [string]$InstallMode = $Mode)
    if ($Cli -and $App) { Stop-Install '-CliOnly and -AppOnly are mutually exclusive' }
    if ($OnlyList -and $SelectList) { Stop-Install '-Only and -Select are mutually exclusive' }
    $extra = @(ConvertTo-CuaItems "$SelectList,$OnlyList")
    if ($Cli -and $extra.Count -and -not $Pair) { Stop-Install "-CliOnly installs only the CLI; use -Only $($extra -join ',') instead" }
    if ($App -and ($extra -contains 'cua-driver' -or $extra -contains 'host')) { Stop-Install '-AppOnly skips the CLI, which cua-driver and host need' }
    $sel = [ordered]@{ cli = -not $App; spaces = $false; 'cua-driver' = $false; host = $false }
    $list = if ($OnlyList -or $Cli) { $OnlyList } else { $SelectList }
    # The app is a default, as on macOS and Linux (not with -Only, -CliOnly
    # or a pairing code, which adds a host machine).
    if (-not ($OnlyList -or $Cli -or $Pair)) { $sel.spaces = $true }
    foreach ($item in @(ConvertTo-CuaItems $list)) { $sel[$item] = $true }
    # A pairing code adds this machine: the CLI and host setup.
    if ($Pair) {
        if ($App) { Stop-Install '-Pair needs the CLI; drop -AppOnly' }
        $sel.host = $true
    }
    # Legacy: -AppOnly and -Mode mean the app, as they always did.
    if ($App -or ($InstallMode -and -not $Cli)) { $sel.spaces = $true }
    return $sel
}

function Test-WantsChecklist {
    return (-not $Pair -and -not $Yes -and -not $Only -and -not $CliOnly -and -not $AppOnly -and
        $env:CUA_INSTALL_NONINTERACTIVE -ne '1' -and (Test-Interactive))
}

# Numbered list: what Cua Spaces is, what is always installed, then the
# optional rows. Toggle numbers, Enter to install. `host` is not a row (the
# app's first run asks how to use this computer; -Select host is for scripts).
function Read-CuaSelection($Sel, [scriptblock]$Reader = { Read-Host 'Type a number to switch an item on or off. Press Enter to install' }) {
    $rows = @()
    if ($script:SpacesSupported) { $rows += 'spaces' }
    $rows += 'cua-driver'
    $labels = @{ spaces = 'Cua Spaces app: lets your AI agents use computers'; 'cua-driver' = 'Cua Driver: lets AI agents use your apps while you keep working' }
    $help = @{
        spaces = 'Open the app to sign in, connect your AI agents and watch them work.'
        'cua-driver' = 'For advanced users. Adds it to Claude Code, Codex and other coding agents.'
    }
    Write-Host ''
    Write-Host 'Cua Spaces lets your AI agents use computers, and lets you watch them work.'
    Write-Host ''
    Write-Host 'Needed, so always installed:'
    Write-Host '  Command-line tools: the "cua" program that the app and your AI agents use.'
    if ($Sel.host) { Write-Host 'Also: your other devices will be able to use this computer (you asked for this with -Select host).' }
    Write-Host ''
    Write-Host 'Choose what else to add:'
    Write-Host ''
    while ($true) {
        for ($i = 0; $i -lt $rows.Count; $i++) {
            $box = if ($Sel[$rows[$i]]) { '[x]' } else { '[ ]' }
            Write-Host "  $($i + 1) $box $($labels[$rows[$i]])"
            Write-Host "        $($help[$rows[$i]])"
        }
        $line = & $Reader
        if (-not $line) { break }
        foreach ($tok in @($line -split '[,\s]+')) {
            $n = 0
            if ([int]::TryParse($tok, [ref]$n) -and $n -ge 1 -and $n -le $rows.Count) {
                $id = $rows[$n - 1]
                $Sel[$id] = -not $Sel[$id]
            }
        }
    }
    $script:Picked = $true
    return $Sel
}

function Get-ManifestUrl {
    if ($env:CUA_INSTALL_MANIFEST_URL) { return $env:CUA_INSTALL_MANIFEST_URL }
    if ($Version) { return "$BaseUrl/cua-sdk-v$($Version.TrimStart('v'))/release-artifacts.json" }
    return "$BaseUrl/$LatestTag/release-artifacts.json"
}

# HTTPS only. Plain HTTP is allowed only to a loopback host (tests, local
# mirrors); file:// only when the manifest itself is a local file.
function Assert-DownloadUrl([string]$Url, [string]$ManifestUrl = '') {
    $uri = $null
    if (-not [Uri]::TryCreate($Url, [UriKind]::Absolute, [ref]$uri) -or $Url -match '\s') {
        Stop-Install "refusing a malformed download URL: $Url"
    }
    if ($uri.Scheme -eq 'https') { return }
    if ($uri.Scheme -eq 'http' -and $uri.IsLoopback -and -not $uri.UserInfo) { return }
    if ($uri.Scheme -eq 'file') {
        if ($ManifestUrl -and $ManifestUrl -notlike 'file://*') { Stop-Install "refusing a file:// URL from a remote manifest: $Url" }
        return
    }
    Stop-Install "refusing a non-HTTPS download: $Url"
}

function Get-Remote([string]$Url, [string]$Dest, [string]$ManifestUrl = '') {
    Assert-DownloadUrl $Url $ManifestUrl
    if ($Url -like 'file://*') {
        Copy-Item -LiteralPath ([Uri]$Url).LocalPath -Destination $Dest
        return
    }
    try {
        # Windows PowerShell 5.1 may default to TLS 1.0/1.1.
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        $resp = Invoke-WebRequest -Uri $Url -OutFile $Dest -UseBasicParsing -PassThru
        # The final URL after redirects: ResponseUri (5.1), RequestMessage (7+).
        $br = $resp.BaseResponse
        $final = $null
        if ($br.PSObject.Properties['ResponseUri']) { $final = $br.ResponseUri }
        elseif ($br.PSObject.Properties['RequestMessage'] -and $br.RequestMessage) { $final = $br.RequestMessage.RequestUri }
        if ($final -and $final.Scheme -ne 'https' -and -not ($final.Scheme -eq 'http' -and $final.IsLoopback)) {
            Remove-Item -Force -LiteralPath $Dest -ErrorAction SilentlyContinue
            Stop-Install "refusing a redirect to a non-HTTPS URL: $Url"
        }
    } catch {
        Stop-Install "download failed: $Url ($($_.Exception.Message))"
    }
}

function Resolve-ArtifactUrl([string]$Value, [string]$ManifestUrl) {
    if ($Value -match '://') { return $Value }
    $dir = $ManifestUrl.Substring(0, $ManifestUrl.LastIndexOf('/'))
    return "$dir/$Value"
}

function Find-Artifact($Manifest, [string]$Component, [string]$Platform, [string]$Kind) {
    $hit = @($Manifest.artifacts | Where-Object { $_.component -eq $Component -and $_.platform -eq $Platform -and $_.kind -eq $Kind })
    if ($hit.Count -eq 0) { return $null }
    return $hit[0]
}

# The exact Sigstore identity that must have signed a manifest entry: the CLI
# by cd-cua-sdk.yml at refs/tags/cua-sdk-v<version>, the app by
# cd-cua-spaces.yml at refs/tags/cua-spaces-v<version>.
function Get-CosignIdentity($Entry) {
    $v = [string]$Entry.version
    if ($v -notmatch '^[0-9A-Za-z.+-]+$') { return $null }
    switch ([string]$Entry.component) {
        'cli' { return "https://github.com/$Repo/.github/workflows/cd-cua-sdk.yml@refs/tags/cua-sdk-v$v" }
        'app' { return "https://github.com/$Repo/.github/workflows/cd-cua-spaces.yml@refs/tags/cua-spaces-v$v" }
        default { return $null }
    }
}

function Test-Signature([string]$File, $Entry, [string]$ManifestUrl) {
    $ok = $false
    $minisign = Get-Command minisign -ErrorAction SilentlyContinue
    if ($Entry.PSObject.Properties['minisig'] -and $MinisignPubkey -and $minisign) {
        Get-Remote (Resolve-ArtifactUrl $Entry.minisig $ManifestUrl) "$File.minisig" $ManifestUrl
        & $minisign.Source -Vqm $File -x "$File.minisig" -P $MinisignPubkey | Out-Null
        if ($LASTEXITCODE -ne 0) { Stop-Install "minisign signature check failed for $(Split-Path -Leaf $File)" }
        $ok = $true
    }
    $cosign = Get-Command cosign -ErrorAction SilentlyContinue
    if (-not $ok -and $Entry.PSObject.Properties['cosign_bundle'] -and $cosign) {
        Get-Remote (Resolve-ArtifactUrl $Entry.cosign_bundle $ManifestUrl) "$File.sigstore.json" $ManifestUrl
        $identity = Get-CosignIdentity $Entry
        if (-not $identity) { Stop-Install "no release identity for $(Split-Path -Leaf $File) in the manifest" }
        & $cosign.Source verify-blob --bundle "$File.sigstore.json" `
            --certificate-identity $identity `
            --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' $File *> $null
        if ($LASTEXITCODE -ne 0) { Stop-Install "cosign signature check failed for $(Split-Path -Leaf $File)" }
        $ok = $true
    }
    if (-not $ok -and $RequireSignature) {
        Stop-Install "no verifiable signature for $(Split-Path -Leaf $File) (install minisign or cosign)"
    }
    if (-not $ok -and ($Entry.PSObject.Properties['minisig'] -or $Entry.PSObject.Properties['cosign_bundle'])) {
        # sha256 is checked either way; the closing summary says so. The
        # detail is for -Verbose.
        if (-not $script:SigNoted) {
            $script:SigNoted = $true
            Write-Info 'release signatures were not checked: neither minisign nor cosign is installed (-RequireSignature makes that an error)'
        }
    }
}

function Get-Artifact($Manifest, [string]$ManifestUrl, [string]$Component, [string]$Platform, [string]$Kind, [string]$Tmp) {
    $entry = Find-Artifact $Manifest $Component $Platform $Kind
    if (-not $entry) { Stop-Install "no $Component $Kind artifact for $Platform in the release manifest" }
    if ($entry.name -match '[\\/]' -or $entry.name.StartsWith('.')) { Stop-Install "bad artifact name in manifest: $($entry.name)" }
    $url = Resolve-ArtifactUrl $entry.url $ManifestUrl
    $dest = Join-Path $Tmp $entry.name
    if ($DryRun) {
        Write-Host "[dry-run] download $url (sha256 $($entry.sha256))"
        return $dest
    }
    Write-Info "downloading $($entry.name)"
    Get-Remote $url $dest $ManifestUrl
    $actual = Get-Sha256Hex $dest
    if ($actual -ne $entry.sha256.ToLowerInvariant()) {
        Stop-Install "checksum mismatch for $($entry.name) (the download is damaged or was changed; expected $($entry.sha256), got $actual); nothing was installed"
    }
    Test-Signature $dest $entry $ManifestUrl
    return $dest
}

function Get-CliBinDir {
    if ($Prefix) { return (Join-Path $Prefix 'bin') }
    $base = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { Join-Path $HOME 'AppData\Local' }
    return (Join-Path $base 'Programs\cua\bin')
}

function Add-UserPath([string]$Dir) {
    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    $parts = @(if ($current) { $current -split ';' | Where-Object { $_ } })
    if ($DryRun) { Write-Host "[dry-run] add $Dir to the user PATH"; return }
    # This terminal too, so `cua` works right away.
    if (@($env:Path -split ';') -notcontains $Dir) { $env:Path = "$env:Path;$Dir" }
    if ($parts -contains $Dir) { return }
    [Environment]::SetEnvironmentVariable('Path', (($parts + $Dir) -join ';'), 'User')
    Write-Step "Added $Dir to your user PATH (open a new terminal window to use cua)"
}

function Install-Cli($Manifest, [string]$ManifestUrl, [string]$Platform, [string]$Tmp) {
    $bin = Get-CliBinDir
    $target = Join-Path $bin 'cua.exe'
    # -CliOnly / -Only already said what to install.
    if (-not ($CliOnly -or $Only) -and -not (Confirm-Step "Install the command-line tools to ${target}?")) { Stop-Install 'cancelled; rerun with -Prefix DIR to choose another location' }
    Write-Step "Installing the command-line tools to $target"
    $zip = Get-Artifact $Manifest $ManifestUrl 'cli' $Platform 'zip' $Tmp
    if ($DryRun) {
        Write-Host "[dry-run] install cua.exe -> $target"
    } else {
        $out = Join-Path $Tmp 'cli'
        Expand-Archive -LiteralPath $zip -DestinationPath $out -Force
        $exe = Get-ChildItem -LiteralPath $out -Recurse -Filter 'cua.exe' | Select-Object -First 1
        if (-not $exe) { Stop-Install "no cua.exe inside $(Split-Path -Leaf $zip)" }
        New-Item -ItemType Directory -Force -Path $bin | Out-Null
        Copy-Item -LiteralPath $exe.FullName -Destination $target -Force
        $script:DidCli = $target
    }
    $onPath = @($env:Path -split ';') -contains $bin
    if (-not $onPath) {
        if (-not $NoModifyPath) { Add-UserPath $bin }
        else {
            # Said at the end, after what was installed.
            $script:PathHint = @(
                "$bin is not on your PATH, the list of places your terminal looks for programs.",
                "To use cua, run this, then open a new terminal window:",
                "  [Environment]::SetEnvironmentVariable('Path', `"`$([Environment]::GetEnvironmentVariable('Path','User'));$bin`", 'User')"
            )
        }
    }
    return $target
}

function Get-AppInstallCommand([string]$File, [string]$Kind, [string]$InstallMode) {
    if ($Kind -eq 'msi') {
        $msiArgs = @('/i', "`"$File`"", '/qn', '/norestart')
        if ($InstallMode) { $msiArgs += "CUA_SPACES_MODE=$InstallMode" }
        return @{ FilePath = 'msiexec.exe'; ArgumentList = $msiArgs }
    }
    $nsisArgs = @('/S')
    if ($InstallMode) { $nsisArgs += "/MODE=$InstallMode" }
    return @{ FilePath = $File; ArgumentList = $nsisArgs }
}

function Install-App($Manifest, [string]$ManifestUrl, [string]$Platform, [string]$Tmp) {
    if (-not (Confirm-Step 'Install the Cua Spaces app?')) { Write-Step 'Skipping the Cua Spaces app'; return }
    Write-Step 'Installing the Cua Spaces app'

    $kind = $Installer
    if (-not (Find-Artifact $Manifest 'app' $Platform $kind)) {
        $kind = if ($kind -eq 'msi') { 'nsis' } else { 'msi' }
    }
    $file = Get-Artifact $Manifest $ManifestUrl 'app' $Platform $kind $Tmp
    $cmd = Get-AppInstallCommand $file $kind $Mode
    if ($DryRun) {
        Write-Host "[dry-run] $($cmd.FilePath) $($cmd.ArgumentList -join ' ')"
        return
    }
    if ($kind -eq 'nsis') {
        try {
            $sig = Get-AuthenticodeSignature -LiteralPath $file -ErrorAction Stop
        } catch {
            Stop-Install "Cua Spaces Authenticode signature verification failed: $($_.Exception.Message); refusing to run installer."
        }
        if ($sig.Status -ne 'Valid') {
            Stop-Install "Cua Spaces Authenticode signature is not valid ($($sig.Status): $($sig.StatusMessage)); refusing to run installer."
        }
        if (-not $sig.SignerCertificate -or
            $sig.SignerCertificate.GetNameInfo([System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false) -cne 'Cua AI, Inc.') {
            Stop-Install 'Cua Spaces Authenticode signature signer is not Cua AI, Inc.; refusing to run installer.'
        }
        Write-Info "verified the Authenticode signature of $(Split-Path -Leaf $file) (Cua AI, Inc.)"
    }
    Write-Info "installing Cua Spaces ($kind, silent)"
    $proc = Start-Process -FilePath $cmd.FilePath -ArgumentList $cmd.ArgumentList -Wait -PassThru
    if ($proc.ExitCode -ne 0 -and $proc.ExitCode -ne 3010) { Stop-Install "Cua Spaces installer exited with $($proc.ExitCode)" }
    $script:DidApp = $true
}

function Get-DriverBinDir {
    if ($Prefix) { return (Join-Path $Prefix 'bin') }
    if ($env:CUA_DRIVER_RS_INSTALL_DIR) { return $env:CUA_DRIVER_RS_INSTALL_DIR }
    $base = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { Join-Path $HOME 'AppData\Local' }
    return (Join-Path $base 'Programs\Cua\cua-driver\bin')
}

function Get-DriverInstallArgs {
    $a = @()
    if ($RequireSignature) { $a += '-RequireSignature' }
    if ($NoModifyPath) { $a += '-NoPathUpdate' }
    return $a
}

function Get-PowerShellExe {
    $p = (Get-Process -Id $PID).Path
    if ($p -and (Split-Path -Leaf $p) -match '^(pwsh|powershell)(\.exe)?$') { return $p }
    if (Get-Command pwsh -ErrorAction SilentlyContinue) { return 'pwsh' }
    return 'powershell'
}

# cua-driver's own release installer (it checks each zip against the release's
# SHA256SUMS, its Sigstore bundle and the Authenticode signature), then the
# cua-driver skill and MCP server for your agents.
function Install-CuaDriver([string]$Cua, [string]$Tmp, [string]$ManifestUrl) {
    $url = if ($env:CUA_INSTALL_DRIVER_URL) { $env:CUA_INSTALL_DRIVER_URL } else { 'https://cua.ai/driver/install.ps1' }
    $script = Join-Path $Tmp 'cua-driver-install.ps1'
    $driverArgs = @(Get-DriverInstallArgs)
    $binDir = Get-DriverBinDir
    $exe = Get-PowerShellExe
    if ($DryRun) {
        Write-Host "[dry-run] download $url"
        Write-Host "[dry-run] $exe -NoProfile -ExecutionPolicy Bypass -File cua-driver-install.ps1 $($driverArgs -join ' ')"
    } else {
        Write-Step 'Installing Cua Driver and adding it to your AI agents'
        Get-Remote $url $script $ManifestUrl
        $saved = $env:CUA_DRIVER_RS_INSTALL_DIR
        if ($Prefix) { $env:CUA_DRIVER_RS_INSTALL_DIR = $binDir }
        try { & $exe -NoProfile -ExecutionPolicy Bypass -File $script @driverArgs } finally { $env:CUA_DRIVER_RS_INSTALL_DIR = $saved }
        if ($LASTEXITCODE -ne 0) { Stop-Install "the cua-driver installer failed (exit $LASTEXITCODE)" }
    }
    if ($NoSkills) { Write-Info 'adding the cua-driver MCP server to your agents (no skill: -NoSkills)' }
    else { Write-Info 'adding the cua-driver skill and MCP server to your agents' }
    $setup = @('agents', 'setup', '--cua-driver', '--agents', 'all', '--yes')
    if ($NoSkills) { $setup += '--no-skills' }
    $driver = Join-Path $binDir 'cua-driver.exe'
    if ($DryRun -or (Test-Path -LiteralPath $driver)) { $setup += @('--mcp-command', $driver) }
    if ($DryRun) { Write-Host "[dry-run] $Cua $($setup -join ' ')"; return }
    & $Cua @setup
    if ($LASTEXITCODE -ne 0) { Write-Warning 'agent setup for cua-driver did not finish; rerun: cua agents setup --cua-driver' }
    $script:DidDriver = $true
}

# Checks -Pair / -Relay: they end up on a command line.
function Test-PairArgs {
    if ($Pair -and $Pair -notmatch '^[A-Za-z0-9-]+$') { Stop-Install "-Pair: '$Pair' is not a pairing code (like K7QM-4XPB-2RHD)" }
    if ($Relay) {
        if (-not $Pair) { Stop-Install '-Relay goes with -Pair' }
        if ($Relay -notmatch '^(https://[A-Za-z0-9.-]+(:[0-9]+)?|http://(localhost|127\.0\.0\.1)(:[0-9]+)?)(/[A-Za-z0-9._%/-]*)?$') { Stop-Install '-Relay must be a plain https:// URL (plain http only for localhost)' }
    }
}

# Add this machine with a pairing code: joins the account (or team) that
# made it, with no sign-in here.
function Invoke-HostPair([string]$Cua) {
    $pairArgs = @('host', 'setup', '--pair', $Pair)
    if ($Relay) { $pairArgs += @('--relay', $Relay) }
    if ($DryRun) { Write-Host "[dry-run] $Cua $($pairArgs -join ' ')"; return }
    # A CLI released before pairing exists would fail with a bare
    # "unexpected argument '--pair'"; say what to do instead.
    $setupHelp = (& $Cua host setup --help 2>&1 | Out-String)
    if ($setupHelp -notmatch '--pair') {
        $have = (& $Cua --version 2>&1 | Out-String).Trim()
        Stop-Install "the installed cua CLI ($have) cannot add a machine with a code (no 'cua host setup --pair'); install a newer release (-Version) or sign in instead: cua auth login; cua host setup"
    }
    Write-Step 'Adding this computer to your account (cua host setup --pair)'
    $null | & $Cua @pairArgs
    if ($LASTEXITCODE -ne 0) { Stop-Install 'adding this machine did not finish; if the code expired or was used, make a new one in Cua Spaces (Add a machine) and run: cua host setup --pair <code>' }
    Write-Host ''
    Write-Host 'This machine is added. It shows up in Cua Spaces on your other devices in a few seconds.'
}

# Host this machine: joins the relay as the signed-in account.
function Invoke-HostSetup([string]$Cua) {
    if ($Pair) { Invoke-HostPair $Cua; return }
    if ($DryRun) { Write-Host "[dry-run] $Cua host setup"; return }
    Write-Step 'Letting your other devices use this computer (cua host setup)'
    if (Test-Interactive) { & $Cua host setup } else { $null | & $Cua host setup }
    if ($LASTEXITCODE -ne 0) { Write-Warning "host setup did not finish; run 'cua host setup' after 'cua auth login'" }
}

# -Pair on a computer that already has Cua: the `cua` that is there (it must
# know `host setup --pair`), or $null. -Prefix, -Version and
# CUA_INSTALL_REINSTALL=1 install anyway.
function Find-ExistingCua {
    if (-not $Pair -or $Prefix -or $Version -or $env:CUA_INSTALL_REINSTALL -eq '1') { return $null }
    $candidates = @()
    $onPath = Get-Command cua -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($onPath) { $candidates += $onPath.Source }
    if ($env:LOCALAPPDATA) { $candidates += (Join-Path $env:LOCALAPPDATA 'Programs\cua\bin\cua.exe') }
    foreach ($candidate in $candidates) {
        if (-not (Test-Path -LiteralPath $candidate)) { continue }
        $help = (& $candidate host setup --help 2>&1 | Out-String)
        if ($help -match '--pair') { return $candidate }
    }
    return $null
}

# "a, b and c" from a list of phrases.
function Join-And([string[]]$Items) {
    if ($Items.Count -le 1) { return ($Items -join '') }
    if ($Items.Count -eq 2) { return "$($Items[0]) and $($Items[1])" }
    return (($Items[0..($Items.Count - 2)] -join ', ') + " and $($Items[-1])")
}

# The closing lines: what was installed, then what to do next, in plain words.
function Write-CuaSummary($Sel) {
    if ($DryRun -or $Pair) { return }
    $did = @()
    if ($script:DidCli) { $did += 'the command-line tools' }
    if ($script:DidApp) { $did += 'the Cua Spaces app' }
    if ($script:DidDriver) { $did += 'Cua Driver' }
    Write-Host ''
    if ($did.Count -gt 0) {
        $checked = if ($script:DidCli -or $script:DidApp) { ' (downloads checked)' } else { '' }
        Write-Host "Installed $(Join-And $did)$checked."
    }
    if ($script:DidApp) {
        if ($script:SignedIn) { Write-Host 'Next: open Cua Spaces from the Start menu. There you can also let your other devices use this computer.' }
        else { Write-Host 'Next: open Cua Spaces from the Start menu. It guides you through signing in and connecting your AI agents. You can also let your other devices use this computer.' }
        if ($Sel.cli) {
            if ($script:SignedIn) { Write-Host 'To connect more AI agents later, run "cua agents setup" in a terminal.' }
            else { Write-Host 'Prefer a terminal? Run "cua auth login" to sign in, then "cua agents setup" to connect your AI agents.' }
        }
    } elseif ($Sel.cli) {
        if ($script:SignedIn) { Write-Host 'Next: run "cua agents setup" to connect your AI agents.' }
        else {
            Write-Host 'Next: run "cua auth login" to sign in, then "cua agents setup" to connect your AI agents.'
            Write-Host '(No web browser on this computer? Run "cua auth login --remote" instead.)'
        }
        if (-not $Sel.host) { Write-Host 'To let your other devices use this computer, run "cua host setup".' }
    }
}

function Invoke-CuaInstall {
    # Function scope: `irm | iex` runs in the caller's session, so keep these local.
    Set-StrictMode -Version 3
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    Test-PairArgs
    $sel = Get-CuaSelection
    Initialize-CuaTelemetry
    $script:TelStage = 'platform'
    try {
        Invoke-CuaInstallSteps $sel
        $script:TelStage = 'other'
        Send-CuaInstallerStep $sel 'installer_completed'
    } catch {
        Send-CuaInstallerStep $sel 'installer_failed' $script:TelStage (Get-CuaInstallErrorKind "$_")
        throw
    } finally {
        # Stopped without an error (Ctrl+C): cancelled.
        Send-CuaInstallerStep $sel 'installer_failed' $script:TelStage 'cancelled'
    }
}

function Invoke-CuaInstallSteps($sel) {
    if ($sel.spaces -and -not $script:SpacesSupported) {
        Write-Step 'Cua Spaces is not available on this platform; skipping the app'
        $sel.spaces = $false
    }
    if ($Version -and $Version.TrimStart('v') -notmatch '^\d+\.\d+\.\d+') { Stop-Install "-Version must look like 1.2.3, not '$Version'" }
    $platform = Get-CuaPlatform
    Send-CuaInstallerStep $sel 'installer_started'
    if (Test-WantsChecklist) {
        $picked = Read-CuaSelection $sel
        foreach ($k in @($picked.Keys)) { $sel[$k] = $picked[$k] }
    }
    if ($DryRun) { Write-Step 'Dry run: nothing will be changed' }
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("cua-install-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    try {
        $cua = $null
        $existing = Find-ExistingCua
        if ($existing) {
            Write-Step "cua is already installed at $existing; using it (CUA_INSTALL_REINSTALL=1 installs it again)"
            $sel.cli = $false
            $cua = $existing
        }
        $manifestUrl = $null
        # Nothing to download (-Pair on a computer that has Cua): no manifest to read.
        if ($sel.cli -or $sel.spaces -or $sel['cua-driver']) {
            $script:TelStage = 'manifest'
            $manifestUrl = Get-ManifestUrl
            $manifestFile = Join-Path $tmp 'release-artifacts.json'
            Get-Remote $manifestUrl $manifestFile
            $manifest = Get-Content -Raw -LiteralPath $manifestFile | ConvertFrom-Json
            if ($manifest.schema -ne 1) { Stop-Install "unsupported release manifest at $manifestUrl" }
            Write-Info "platform $platform, manifest $manifestUrl"
            $script:TelVersion = "$(@($manifest.artifacts | Where-Object { $_.component -eq 'cli' } | Select-Object -First 1).version)"
        }
        $script:TelStage = 'cli'
        if ($sel.cli) { $cua = Install-Cli $manifest $manifestUrl $platform $tmp }
        if ($sel.spaces) {
            $script:TelStage = 'app'
            if ($Mode) {
                $cuaHome = if ($env:CUA_HOME) { $env:CUA_HOME } else { Join-Path $HOME '.cua' }
                $modeFile = Join-Path $cuaHome 'spaces-install-mode'
                if ($DryRun) { Write-Host "[dry-run] write $modeFile ($Mode)" }
                else {
                    New-Item -ItemType Directory -Force -Path $cuaHome | Out-Null
                    Set-Content -LiteralPath $modeFile -Value $Mode -NoNewline
                }
            }
            Install-App $manifest $manifestUrl $platform $tmp
        }
        # How Cua was installed, for the one-time first-run telemetry event
        # (a fixed enum value; no identifiers). Kept when already recorded.
        $telemetryDir = Join-Path $(if ($env:CUA_HOME) { $env:CUA_HOME } else { Join-Path $HOME '.cua' }) 'telemetry'
        $channelFile = Join-Path $telemetryDir 'install_channel'
        if ($DryRun) { Write-Host "[dry-run] record the install channel in $channelFile" }
        elseif (-not (Test-Path -LiteralPath $channelFile)) {
            try {
                New-Item -ItemType Directory -Force -Path $telemetryDir | Out-Null
                Set-Content -LiteralPath $channelFile -Value 'install_script'
            } catch { Write-Verbose "could not record the install channel: $_" }
        }
        $env:CUA_INSTALL_CHANNEL = 'install_script'
        $script:TelStage = 'driver'
        if ($sel['cua-driver']) { Install-CuaDriver $cua $tmp $manifestUrl }
        $script:TelStage = 'sign_in'
        if ($cua -and -not $Pair -and -not $CliOnly -and -not $NoOnboarding -and -not $DryRun -and (Test-Interactive)) {
            Write-Host ''
            Write-Step 'Signing in to Cua'
            & $cua auth login
            if ($LASTEXITCODE -ne 0) { Write-Warning 'Sign-in did not finish. Run "cua auth login" later (add --remote if this computer has no web browser).' } else { $script:SignedIn = $true }
        }
        $script:TelStage = 'host'
        if ($sel.host) { Invoke-HostSetup $cua }
        Write-CuaSummary $sel
        if ($script:PathHint) {
            Write-Host ''
            $script:PathHint | ForEach-Object { Write-Host $_ }
        }
    } finally {
        Remove-Item -Recurse -Force -LiteralPath $tmp -ErrorAction SilentlyContinue
    }
}

# Dot-sourcing with CUA_INSTALL_NO_RUN=1 loads the functions for tests.
if ($env:CUA_INSTALL_NO_RUN -ne '1') {
    Invoke-CuaInstall
}
