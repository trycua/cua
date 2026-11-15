# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

<#
.SYNOPSIS
Builds Cua Spaces from this checkout and installs it as "Cua Spaces Local",
next to a released Cua Spaces: another app id and AppUserModelID, app folder,
executable, CLI (`cua-local`), cua home (`%USERPROFILE%\.cua-local`), channel
(ports, names, sign-in), URL scheme (`cua-local://`), user data folder and no
update feed. The Windows counterpart of
apps/cua-spaces-macos/scripts/install-local.sh.

.DESCRIPTION
  scripts\install-local.ps1 [-Release] [-NoLaunch] [-Prefix DIR]
                            [-SkipRustBuild] [-SkipWebBuild]
                            [-NoIntegration] [-NormalPriority]

  -Release          build with the Electron fuses the release build flips
                    (default: fuses off, so the app can be debugged with
                    --inspect, as CUA_SPACES_NO_FUSES=1 does)
  -NoLaunch         do not start the app when it is installed
  -Prefix DIR       install under DIR\Programs instead of %LOCALAPPDATA%\Programs.
                    DIR is resolved (`..`, links) and refused when the app
                    would be, hold, or be inside anything the released Cua
                    Spaces owns (Programs\cua-spaces, Programs\Cua Spaces,
                    Programs\Cua, %USERPROFILE%\.cua, Program Files\Cua Spaces)
  -SkipRustBuild    do not run cargo: use native\win32-<arch> as it is
  -SkipWebBuild     do not build the web UI (apps\cua-spaces-web): use the
                    build already in its dist\, or the one $env:CUA_WEBUI_DIST
                    names. Stops when there is none.
  -NoIntegration    do not touch the Start Menu, the user PATH or the
                    cua-local:// registration (the files only)
  -NormalPriority   builds run at BelowNormal priority by default, so a PC in
                    use stays usable; this keeps the normal one

Needs (checked first; one message says what to install): Rust (rustup) with the
MSVC toolchain and the Visual Studio C++ Build Tools, CMake (the audio codec
build), Node 22 or later, pnpm, the wasm32 Rust target and wasm-bindgen-cli
(the versions the release build pins).

What it installs, and nothing else (default paths):
  %LOCALAPPDATA%\Programs\Cua Spaces Local\            the app
  %LOCALAPPDATA%\Programs\Cua Spaces Local\bin\cua-local.exe   the CLI (a copy of the app's cua.exe)
  the user PATH entry for that bin folder
  %APPDATA%\Microsoft\Windows\Start Menu\Programs\Cua Spaces Local.lnk
  HKCU\Software\Classes\cua-local                      the cua-local:// scheme

What it never touches: the released Cua Spaces and its `cua`, %USERPROFILE%\.cua,
their Run entry, scheme and PATH entry, and any folder at the Local path that is
not the Local one (it refuses). The build is staged and verified first; the live
copy is swapped only then, and put back if the swap fails.

The app sets CUA_CHANNEL=local and CUA_HOME=%USERPROFILE%\.cua-local at start
(src\channel.ts), so the daemon and every child inherit them; a variable that is
already set wins. Local talks to staging (CUA_ENV=prod opts in).

Uninstall: scripts\uninstall-local.ps1.
#>
[CmdletBinding()]
param(
    [switch]$Release,
    [switch]$NoLaunch,
    [string]$Prefix = '',
    [switch]$SkipRustBuild,
    [switch]$SkipWebBuild,
    [switch]$NoIntegration,
    [switch]$NormalPriority
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $ScriptDir '_local-common.ps1')

$AppDir = (Resolve-Path (Join-Path $ScriptDir '..')).Path
$RepoRoot = (Resolve-Path (Join-Path $AppDir '..\..')).Path
$WebApp = Join-Path $RepoRoot 'apps\cua-spaces-web'
$WasmScript = Join-Path $WebApp 'scripts\build-core-wasm.mjs'
$WasmToolchainFile = Join-Path $RepoRoot 'apps\cua-spaces\rust-toolchain.toml'
$Arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }
$NativeDir = Join-Path $AppDir "native\win32-$Arch"
$BuilderConfig = 'packaging/electron-builder.local.cjs'
$BuildOut = Join-Path $AppDir 'dist-local'

