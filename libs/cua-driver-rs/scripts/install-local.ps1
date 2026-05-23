# cua-driver-rs local installer (Windows). Builds release-mode from the
# current source tree and drops the resulting cua-driver.exe into the same
# install layout that scripts/install.ps1 produces — so a local build and
# a release install can coexist + the `current` junction can flip between
# them.
#
# Params mirror scripts/install.ps1 so the developer loop matches what
# end users experience:
#   -AutoStart      register the cua-driver-serve Scheduled Task at logon
#                   (Windows-native equivalent of macOS LaunchAgent).
#                   Default off; the post-install message prints the
#                   registration recipe so you can opt in later.
#   -NoPathUpdate   skip the auto-append of the bin dir to the User PATH.
#                   Mirrors install.ps1's flag.
#
# Always builds in `release` configuration to match what install.ps1
# hands users (the prebuilt zip from GitHub Releases is `--release`).
# Use `cargo build -p cua-driver` directly + invoke target\debug\cua-driver.exe
# if you specifically want a faster-to-compile debug binary.
#
# Not for end-users — `irm https://.../install.ps1 | iex` fetches a
# signed/built release from GitHub. This script is for the developer
# loop (rapid edit/build/test on a Windows host).
#
# Layout produced (matches install.ps1 — see its header for details):
#
#   <visibleBinDir>            [junction → currentDir]
#   <currentDir>               [junction → release dir, retargeted here]
#   <release dir>              [real dir, this script's output]
#     0.0.0-local-release-<target>\cua-driver.exe
#
# The version-string carries `-local-release` so it never collides with
# a real release dir and is trivial to garbage-collect.

[CmdletBinding()]
param(
    [switch]$AutoStart,
    [switch]$NoPathUpdate
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# Reuse the production installer's helpers (path resolution, junction
# wiring, autostart registration) by dot-sourcing the relevant bits via
# a small wrapper. install.ps1 expects to run end-to-end, so we don't
# dot-source the whole thing — instead duplicate the small handful of
# operations we need, calling out matching install.ps1 functions where
# the logic would otherwise drift.

$ScriptDir   = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot    = (Resolve-Path "$ScriptDir\..").Path
$BinaryName  = "cua-driver.exe"
# Always release-config — matches the binary install.ps1 hands end users.
$Config      = "release"
# Arch detection — use $env:PROCESSOR_ARCHITECTURE rather than
# RuntimeInformation::OSArchitecture so this works under
# Set-StrictMode -Version Latest (same fix as install.ps1 PR #1631).
$archEnv = $env:PROCESSOR_ARCHITECTURE
$Target = switch -Regex ($archEnv) {
    '^ARM64$' { "aarch64-pc-windows-msvc"; break }
    default   { "x86_64-pc-windows-msvc" }
}

# ---------- Paths (must match install.ps1's defaults) ----------------------

if ($env:CUA_DRIVER_RS_INSTALL_DIR) {
    $VisibleBinDir = $env:CUA_DRIVER_RS_INSTALL_DIR
} else {
    # Path layout matches install.ps1's v0.2.14+ rename
    # (trycua\cua-driver-rs → Cua\cua-driver). See PR #1644.
    $VisibleBinDir = Join-Path $env:LOCALAPPDATA "Programs\Cua\cua-driver\bin"
}
if ($env:CUA_DRIVER_RS_HOME) {
    $PackageHome = $env:CUA_DRIVER_RS_HOME
} else {
    $PackageHome = Join-Path $env:USERPROFILE ".cua-driver"
}
$CurrentDir  = Join-Path $PackageHome "packages\current"
$ReleasesDir = Join-Path $PackageHome "packages\releases"

# ---------- Helpers (mirror install.ps1) ----------------------------------
#
# install.ps1 keeps these inline in its own scope (it's a one-shot script
# that runs end-to-end). Mirror them here so install-local.ps1 can stand
# alone too. If install.ps1 ever extracts these into a shared file, this
# duplication is the time to delete it.

function Test-IsJunction([string]$path) {
    if (-not (Test-Path -LiteralPath $path)) { return $false }
    $item = Get-Item -LiteralPath $path -Force
    return [bool]($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)
}

function Ensure-Junction([string]$linkPath, [string]$targetPath) {
    New-Item -ItemType Directory -Path (Split-Path -Parent $linkPath) -Force | Out-Null
    if (Test-Path -LiteralPath $linkPath) {
        if (Test-IsJunction $linkPath) {
            # Always retarget — that's the point of this helper.
            cmd /c rmdir (Resolve-Path -LiteralPath $linkPath).Path | Out-Null
        } else {
            throw "Refusing to replace non-junction at $linkPath. Move or delete it first."
        }
    }
    cmd /c mklink /J $linkPath $targetPath | Out-Null
}

function Register-CuaDriverAutostart {
    param([Parameter(Mandatory = $true)][string]$InstalledBinary)
    if (-not (Test-Path -LiteralPath $InstalledBinary)) {
        throw "binary not found at $InstalledBinary"
    }
    & $InstalledBinary autostart enable
    if ($LASTEXITCODE -ne 0) {
        throw "cua-driver autostart enable failed (exit $LASTEXITCODE)"
    }
}

function Write-Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }

