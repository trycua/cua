# cua-driver-rs local installer (Windows). Builds release-mode from the
# current source tree into a durable, separate local-product namespace.
#
# Params mirror scripts/install.ps1 so the developer loop matches what
# end users experience:
#   -AutoStart      register the cua-driver-local-serve Scheduled Task at logon
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
# Separate local layout produced:
#
#   <visibleBinDir>            [junction → currentDir]
#   <currentDir>               [junction → release dir, retargeted here]
#   <release dir>              [real dir, this script's output]
#     0.0.0-local-release-<target>\cua-driver-local.exe
#
# The version-string carries `-local-release` so it never collides with
# a real release dir and is trivial to garbage-collect.

[CmdletBinding()]
param(
    # Default-on: most users want the daemon to come back at every
    # logon. Opt out with `-AutoStart:$false` (or the dedicated
    # `-NoAutoStart`) when running install-local.ps1 from CI / a
    # container build / a sandbox where you specifically don't want
    # a scheduled task registered.
    [switch]$AutoStart = $true,
    [switch]$NoAutoStart,
    [switch]$NoPathUpdate
)
# `-NoAutoStart` is the explicit opt-out; takes precedence over the
# default-true `-AutoStart` for callers who'd rather read negative
# than `-AutoStart:$false`.
if ($NoAutoStart) { $AutoStart = $false }

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
# Rust workspace root: scripts/ is the cross-cutting installer dir at
# libs/cua-driver/scripts/; the Cargo workspace lives one level deeper
# under libs/cua-driver/rust/.
$RepoRoot    = (Resolve-Path "$ScriptDir\..\rust").Path
$BinaryName  = "cua-driver-local.exe"
$BuiltBinaryName = "cua-driver.exe"
$UiaBinaryName = "cua-driver-uia-local.exe"
# Always release-config — matches the binary install.ps1 hands end users.
$Config      = "release"

# Embed local-build provenance in `get_config`. Preserve an explicit value for
# source snapshots copied to VMs without `.git`; otherwise derive the commit
# from the checkout being built. Mark dirty developer trees honestly instead
# of presenting their binaries as exact products of the clean commit.
if ([string]::IsNullOrWhiteSpace($env:CUA_DRIVER_SOURCE_SHA)) {
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        throw "git is required to determine CUA_DRIVER_SOURCE_SHA; set it explicitly for a source snapshot"
    }
    $detectedSourceSha = (& git -C $RepoRoot rev-parse --verify 'HEAD^{commit}' 2>$null | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $detectedSourceSha -notmatch '^[0-9a-fA-F]{40}([0-9a-fA-F]{24})?$') {
        throw "could not determine an exact Git commit for $RepoRoot; set CUA_DRIVER_SOURCE_SHA explicitly"
    }
    $dirtyState = (& git -C $RepoRoot status --porcelain --untracked-files=normal 2>$null | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "could not determine whether the source tree is dirty: $RepoRoot"
    }
    $env:CUA_DRIVER_SOURCE_SHA = if ($dirtyState) { "$detectedSourceSha-dirty" } else { $detectedSourceSha }
}
# Arch detection — use $env:PROCESSOR_ARCHITECTURE rather than
# RuntimeInformation::OSArchitecture so this works under
# Set-StrictMode -Version Latest (same fix as install.ps1 PR #1631).
$archEnv = $env:PROCESSOR_ARCHITECTURE
$Target = switch -Regex ($archEnv) {
    '^ARM64$' { "aarch64-pc-windows-msvc"; break }
    default   { "x86_64-pc-windows-msvc" }
}

# ---------- Paths (must match install.ps1's defaults) ----------------------

if ($env:CUA_DRIVER_LOCAL_INSTALL_DIR) {
    $VisibleBinDir = $env:CUA_DRIVER_LOCAL_INSTALL_DIR
} else {
    $VisibleBinDir = Join-Path $env:LOCALAPPDATA "Programs\Cua\cua-driver-local\bin"
}
if ($env:CUA_DRIVER_LOCAL_HOME) {
    $PackageHome = $env:CUA_DRIVER_LOCAL_HOME
} else {
    $PackageHome = Join-Path $env:USERPROFILE ".cua-driver-local"
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
        throw "cua-driver-local autostart enable failed (exit $LASTEXITCODE)"
    }
}

function Stop-CuaDriverLocalDaemons {
    & schtasks.exe /End /TN "cua-driver-local-serve" 2>$null | Out-Null
    Get-Process -Name "cua-driver-local","cua-driver-uia-local" -ErrorAction SilentlyContinue |
        Stop-Process -Force -ErrorAction SilentlyContinue
}

function Write-Step($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }

# ---------- Prerequisites --------------------------------------------------