function Stop-Install([string]$Message) { Write-Err $Message; exit 1 }

# Runs a native program; a non-zero exit stops the script. (Windows PowerShell
# 5.1 turns a program's stderr into errors under 'Stop', so it is relaxed here.)
function Invoke-Native([string]$What, [string]$File, [string[]]$Arguments, [string]$WorkDir) {
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    Push-Location $WorkDir
    try { & $File @Arguments | Out-Host; $code = $LASTEXITCODE } finally { Pop-Location; $ErrorActionPreference = $previous }
    if ($code -ne 0) { Stop-Install "$What failed (exit $code). Nothing was installed." }
}

# The pnpm this checkout pins (package.json "packageManager"). pnpm 10 cannot read pnpm 11's
# lockfile and its self-switch can leave a broken copy, so a pnpm of another major than the pin
# is not used: the pinned one is installed into
# .local-tools (npm, a folder of this checkout; nothing global changes).
function Resolve-Pnpm {
    $pin = (Get-Content (Join-Path $AppDir 'package.json') -Raw | ConvertFrom-Json).packageManager
    if ($pin -notmatch '^pnpm@(\d+)\.[\d.]+') { return 'pnpm' }
    $version = ($pin -replace '^pnpm@', '')
    $major = $Matches[1]
    $have = ''
    if (Test-Tool 'pnpm') { $have = ((& pnpm --version 2>$null) | Out-String).Trim() }
    if ($have -like "$major.*") { return 'pnpm' }
    $private = Join-Path $AppDir ".local-tools\pnpm-$version"
    $exe = Join-Path $private 'node_modules\.bin\pnpm.cmd'
    if (-not (Test-Path $exe)) {
        Write-Step "installing pnpm $version (this checkout pins it; this PC has pnpm $have) into $private"
        New-Item -ItemType Directory -Path $private -Force | Out-Null
        Invoke-Native 'npm install pnpm' 'npm' @('install', '--prefix', $private, "pnpm@$version", '--no-audit', '--no-fund', '--loglevel=error') $AppDir
    }
    return $exe
}

if ($env:OS -ne 'Windows_NT') { Stop-Install 'Cua Spaces Local (this script) is for Windows.' }
if (-not $NormalPriority) {
    # Children inherit it: cargo, node and the compilers stay out of the way.
    try { (Get-Process -Id $PID).PriorityClass = 'BelowNormal' } catch { Write-Warn "could not lower the priority: $($_.Exception.Message)" }
}

# --- Where it goes -------------------------------------------------------------