# ---------- Prerequisites --------------------------------------------------

Write-Step "cua-driver-rs local installer (Windows)"
Write-Host "  source:    $RepoRoot"
Write-Host "  config:    $Config"
Write-Host "  target:    $Target"
Write-Host "  visible:   $VisibleBinDir"
Write-Host "  current:   $CurrentDir"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "Error: cargo not found on PATH." -ForegroundColor Red
    Write-Host "Install Rust + MSVC toolchain via rustup-init: https://rustup.rs/"
    exit 1
}

# ---------- Build ----------------------------------------------------------

Write-Step "cargo build --release -p cua-driver"
Push-Location $RepoRoot
try {
    & cargo build --release -p cua-driver
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Error: cargo build failed." -ForegroundColor Red
        exit $LASTEXITCODE
    }
}
finally {
    Pop-Location
}
$BuiltBinary = Join-Path $RepoRoot "target\$Config\$BinaryName"
if (-not (Test-Path -LiteralPath $BuiltBinary)) {
    Write-Host "Error: build produced no binary at $BuiltBinary" -ForegroundColor Red
    exit 1
}

# ---------- Stage into versioned release dir -------------------------------

$VersionTag  = "0.0.0-local-$Config"
$VersionedDir = Join-Path $ReleasesDir "$VersionTag-$Target"
Write-Step "staging into $VersionedDir"
New-Item -ItemType Directory -Path $VersionedDir -Force | Out-Null
Copy-Item -LiteralPath $BuiltBinary -Destination (Join-Path $VersionedDir $BinaryName) -Force
$installedBinary = Join-Path $VersionedDir $BinaryName

# Stage the skill pack alongside the binary. install-local mirrors what
# install.ps1 does from a release zip — copies Skills/cua-driver-rs/ from
# the repo into the versioned dir so the `current` junction below
# transparently exposes it to agents.
$SourceSkills = Join-Path $RepoRoot "Skills\cua-driver-rs"
if (Test-Path -LiteralPath $SourceSkills) {
    $StagedSkills = Join-Path $VersionedDir "Skills\cua-driver-rs"
    if (Test-Path -LiteralPath $StagedSkills) {
        Remove-Item -LiteralPath $StagedSkills -Recurse -Force
    }
    New-Item -ItemType Directory -Path (Split-Path -Parent $StagedSkills) -Force | Out-Null
    Copy-Item -Path $SourceSkills -Destination $StagedSkills -Recurse -Force
    Write-Step "staged skill pack at $StagedSkills"
}

# ---------- Repoint junctions ---------------------------------------------

Write-Step "retargeting $CurrentDir -> $VersionedDir"
Ensure-Junction -linkPath $CurrentDir -targetPath $VersionedDir