Write-Step "cua-driver-rs local installer (Windows)"
Write-Host "  source:    $RepoRoot"
Write-Host "  sha:       $env:CUA_DRIVER_SOURCE_SHA"
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

Write-Step "cargo build --release -p cua-driver -p cua-driver-uia"
Push-Location $RepoRoot
try {
    & cargo build --release -p cua-driver -p cua-driver-uia
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Error: cargo build failed." -ForegroundColor Red
        exit $LASTEXITCODE
    }
}
finally {
    Pop-Location
}
$BuiltBinary = Join-Path $RepoRoot "target\$Config\$BuiltBinaryName"
$BuiltUiaBinary = Join-Path $RepoRoot "target\$Config\cua-driver-uia.exe"
if (-not (Test-Path -LiteralPath $BuiltBinary)) {
    Write-Host "Error: build produced no binary at $BuiltBinary" -ForegroundColor Red
    exit 1
}

# ---------- Stage into versioned release dir -------------------------------

$VersionTag  = "0.0.0-local-$Config"
$VersionedDir = Join-Path $ReleasesDir "$VersionTag-$Target"
$DestBinary = Join-Path $VersionedDir $BinaryName

# If a previous install-local left a binary here and it's currently
# being executed (typical: `cua-driver-local autostart kick` spawned a
# High-IL daemon at logon, which we can't terminate from this
# Medium-IL shell without UAC), the Copy-Item below fails with
# "The process cannot access the file ... because it is being used by
# another process." Windows DOES allow renaming a locked .exe — the
# loader opens images with FILE_SHARE_DELETE, so a rename succeeds
# while the content stays locked. Renaming out of the way frees up
# the destination path so Copy-Item lands cleanly. The old file gets
# unlinked at the next reboot or when the daemon exits.
if (Test-Path -LiteralPath $DestBinary) {
    $ts = (Get-Date).ToString('yyyyMMdd-HHmmss')
    $stale = "$DestBinary.stale-$ts"
    try {
        Move-Item -LiteralPath $DestBinary -Destination $stale -Force -ErrorAction Stop
        Write-Step "renamed locked previous binary to $(Split-Path -Leaf $stale)"
    } catch {
        Write-Host "Note: could not rename previous binary at $DestBinary." -ForegroundColor Yellow
        Write-Host "      ($($_.Exception.Message))" -ForegroundColor Yellow
        Write-Host "      Most likely a running cua-driver-local daemon is holding it." -ForegroundColor Yellow
        Write-Host "      Stop it first (e.g. ``schtasks /End /TN cua-driver-local-serve`` then re-run)." -ForegroundColor Yellow
    }
    # Best-effort GC of stale-* siblings older than this run. Cheap;
    # keeps the dir from growing unbounded over many re-builds.
    Get-ChildItem -LiteralPath $VersionedDir -Filter "$BinaryName.stale-*" -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTime -lt (Get-Date).AddDays(-1) } |
        ForEach-Object { try { Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue } catch {} }

    Write-Step "killing previous cua-driver-local processes (best-effort; High-IL needs admin)"
    Stop-CuaDriverLocalDaemons
}

Write-Step "staging into $VersionedDir"
New-Item -ItemType Directory -Path $VersionedDir -Force | Out-Null
Copy-Item -LiteralPath $BuiltBinary -Destination $DestBinary -Force
$DestUiaBinary = Join-Path $VersionedDir $UiaBinaryName
if (Test-Path -LiteralPath $BuiltUiaBinary) {
    Copy-Item -LiteralPath $BuiltUiaBinary -Destination $DestUiaBinary -Force
}
$installedBinary = $DestBinary

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
    Write-Step "registering Scheduled Task 'cua-driver-local-serve'"
    try {
        Register-CuaDriverAutostart -InstalledBinary (Join-Path $VisibleBinDir $BinaryName)
        Write-Host "  Registered. cua-driver-local serve auto-starts at every interactive logon." -ForegroundColor Green
    }
    catch {
        Write-Host "  Failed to register: $($_.Exception.Message)" -ForegroundColor Red
    }
} else {
    # User didn't pass -AutoStart, but if a local task is already registered,
    # re-register it pointing at this
    # fresh binary. Otherwise the user ends up with a task whose
    # <Command> path is the OLD release-install dir, running the OLD
    # binary - even though `cua-driver-local` on PATH now resolves to the
    # fresh one. See trycua/cua#1654 (hidden-console wrapper landed
    # later - old tasks that survived an upgrade still produce the
    # visible console window at logon).
    $prevEAP = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & schtasks.exe /Query /TN "cua-driver-local-serve" 2>$null | Out-Null
        $hasTask = ($LASTEXITCODE -eq 0)
    } finally {
        $ErrorActionPreference = $prevEAP
    }
    if ($hasTask) {
        Write-Step "found existing 'cua-driver-local-serve' task - re-registering against fresh binary"
        try {
            Register-CuaDriverAutostart -InstalledBinary (Join-Path $VisibleBinDir $BinaryName)
            Write-Host "  Re-registered. Task action now uses this build's hidden-console wrapper." -ForegroundColor Green
        }
        catch {
            Write-Host "  Failed to re-register: $($_.Exception.Message)" -ForegroundColor Red
            Write-Host "  The existing task still points at the previous binary. Run 'cua-driver-local autostart enable' from an elevated shell to update."
        }
    }
}