if (-not $Prefix) { $Prefix = $env:LOCALAPPDATA }
if (-not [System.IO.Path]::IsPathRooted($Prefix)) { Stop-Install "-Prefix must be an absolute path (got: $Prefix)" }
$ProgramsDir = Resolve-RealPath (Join-Path $Prefix 'Programs')
$InstallDir = Join-Path $ProgramsDir $Local.AppDirName
$StageDir = Join-Path $ProgramsDir ($Local.AppDirName + '.new')
$OldDir = Join-Path $ProgramsDir ($Local.AppDirName + '.old')
$BinDir = Join-Path $InstallDir 'bin'
$CliPath = Join-Path $BinDir $Local.CliExeName
$ExePath = Join-Path $InstallDir $Local.ExeName
$ShortcutPath = Join-Path $env:APPDATA ('Microsoft\Windows\Start Menu\Programs\' + $Local.ShortcutName)

# None of what this script writes is, holds, or is inside anything the
# released Cua Spaces owns.
$roots = Get-ReleasedRoots $env:LOCALAPPDATA $env:USERPROFILE $env:ProgramFiles
foreach ($p in @($InstallDir, $StageDir, $OldDir)) {
    $hit = Find-ReleasedOverlap $p $roots
    if ($hit) { Stop-Install "refusing to use ${p}: it is, holds or is inside $hit, which the released Cua Spaces owns." }
}
if ((Split-Path -Leaf $InstallDir) -ne $Local.AppDirName) { Stop-Install "internal error: app path $InstallDir" }

# --- Preflight: nothing is built or changed when the Local path holds something else ---

foreach ($p in @($InstallDir, $StageDir, $OldDir)) {
    $state = Get-InstallState $p
    if ($state -eq 'foreign') {
        # Our own leftovers (a failed earlier run) are removed; anything else is refused.
        $isLeftover = ($p -ne $InstallDir) -and (Test-Path -LiteralPath (Join-Path $p $Local.ExeName))
        if (-not $isLeftover) {
            Stop-Install "refusing to touch ${p}: it is not a Cua Spaces Local install (no $($Local.MarkerName) with the Local app id). Move it away or pass another -Prefix."
        }
    }
}

# --- Preflight: the tools the build needs --------------------------------------

$Missing = New-Object System.Collections.Generic.List[string]
function Add-Missing([string]$Text) { $Missing.Add($Text) }
function Test-Tool([string]$Name) { return [bool](Get-Command $Name -ErrorAction SilentlyContinue) }

$BuildWeb = (-not $SkipWebBuild) -and (-not $env:CUA_WEBUI_DIST)
$WebDist = if ($env:CUA_WEBUI_DIST) { $env:CUA_WEBUI_DIST } else { Join-Path $WebApp 'dist' }

function Test-Prerequisites {
    # A shell that did not get rustup's PATH entry still has cargo here.
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if ((Test-Path $cargoBin) -and ($env:PATH -notlike "*$cargoBin*")) { $env:PATH = "$cargoBin;$env:PATH" }

    if (-not $SkipRustBuild) {
        if (-not (Test-Tool 'cmake')) { Add-Missing 'CMake (the audio codec build needs it): winget install Kitware.CMake' }
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
        $hasMsvc = $false
        if (Test-Path $vswhere) {
            $found = & $vswhere -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
            $hasMsvc = [bool]$found
        }
        if (-not $hasMsvc -and -not (Test-Tool 'cl')) {
            Add-Missing 'the Visual Studio C++ Build Tools (the "Desktop development with C++" workload, MSVC and a Windows SDK): winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"'
        }
    }
    if ((-not $SkipRustBuild) -or $BuildWeb) {
        if (-not (Test-Tool 'cargo') -or -not (Test-Tool 'rustup')) {
            Add-Missing 'Rust (cargo and rustup), MSVC host: winget install Rustlang.Rustup   (then open a new terminal)'
        }
    }
    # The shell itself (electron-builder, tsdown) always needs Node and pnpm.
    if (-not (Test-Tool 'node')) {
        Add-Missing 'Node 22 or later: winget install OpenJS.NodeJS'
    } elseif (-not (Test-NodeVersionOk (& node --version))) {
        Add-Missing "Node 22 or later, this PC has $(& node --version): winget install OpenJS.NodeJS"
    }
    if (-not (Test-Tool 'pnpm')) { Add-Missing 'pnpm: corepack enable   (or: winget install pnpm.pnpm)' }
    if ($BuildWeb) {
        # The web UI's app core is Rust built for the browser (WebAssembly), with the
        # toolchain and the wasm-bindgen the release build pins.
        $toolchain = Get-ToolchainChannel $WasmToolchainFile
        if ($toolchain -and (Test-Tool 'rustup')) {
            $targets = & rustup target list --installed --toolchain $toolchain 2>$null
            if (-not ($targets -contains 'wasm32-unknown-unknown')) {
                Add-Missing "the wasm32 Rust target for Rust $toolchain (the web UI): rustup toolchain install $toolchain --profile minimal --target wasm32-unknown-unknown"
            }
        }
        $pin = Get-WasmBindgenPin $WasmScript
        if ($pin) {
            $have = if (Test-Tool 'wasm-bindgen') { (& wasm-bindgen --version) } else { '' }
            if ("$have" -notlike "*$pin*") { Add-Missing "wasm-bindgen-cli $pin (the web UI): cargo install wasm-bindgen-cli --version $pin --locked" }
        }
    }
}

Test-Prerequisites
if ($Missing.Count -gt 0) {
    Write-Err "Cua Spaces Local cannot be built yet: $($Missing.Count) thing(s) to install first. Nothing was built or changed."
    foreach ($m in $Missing) { [Console]::Error.WriteLine("  - $m") }
    [Console]::Error.WriteLine('Install them, then run this script again.')
    exit 1
}

if ($SkipRustBuild) {
    foreach ($f in @('cua_spaces_ffi.dll', 'cua_node_runtime.node', 'cua.exe')) {
        if (-not (Test-Path (Join-Path $NativeDir $f))) {
            Stop-Install "-SkipRustBuild: there is no $f in $NativeDir. Build it (node scripts\build-native.mjs) or leave out -SkipRustBuild."
        }
    }
}
if (-not $BuildWeb -and -not (Test-Path (Join-Path $WebDist 'index.html'))) {
    Stop-Install "there is no web UI build at ${WebDist}\index.html, and without it Cua Spaces Local shows ""The new UI is not in this build"". Build it ((cd apps\cua-spaces-web; pnpm install --ignore-workspace --frozen-lockfile; pnpm build)) or leave out -SkipWebBuild."
}

Write-Host 'Cua Spaces Local installer' -ForegroundColor Cyan
Write-Host "  source:  $RepoRoot"
Write-Host "  config:  $(if ($Release) { 'release (Electron fuses on)' } else { 'debug (Electron fuses off)' })"
Write-Host "  app:     $InstallDir"
Write-Host "  cli:     $CliPath"
Write-Host ''

# --- Build ---------------------------------------------------------------------

$env:CI = 'true'
# The pinned pnpm goes first on PATH, so the `pnpm` inside package scripts is it too.
$env:npm_config_manage_package_manager_versions = 'false'
$pnpmPath = Resolve-Pnpm
if ($pnpmPath -ne 'pnpm') { $env:PATH = (Split-Path -Parent $pnpmPath) + ';' + $env:PATH }
$webProc = $null
$webLog = Join-Path ([System.IO.Path]::GetTempPath()) "cua-local-webui-$PID.log"
if ($BuildWeb) {
    Write-Step 'building the web UI (apps\cua-spaces-web: pnpm install, pnpm build) in the background'
    $cmd = "pnpm install --ignore-workspace --frozen-lockfile > ""$webLog"" 2>&1 && pnpm build >> ""$webLog"" 2>&1"
    $webProc = Start-Process -FilePath "$env:ComSpec" -ArgumentList '/d', '/c', $cmd -WorkingDirectory $WebApp -PassThru -WindowStyle Hidden
    $null = $webProc.Handle
}
function Wait-WebBuild {
    if (-not $webProc) { return }
    $webProc.WaitForExit()
    if ($webProc.ExitCode -ne 0) {
        Get-Content $webLog -Tail 60 | ForEach-Object { [Console]::Error.WriteLine($_) }
        Stop-Install "the web UI build failed (log: $webLog). Nothing was installed."
    }
    Remove-Item $webLog -Force -ErrorAction SilentlyContinue
    Write-Ok 'the web UI is built'
}

Write-Step 'installing the desktop shell dependencies (pnpm install --frozen-lockfile)'
Invoke-Native 'pnpm install' 'pnpm' @('install', '--frozen-lockfile') $AppDir

if (-not $SkipRustBuild) {
    Write-Step 'building the native layer: the Cua Spaces library, the Node runtime and cua.exe (cargo, release)'
    Invoke-Native 'the native build' 'node' @('scripts/build-native.mjs') $AppDir
}
Wait-WebBuild
if ($BuildWeb -or $SkipWebBuild) {
    if (-not (Test-Path (Join-Path $WebDist 'index.html'))) { Stop-Install "the web UI build left no ${WebDist}\index.html" }
}
if ($env:CUA_WEBUI_DIST -and ((Resolve-RealPath $env:CUA_WEBUI_DIST) -ne (Resolve-RealPath (Join-Path $WebApp 'dist')))) {
    Write-Warn 'CUA_WEBUI_DIST is set, but the shell packs apps\cua-spaces-web\dist: copy the build there to use it.'
}

Write-Step 'packaging the app (electron-builder --win dir, Local config)'
Remove-Item -LiteralPath $BuildOut -Recurse -Force -ErrorAction SilentlyContinue
$env:CUA_SPACES_ARCHS = $Arch
$env:CUA_SPACES_LOCAL_ARCH = $Arch
if ($Release) { Remove-Item Env:\CUA_SPACES_NO_FUSES -ErrorAction SilentlyContinue } else { $env:CUA_SPACES_NO_FUSES = '1' }
Invoke-Native 'pnpm build' 'pnpm' @('build') $AppDir
Invoke-Native 'electron-builder' 'pnpm' @('exec', 'electron-builder', '--win', 'dir', "--$Arch", '--config', $BuilderConfig, '--publish', 'never') $AppDir
$Built = Join-Path $BuildOut $(if ($Arch -eq 'arm64') { 'win-arm64-unpacked' } else { 'win-unpacked' })
if (-not (Test-Path (Join-Path $Built $Local.ExeName))) {
    Stop-Install "electron-builder did not produce $($Local.ExeName) in ${Built}: it did not build the Local config, so nothing was installed."
}

# --- Stage and verify -------------------------------------------------------------

Write-Step "staging in $StageDir"
$rollback = $null
try {
    Remove-Item -LiteralPath $StageDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path $ProgramsDir -Force | Out-Null
    Copy-Item -LiteralPath $Built -Destination $StageDir -Recurse
    $stageBin = Join-Path $StageDir 'bin'
    New-Item -ItemType Directory -Path $stageBin -Force | Out-Null
    # The CLI is a copy named cua-local.exe: the CLI knows it is the Local channel by that name
    # (CuaChannel::path_is_local). A copy, not a link: no privilege, and PATH never names cua.exe.
    $nativeCli = Join-Path $StageDir 'resources\native\cua.exe'
    Copy-Item -LiteralPath $nativeCli -Destination (Join-Path $stageBin $Local.CliExeName)
    # The guest init of Windows microVMs sits next to the CLI too (the daemon looks beside its own executable).
    $nativeInit = Join-Path $StageDir 'resources\native\cua-microvm-init'
    if (Test-Path $nativeInit) { Copy-Item -LiteralPath $nativeInit -Destination (Join-Path $stageBin 'cua-microvm-init') }

    $problems = @()
    foreach ($f in @($Local.ExeName, 'resources\app.asar', 'resources\native\cua_spaces_ffi.dll', 'resources\native\cua_node_runtime.node', 'resources\web\index.html', "bin\$($Local.CliExeName)")) {
        if (-not (Test-Path (Join-Path $StageDir $f))) { $problems += "missing $f" }
    }
    if (Test-Path (Join-Path $StageDir 'resources\app-update.yml')) { $problems += 'it has an update feed (resources\app-update.yml)' }
    $cliVersion = ''
    try { $cliVersion = (& (Join-Path $stageBin $Local.CliExeName) --version 2>&1 | Out-String).Trim() } catch { $problems += "cua-local --version failed: $($_.Exception.Message)" }
    if ($LASTEXITCODE -ne 0 -or -not $cliVersion) { $problems += "cua-local --version printed nothing useful (exit $LASTEXITCODE)" }
    if ($problems.Count -gt 0) {
        Stop-Install ("the staged build does not verify: " + ($problems -join '; ') + '. Nothing was installed.')
    }
    Write-Ok "verified: $cliVersion"

    $pkgVersion = (Get-Content (Join-Path $AppDir 'package.json') -Raw | ConvertFrom-Json).version
    $marker = [ordered]@{
        appId       = $Local.AppId
        version     = $pkgVersion
        source      = $RepoRoot
        installedAt = (Get-Date).ToString('o')
        pathEntry   = $null
        shortcut    = $null
        protocol    = $false
    }

    # --- Swap -------------------------------------------------------------------
    $liveState = Get-InstallState $InstallDir
    if ($liveState -eq 'local') {
        Write-Step 'stopping the running Cua Spaces Local (only processes from its folder)'
        $stopped = Stop-ProcessesUnder $InstallDir
        if ($stopped -gt 0) { Write-Host "  stopped $stopped process(es)" }
    }
    Remove-Item -LiteralPath $OldDir -Recurse -Force -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $InstallDir) {
        Move-Item -LiteralPath $InstallDir -Destination $OldDir
        $rollback = $OldDir
    }
    try {
        Move-Item -LiteralPath $StageDir -Destination $InstallDir
    } catch {
        if ($rollback) { Move-Item -LiteralPath $OldDir -Destination $InstallDir -ErrorAction SilentlyContinue }
        throw
    }

    # --- Windows integration -------------------------------------------------------
    if (-not $NoIntegration) {
        Write-Step 'Start Menu shortcut, user PATH, cua-local:// scheme'
        New-AppShortcut $ShortcutPath $ExePath $InstallDir $Local.AppId 'Cua Spaces Local'
        $marker.shortcut = $ShortcutPath
        $userPath = Get-UserPath
        if (-not (Test-PathListHas $userPath $BinDir)) {
            Set-UserPath (Add-PathListEntry $userPath $BinDir)
            $marker.pathEntry = $BinDir
        } else {
            $marker.pathEntry = $BinDir
        }
        Register-LocalProtocol $ExePath
        $marker.protocol = $true
    }
    ($marker | ConvertTo-Json) | Set-Content -LiteralPath (Join-Path $InstallDir $Local.MarkerName) -Encoding UTF8
    $rollback = $null
    Remove-Item -LiteralPath $OldDir -Recurse -Force -ErrorAction SilentlyContinue
} finally {
    Remove-Item -LiteralPath $StageDir -Recurse -Force -ErrorAction SilentlyContinue
    if ($rollback -and (Test-Path -LiteralPath $OldDir) -and -not (Test-Path -LiteralPath $InstallDir)) {
        Move-Item -LiteralPath $OldDir -Destination $InstallDir -ErrorAction SilentlyContinue
        Write-Warn 'the install failed; the previous Cua Spaces Local was put back.'
    }
}

Write-Host ''
Write-Ok 'Cua Spaces Local is installed.'
Write-Host "  app:   $ExePath"
Write-Host "  cli:   $CliPath   (open a new terminal: cua-local --version)"
Write-Host "  home:  $(Join-Path $env:USERPROFILE $Local.HomeDirName)   (the app sets CUA_HOME and CUA_CHANNEL=local)"
Write-Host '  scheme: cua-local://   updates: none'

if (-not $NoLaunch) {
    Write-Step 'starting Cua Spaces Local'
    # The app inherits this script's priority: it runs at the normal one.
    try { (Get-Process -Id $PID).PriorityClass = 'Normal' } catch { }
    Start-Process -FilePath $ExePath -WorkingDirectory $InstallDir
}