Write-Step "ensuring $VisibleBinDir -> $CurrentDir"
if (Test-Path -LiteralPath $VisibleBinDir) {
    if (-not (Test-IsJunction $VisibleBinDir)) {
        Write-Host "$VisibleBinDir exists and is not a junction; aborting." -ForegroundColor Red
        Write-Host "Remove or relocate it, then re-run."
        exit 1
    }
}
Ensure-Junction -linkPath $VisibleBinDir -targetPath $CurrentDir

# ---------- User PATH update (matches install.ps1) ------------------------

if (-not $NoPathUpdate) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $alreadyOnPath = $userPath -and (($userPath -split ';') -contains $VisibleBinDir)
    if (-not $alreadyOnPath) {
        $newValue = if ($userPath) { ($userPath.TrimEnd(';')) + ';' + $VisibleBinDir } else { $VisibleBinDir }
        [Environment]::SetEnvironmentVariable('Path', $newValue, 'User')
        # Also update the current process's $env:Path so subsequent
        # commands in THIS shell see cua-driver immediately. install.ps1
        # does the same — see #1651.
        if (-not (($env:Path -split ';') -contains $VisibleBinDir)) {
            $env:Path = "$VisibleBinDir;$env:Path"
        }
        Write-Step "added $VisibleBinDir to User PATH"
    } else {
        Write-Step "$VisibleBinDir already on User PATH"
    }
} else {
    Write-Step "skipping User PATH update (-NoPathUpdate)"
}

# Agent skill pack symlinks: NOT auto-created. Run
# `cua-driver skills install --local` to symlink agent dirs to the
# staged copy at $StagedSkills above.

# ---------- Done -----------------------------------------------------------

Write-Host ""
Write-Step "installed"
Write-Host "  exe:    $(Join-Path $VisibleBinDir $BinaryName)"
Write-Host "  source: $installedBinary"
Write-Host ""

if ($AutoStart) {
    Write-Step "registering Scheduled Task 'cua-driver-serve'"
    try {
        Register-CuaDriverAutostart -InstalledBinary (Join-Path $VisibleBinDir $BinaryName)
        Write-Host "  Registered. cua-driver serve auto-starts at every interactive logon." -ForegroundColor Green
    }
    catch {
        Write-Host "  Failed to register: $($_.Exception.Message)" -ForegroundColor Red
    }
}

# Unified post-install hints come from a single shared text file so the
# 4 Rust installers (this script + install-local.sh + install.ps1 +
# _install-rust.sh) never drift. The .txt holds the OS-agnostic bulk
# (Try-it / skill pack / MCP setup / docs link) with {{BINARY}}
# placeholders; OS-specific bits stay inline below.
$installedBinary = Join-Path $VisibleBinDir $BinaryName
$HintsTxt = Join-Path $RepoRoot "..\cua-driver\scripts\post-install-hints.txt"
if (Test-Path -LiteralPath $HintsTxt) {
    $hintsRaw = Get-Content -Raw -LiteralPath $HintsTxt
    Write-Host ($hintsRaw -replace '\{\{BINARY\}\}', $installedBinary)
} else {
    # Repo layout changed or .txt missing — fall back to one-line
    # essentials so users still know what to do next.
    Write-Host "Next steps: $installedBinary --version  |  $installedBinary mcp-config  |  $installedBinary skills install"
    Write-Host "Docs: https://github.com/trycua/cua/tree/main/libs/cua-driver-rs"
}

# Windows-specific autostart hint (kept inline; per-shell natural location).
if (-not $AutoStart) {
    Write-Host ""
    Write-Host "Auto-start at logon (Windows-native equivalent of macOS LaunchAgent):" -ForegroundColor Cyan
    Write-Host "  cua-driver autostart enable    (register Scheduled Task at RunLevel=Highest)" -ForegroundColor Cyan
    Write-Host "  cua-driver autostart kick      (start now without re-logging)" -ForegroundColor Cyan
    Write-Host "  cua-driver autostart status    (inspect)" -ForegroundColor Cyan
    Write-Host "  cua-driver autostart disable   (remove)" -ForegroundColor Cyan
    Write-Host "  Or re-run install-local.ps1 with -AutoStart for the same result." -ForegroundColor Cyan
    Write-Host ""
}