# Unified post-install hints come from a single shared text file so the
# 4 Rust installers (this script + install-local.sh + install.ps1 +
# _install-rust.sh) never drift. The .txt holds the OS-agnostic bulk
# (Try-it / skill pack / MCP setup / docs link) with {{BINARY}}
# placeholders; OS-specific bits stay inline below.
$installedBinary = Join-Path $VisibleBinDir $BinaryName
$HintsTxt = Join-Path $ScriptDir "post-install-hints.txt"
if (Test-Path -LiteralPath $HintsTxt) {
    # Read explicitly as UTF-8. PowerShell 5.1's Get-Content -Raw falls
    # back to Windows-1252 when the source file has no BOM, which turns
    # the .txt's `•` / `—` into mojibake (`â€¢` / `â€"`) in the rendered
    # block. The other 3 installers don't hit this: install.ps1 reads
    # the URL response via Invoke-WebRequest (HTTP charset decoding);
    # _install-rust.sh / install-local.sh stream raw bytes through sed.
    $hintsRaw = [System.IO.File]::ReadAllText($HintsTxt, [System.Text.Encoding]::UTF8)
    Write-Host ($hintsRaw -replace '\{\{BINARY\}\}', $installedBinary)
} else {
    # Repo layout changed or .txt missing — fall back to one-line
    # essentials so users still know what to do next.
    Write-Host "Next steps: $installedBinary --version  |  $installedBinary mcp-config  |  $installedBinary skills install"
    Write-Host "Docs: https://github.com/trycua/cua/tree/main/libs/cua-driver/rust"
}

# The local/release identity split deliberately stopped source installs from
# creating or repairing the published cua-driver.exe path. Surface the
# migration state when only the local product is present so an existing MCP
# client does not keep launching a missing release command. Do not create an
# alias here: local and published products must retain separate identities.
$releaseBinary = Join-Path $env:LOCALAPPDATA "Programs\Cua\cua-driver\bin\cua-driver.exe"
if (-not (Test-Path -LiteralPath $releaseBinary -PathType Leaf)) {
    Write-Host ""
    Write-Host "Migration note: the published cua-driver CLI is not installed at $releaseBinary." -ForegroundColor Yellow
    Write-Host "  Existing MCP clients configured for 'cua-driver' will not use this local build." -ForegroundColor Yellow
    Write-Host "  To configure Codex for the local build, run:"
    Write-Host "    $installedBinary mcp-config --client codex"
    Write-Host "  To restore the published product instead, run:"
    Write-Host "    irm https://cua.ai/driver/install.ps1 | iex"
}

# Windows-specific autostart hint (kept inline; per-shell natural location).
if ($AutoStart) {
    # Default branch: autostart was enabled (either by default or explicitly).
    # Surface the management subcommands so the user knows how to inspect /
    # disable later without digging through Task Scheduler.
    Write-Host ""
    Write-Host "Auto-start: 'cua-driver-local-serve' is registered at RunLevel=Highest." -ForegroundColor Cyan
    Write-Host "  cua-driver-local autostart status    (inspect)" -ForegroundColor Cyan
    Write-Host "  cua-driver-local autostart disable   (remove)" -ForegroundColor Cyan
    Write-Host "  cua-driver-local autostart kick      (start now without re-logging)" -ForegroundColor Cyan
    Write-Host ""
} else {
    # Opt-out branch (-NoAutoStart or -AutoStart:`$false`).
    Write-Host ""
    Write-Host "Auto-start at logon (NOT enabled - re-run without -NoAutoStart to register, or:):" -ForegroundColor Cyan
    Write-Host "  cua-driver-local autostart enable    (register Scheduled Task at RunLevel=Highest)" -ForegroundColor Cyan
    Write-Host "  cua-driver-local autostart kick      (start now without re-logging)" -ForegroundColor Cyan
    Write-Host "  cua-driver-local autostart status    (inspect)" -ForegroundColor Cyan
    Write-Host "  cua-driver-local autostart disable   (remove)" -ForegroundColor Cyan
    Write-Host ""
}
