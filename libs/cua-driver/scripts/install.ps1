# cua-driver-rs installer (Windows) — download the latest cua-driver-rs
# release zip from GitHub Releases and wire it up via a chain of
# directory junctions, so future upgrades / rollbacks retarget a
# junction instead of overwriting files. Sudo-free, no Developer Mode
# required, no admin elevation.
#
# Usage (one-liner — recommended):
#   irm https://cua.ai/driver/install.ps1 | iex
#
# Pin a version:
#   $env:CUA_DRIVER_RS_VERSION = "0.2.0"
#   irm https://cua.ai/driver/install.ps1 | iex
#
# Layout on disk (three tiers, two directory junctions):
#
#   <visibleBinDir>            [directory junction → currentDir]
#     = %LOCALAPPDATA%\Programs\Cua\cua-driver\bin
#   <currentDir>               [directory junction → release dir]
#     = %USERPROFILE%\.cua-driver\packages\current
#   <release dir>              [real directory, immutable per version]
#     = %USERPROFILE%\.cua-driver\packages\releases\<version>-<target>
#         cua-driver.exe
#
# Path layout renamed v0.2.14: `Programs\trycua\cua-driver-rs\` →
# `Programs\Cua\cua-driver\` and `.cua-driver-rs\` → `.cua-driver\`. The
# Rust port IS the canonical Windows driver now (no `-rs` suffix needed),
# and `trycua` is the GitHub org prefix that doesn't belong in
# %LOCALAPPDATA%\Programs. Legacy installs are auto-migrated at the next
# `irm install.ps1 | iex` run.
#
# PATH consumers see <visibleBinDir>; the contents are transparently
# served from whichever release the inner junction currently points at.
# Atomic upgrade  = retarget <currentDir> at a newer release dir.
# Rollback        = retarget <currentDir> at an older release dir already on disk.
#
# Directory junctions (NTFS reparse points, IO_REPARSE_TAG_MOUNT_POINT)
# are creatable by any user without admin rights and without Developer
# Mode — unlike file/dir *symlinks*, which require either elevated
# privileges or Developer Mode. So the whole installer stays sudo-free.
#
# Env overrides:
#   $env:CUA_DRIVER_RS_VERSION       pin a specific release (e.g. "0.2.0")
#   $env:CUA_DRIVER_RS_INSTALL_DIR   override the visible PATH-entry dir
#                                    (default %LOCALAPPDATA%\Programs\Cua\cua-driver\bin)
#   $env:CUA_DRIVER_RS_HOME          override the package home
#                                    (default %USERPROFILE%\.cua-driver)
#   $env:CUA_DRIVER_RS_KEEP_VERSIONS keep the N most recent per-version
#                                    release dirs after install; older ones
#                                    are deleted (default 5; set 0 to
#                                    disable GC entirely). Per-target —
#                                    multi-arch dirs are pruned
#                                    independently of each other.
#
# Params:
#   -Release    release tag to install ("latest" or a bare version like "0.2.0").
#               Overridden by $env:CUA_DRIVER_RS_VERSION when set.
#   -AutoStart  register a Scheduled Task that runs `cua-driver serve` at
#               every logon (Windows-native equivalent of macOS LaunchAgent).
#               The task runs with LogonType=Interactive so it lands in
#               Session 1+ with an attached desktop — required for the
#               GUI tools (click, type_text, screenshot, get_window_state)
#               to function. Default off; the post-install message prints
#               the registration command so you can opt in later. Safe to
#               re-run: existing task is replaced.
#   -NoPathUpdate
#               skip the auto-append of $VisibleBinDir to the User PATH.
#               Default off — the installer auto-adds the bin dir so
#               `cua-driver --version` works in new shells without manual
#               `[Environment]::SetEnvironmentVariable` gymnastics. Pass
#               this when you manage PATH out-of-band (chezmoi, a dotfiles
#               repo, group policy). Idempotent either way: if the dir is
#               already on PATH, nothing changes.
#
# Windows installer for the cross-platform cua-driver Rust implementation.

[CmdletBinding()]
param(
    [string]$Release = "latest",
    # Default-on: cua-driver-serve is what makes the agent flow work
    # across logon / reboot. Without the scheduled task the user has
    # to remember to run `cua-driver autostart kick` every time, and
    # MCP-style flows go silently in-process. Opt out with
    # `-AutoStart:$false` or `-NoAutoStart` for CI / sandbox installs
    # that specifically don't want a scheduled task registered.
    [switch]$AutoStart = $true,
    [switch]$NoAutoStart,
    [switch]$NoPathUpdate
)
# `-NoAutoStart` is the explicit opt-out and takes precedence over
# the default-true `-AutoStart`.
if ($NoAutoStart) { $AutoStart = $false }

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
# Invoke-WebRequest's progress bar is ~10x slower than the actual download
# over PowerShell ISE and Windows PowerShell 5 — silence it. Restored
# nowhere on purpose: this script is a one-shot, the user can re-set it.
$ProgressPreference = "SilentlyContinue"

$Repo       = "trycua/cua"
$TagPrefix  = "cua-driver-rs-v"
$BinaryName = "cua-driver.exe"

# Baked-version constant — kept in lock-step with the latest published
# cua-driver-rs-v* release tag by the Release Please release pull request
# (see .github/workflows/cd-rust-cua-driver.yml). The sentinel-block
# markers identify this line for Release Please's generic version updater.
#
# Precedence at resolve time: $env:CUA_DRIVER_RS_VERSION > -Release arg >
# this baked value > GitHub Releases API. Baked means the `irm | iex`
# one-liner against `main` is API-free in the common case; the API is
# only consulted as a fallback when this script is run from a branch
# where the baked line hasn't been updated yet.
#
# ~~~ BAKED_VERSION: auto-updated in the release PR — do not edit ~~~
$Script:CuaDriverRsBakedVersion = "0.8.3" # x-release-please-version
# ~~~ END_BAKED_VERSION ~~~

# ---------- Path resolution ------------------------------------------------

if ($env:CUA_DRIVER_RS_INSTALL_DIR) {
    $VisibleBinDir = $env:CUA_DRIVER_RS_INSTALL_DIR
} else {
    # Path layout renamed v0.2.14: `Programs\trycua\cua-driver-rs\` →
    # `Programs\Cua\cua-driver\`. The Rust port IS the canonical Windows
    # driver now (no more `-rs` suffix needed in user-facing paths), and
    # `trycua` is the GitHub org prefix that doesn't belong in
    # %LOCALAPPDATA% — vendor folders there are conventionally PascalCase
    # company names. The env var name keeps the `_RS_` infix so existing
    # automation pinning a custom install dir doesn't break silently.
    $VisibleBinDir = Join-Path $env:LOCALAPPDATA "Programs\Cua\cua-driver\bin"
}

# Legacy install paths from v0.2.13 and earlier. The uninstall path checks
# both; the install path nukes any legacy install before laying down the
# new one, so v0.2.13 → v0.2.14+ is a transparent upgrade.
$LegacyVisibleBinDir = Join-Path $env:LOCALAPPDATA "Programs\trycua\cua-driver-rs\bin"
$LegacyVendorDir     = Join-Path $env:LOCALAPPDATA "Programs\trycua"

if ($env:CUA_DRIVER_RS_HOME) {
    $HomeDir = $env:CUA_DRIVER_RS_HOME
} else {
    # Same rename: `.cua-driver-rs/` → `.cua-driver/`. The `-rs` suffix
    # was the Rust-port-vs-Swift-driver disambiguator while the Swift one
    # still existed for Windows; it doesn't anymore.
    $HomeDir = Join-Path $env:USERPROFILE ".cua-driver"
}

$LegacyHomeDir = Join-Path $env:USERPROFILE ".cua-driver-rs"

$PackagesDir = Join-Path $HomeDir   "packages"
$ReleasesDir = Join-Path $PackagesDir "releases"
$CurrentDir  = Join-Path $PackagesDir "current"

# Post-install GC: how many per-version release dirs to retain. Validated
# in Resolve-KeepVersions below; 0 means "never GC".
$Script:KeepVersionsDefault = 5

function Resolve-KeepVersions {
    $raw = $env:CUA_DRIVER_RS_KEEP_VERSIONS
    if (-not $raw) { return $Script:KeepVersionsDefault }
    $n = 0
    if ([int]::TryParse($raw, [ref]$n) -and $n -ge 0) { return $n }
    Write-WarningStep "CUA_DRIVER_RS_KEEP_VERSIONS=$raw is not a non-negative integer; falling back to $($Script:KeepVersionsDefault)"
    return $Script:KeepVersionsDefault
}

# ---------- Log helpers ----------------------------------------------------

function Write-Step($message) {
    Write-Host "==> $message"
}

function Write-WarningStep($message) {
    Write-Host "WARNING: $message" -ForegroundColor Yellow
}

function Write-ErrorStep($message) {
    Write-Host "error: $message" -ForegroundColor Red
}

# ---------- Architecture detection -----------------------------------------
#
# RuntimeInformation.OSArchitecture reports the *OS* architecture (not the
# process architecture), so an x64 PowerShell running on an arm64 Windows
# host still reports Arm64. That's exactly what we want — we always pick
# the native target for the host so the binary is fastest, even if the
# shell happens to be running under WOW64.

function Get-TargetTriple {
    # Primary source: $env:PROCESSOR_ARCHITECTURE — always set on Windows,
    # never trips StrictMode property introspection, no .NET version dependency.
    # Fallback: [RuntimeInformation]::OSArchitecture — present since .NET 4.7.1
    # but raises 'property cannot be found on this object' under StrictMode
    # Latest on some PowerShell 5.1 setups (observed on Windows 11 24H2 with a
    # fresh user account, see issue tracker).
    $arch = $env:PROCESSOR_ARCHITECTURE
    if (-not $arch) {
        try {
            $arch = ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture).ToString()
        } catch {
            $arch = "unknown"
        }
    }
    switch ($arch) {
        # $env:PROCESSOR_ARCHITECTURE values
        "AMD64" { return "x86_64-pc-windows-msvc" }
        "ARM64" { return "aarch64-pc-windows-msvc" }
        # RuntimeInformation.OSArchitecture .ToString() values (fallback path)
        "X64"   { return "x86_64-pc-windows-msvc" }
        "Arm64" { return "aarch64-pc-windows-msvc" }
        default {
            Write-ErrorStep "unsupported Windows architecture: $arch"
            Write-ErrorStep "  cua-driver-rs ships prebuilts for: x86_64 (AMD64) and arm64 (ARM64)."
            Write-ErrorStep "  Please file an issue at https://github.com/trycua/cua/issues with the output of"
            Write-ErrorStep "  'echo `$env:PROCESSOR_ARCHITECTURE'."
            exit 1
        }
    }
}

function Get-AssetArchLabel($targetTriple) {
    # Asset filenames use the short label (windows-arm64 / windows-x86_64)
    # to match the CD workflow's $stage variable and the macOS / Linux
    # asset naming convention.
    switch ($targetTriple) {
        "x86_64-pc-windows-msvc"   { return "windows-x86_64" }
        "aarch64-pc-windows-msvc"  { return "windows-arm64"  }
    }
}

# ---------- Directory junction support (P/Invoke) --------------------------
#
# Directory junctions are NTFS reparse points with tag
# IO_REPARSE_TAG_MOUNT_POINT (0xA0000003). Creating one is a single
# DeviceIoControl call with FSCTL_SET_REPARSE_POINT (0x900A4) and a
# REPARSE_DATA_BUFFER payload describing the substitute / print names.
# Reading one back is FSCTL_GET_REPARSE_POINT (0x900A8).
#
# Doing this through P/Invoke (versus shelling out to `cmd /c mklink /J`)
# avoids the cmd dependency, dodges a console window flash, and lets the
# installer report a structured error if the junction can't be created
# (e.g. the path is on a non-NTFS volume).
#
# Junctions vs symlinks (why we picked junctions):
#   - Directory symlinks (CreateSymbolicLink with SYMBOLIC_LINK_FLAG_DIRECTORY)
#     need either elevation or the per-user "Create symbolic links"
#     privilege, which on consumer Windows is gated behind Developer Mode.
#   - Directory junctions need none of that — any unprivileged user can
#     create one as long as the source path is a real directory on a
#     local NTFS volume.

function Add-JunctionSupportType {
    if ("CuaDriverInstaller.Junction" -as [type]) { return }

    $source = @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace CuaDriverInstaller
{
    public static class Junction
    {
        private const uint FILE_FLAG_OPEN_REPARSE_POINT = 0x00200000;
        private const uint FILE_FLAG_BACKUP_SEMANTICS   = 0x02000000;
        private const uint GENERIC_READ                 = 0x80000000;
        private const uint GENERIC_WRITE                = 0x40000000;
        private const uint OPEN_EXISTING                = 3;
        private const uint FSCTL_SET_REPARSE_POINT      = 0x000900A4;
        private const uint FSCTL_GET_REPARSE_POINT      = 0x000900A8;
        private const uint IO_REPARSE_TAG_MOUNT_POINT   = 0xA0000003;
        private const int  MAXIMUM_REPARSE_DATA_BUFFER_SIZE = 16 * 1024;

        [StructLayout(LayoutKind.Sequential)]
        private struct REPARSE_DATA_BUFFER
        {
            public uint ReparseTag;
            public ushort ReparseDataLength;
            public ushort Reserved;
            public ushort SubstituteNameOffset;
            public ushort SubstituteNameLength;
            public ushort PrintNameOffset;
            public ushort PrintNameLength;
            // 16 KB minus the fixed-size header — large enough for any
            // sane junction target including UNC paths.
            [MarshalAs(UnmanagedType.ByValArray, SizeConst = MAXIMUM_REPARSE_DATA_BUFFER_SIZE - 16)]
            public byte[] PathBuffer;
        }

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern SafeFileHandle CreateFile(
            string lpFileName,
            uint dwDesiredAccess,
            uint dwShareMode,
            IntPtr lpSecurityAttributes,
            uint dwCreationDisposition,
            uint dwFlagsAndAttributes,
            IntPtr hTemplateFile);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool DeviceIoControl(
            SafeFileHandle hDevice,
            uint dwIoControlCode,
            IntPtr InBuffer,
            uint nInBufferSize,
            IntPtr OutBuffer,
            uint nOutBufferSize,
            out uint pBytesReturned,
            IntPtr lpOverlapped);

        public static void SetTarget(string linkPath, string targetPath)
        {
            // Junction target must be an absolute, NT-namespace path
            // ("\??\C:\foo"). Normalize to a full path first so callers
            // can hand us a relative-looking string.
            string fullTarget = Path.GetFullPath(targetPath);
            string ntTarget   = @"\??\" + fullTarget;

            byte[] substituteBytes = System.Text.Encoding.Unicode.GetBytes(ntTarget);
            byte[] printBytes      = System.Text.Encoding.Unicode.GetBytes(fullTarget);

            // Layout in PathBuffer: <substitute name>\0<print name>\0
            int substituteLen = substituteBytes.Length;
            int printLen      = printBytes.Length;
            int totalBytes    = substituteLen + 2 + printLen + 2;

            REPARSE_DATA_BUFFER buf = new REPARSE_DATA_BUFFER();
            buf.ReparseTag           = IO_REPARSE_TAG_MOUNT_POINT;
            // ReparseDataLength = size of the variable-length payload
            // (the four offset/length fields + the buffer itself).
            buf.ReparseDataLength    = (ushort)(8 + totalBytes);
            buf.Reserved             = 0;
            buf.SubstituteNameOffset = 0;
            buf.SubstituteNameLength = (ushort)substituteLen;
            buf.PrintNameOffset      = (ushort)(substituteLen + 2);
            buf.PrintNameLength      = (ushort)printLen;
            buf.PathBuffer           = new byte[MAXIMUM_REPARSE_DATA_BUFFER_SIZE - 16];

            Array.Copy(substituteBytes, 0, buf.PathBuffer, 0, substituteLen);
            Array.Copy(printBytes,      0, buf.PathBuffer, substituteLen + 2, printLen);

            if (!Directory.Exists(linkPath))
            {
                Directory.CreateDirectory(linkPath);
            }

            using (SafeFileHandle handle = CreateFile(
                linkPath,
                GENERIC_READ | GENERIC_WRITE,
                0,
                IntPtr.Zero,
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                IntPtr.Zero))
            {
                if (handle.IsInvalid)
                {
                    throw new System.ComponentModel.Win32Exception(
                        Marshal.GetLastWin32Error(),
                        "CreateFile(" + linkPath + ") failed");
                }

                int bufferSize = Marshal.SizeOf(buf);
                IntPtr inBuffer = Marshal.AllocHGlobal(bufferSize);
                try
                {
                    Marshal.StructureToPtr(buf, inBuffer, false);
                    uint bytesReturned;
                    // ReparseDataLength + 8-byte header = total bytes to send.
                    uint inSize = (uint)(buf.ReparseDataLength + 8);
                    if (!DeviceIoControl(
                        handle,
                        FSCTL_SET_REPARSE_POINT,
                        inBuffer,
                        inSize,
                        IntPtr.Zero,
                        0,
                        out bytesReturned,
                        IntPtr.Zero))
                    {
                        throw new System.ComponentModel.Win32Exception(
                            Marshal.GetLastWin32Error(),
                            "DeviceIoControl(FSCTL_SET_REPARSE_POINT) failed for " + linkPath);
                    }
                }
                finally
                {
                    Marshal.FreeHGlobal(inBuffer);
                }
            }
        }

        public static string GetTarget(string linkPath)
        {
            using (SafeFileHandle handle = CreateFile(
                linkPath,
                GENERIC_READ,
                0,
                IntPtr.Zero,
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                IntPtr.Zero))
            {
                if (handle.IsInvalid)
                {
                    return null;
                }

                int bufferSize = MAXIMUM_REPARSE_DATA_BUFFER_SIZE;
                IntPtr outBuffer = Marshal.AllocHGlobal(bufferSize);
                try
                {
                    uint bytesReturned;
                    if (!DeviceIoControl(
                        handle,
                        FSCTL_GET_REPARSE_POINT,
                        IntPtr.Zero,
                        0,
                        outBuffer,
                        (uint)bufferSize,
                        out bytesReturned,
                        IntPtr.Zero))
                    {
                        return null;
                    }

                    REPARSE_DATA_BUFFER buf = (REPARSE_DATA_BUFFER)Marshal.PtrToStructure(
                        outBuffer, typeof(REPARSE_DATA_BUFFER));

                    if (buf.ReparseTag != IO_REPARSE_TAG_MOUNT_POINT)
                    {
                        return null;
                    }

                    string substitute = System.Text.Encoding.Unicode.GetString(
                        buf.PathBuffer,
                        buf.SubstituteNameOffset,
                        buf.SubstituteNameLength);

                    // Strip the "\??\" NT-namespace prefix that the kernel
                    // stores in the substitute name so callers see a normal
                    // Win32 path.
                    if (substitute.StartsWith(@"\??\"))
                    {
                        substitute = substitute.Substring(4);
                    }
                    return substitute;
                }
                finally
                {
                    Marshal.FreeHGlobal(outBuffer);
                }
            }
        }
    }
}
'@

    Add-Type -TypeDefinition $source -Language CSharp
}

function Test-IsJunction([string]$path) {
    if (-not (Test-Path -LiteralPath $path)) { return $false }
    $item = Get-Item -LiteralPath $path -Force
    # ReparsePoint flag (1024) on a directory == junction or symlink.
    # We narrow to junctions by re-reading the reparse tag below; this
    # cheap check is good enough to short-circuit common cases.
    return (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)
}

function Set-JunctionTarget([string]$linkPath, [string]$targetPath) {
    Add-JunctionSupportType
    [CuaDriverInstaller.Junction]::SetTarget($linkPath, $targetPath)
}

function Get-JunctionTarget([string]$linkPath) {
    Add-JunctionSupportType
    return [CuaDriverInstaller.Junction]::GetTarget($linkPath)
}

# Ensure a junction at $linkPath points at $targetPath. Refuses to clobber
# an existing non-junction directory at $linkPath — the user may have
# legitimate files there and we don't want to surprise them.
#
# Retarget is in-place and atomic: DeviceIoControl(FSCTL_SET_REPARSE_POINT)
# on an existing reparse point overwrites the reparse-data buffer in a
# single kernel call. The junction is never absent during the swap. This
# is the only race-free retarget primitive NTFS exposes for directory
# reparse points (CreateSymbolicLink + MoveFileEx-replace-existing only
# works for files, not directories). For the initial-create case the path
# is the same: Directory.CreateDirectory followed by SET_REPARSE_POINT.
function Ensure-Junction([string]$linkPath, [string]$targetPath) {
    if (Test-Path -LiteralPath $linkPath) {
        if (Test-IsJunction $linkPath) {
            $existingTarget = Get-JunctionTarget $linkPath
            if ($existingTarget -and ($existingTarget.TrimEnd('\') -ieq $targetPath.TrimEnd('\'))) {
                Write-Step "junction $linkPath already points at $targetPath (no change)"
                return
            }
            # Fall through to in-place retarget via SetTarget. No
            # Remove-Item first — that would open a window where PATH
            # consumers see the junction as missing.
        }
        else {
            Write-ErrorStep "found existing non-junction directory at $linkPath; refusing to replace"
            Write-ErrorStep "  Move or remove $linkPath manually, then re-run the installer."
            Write-ErrorStep "  (The installer needs to put a directory junction there so future"
            Write-ErrorStep "   upgrades retarget the junction instead of overwriting your files.)"
            exit 1
        }
    }
    # Make sure the parent dir exists — CreateFile won't auto-mkdir.
    $parent = Split-Path -Parent $linkPath
    if ($parent -and -not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    Set-JunctionTarget $linkPath $targetPath
    Write-Step "junction $linkPath -> $targetPath"
}

# ---------- Auto-start Scheduled Task (Windows LaunchAgent equivalent) ---

# Thin wrapper that delegates to `cua-driver autostart enable`. The binary
# itself owns the platform-specific registration logic so the install
# scripts and the runtime stay in lock-step — when the verb's behavior
# changes, this script picks it up automatically with no edit needed.
function Test-IsElevated {
    # Returns $true if the current process is running at High IL (admin token).
    $id = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object System.Security.Principal.WindowsPrincipal($id)
    return $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
}

# Stop-CuaDriverDaemons + Show-CuaDriverDaemonSurvivors live in the
# sibling _install-common.psm1 module so install-local.ps1 and this
# script share the daemon-cleanup logic. Two load paths:
#   * checked-out tree: the .psm1 sits next to install.ps1 on disk;
#     Import-Module from $PSScriptRoot works directly.
#   * `irm | iex` install: no file on disk, $PSScriptRoot is empty.
#     Fetch the .psm1 from GitHub raw and Import-Module from a temp
#     file. See Import-CuaDriverInstallModule below (defined inline so
#     it's available before the module load itself).
# Resolve a temp directory that actually exists. $env:TEMP / $env:TMP can point
# at a missing or unresolvable short 8.3 profile path (e.g. C:\Users\SHORTN~1.DOM),
# which makes Set-Content / Remove-Item fail. Pick the first candidate that
# exists, expand 8.3 short components to the long form, and fall back to
# %SystemRoot%\Temp (always present). See issue #1911.
function Get-CuaDriverTempDir {
    foreach ($cand in @($env:TEMP, $env:TMP, (Join-Path $env:LOCALAPPDATA 'Temp'), (Join-Path $env:SystemRoot 'Temp'))) {
        if ([string]::IsNullOrWhiteSpace($cand)) { continue }
        try {
            if (Test-Path -LiteralPath $cand) {
                return (Get-Item -LiteralPath $cand -ErrorAction Stop).FullName
            }
        } catch { }
    }
    $fallback = Join-Path $env:SystemRoot 'Temp'
    if (-not (Test-Path -LiteralPath $fallback)) {
        New-Item -ItemType Directory -Path $fallback -Force | Out-Null
    }
    return $fallback
}

function Import-CuaDriverInstallModuleBootstrap {
    [CmdletBinding()]
    param(
        [string]$LocalDir,
        [Parameter(Mandatory = $true)][string]$Url
    )
    if ($LocalDir) {
        $localPsm = Join-Path $LocalDir "_install-common.psm1"
        if (Test-Path -LiteralPath $localPsm) {
            Import-Module -Name $localPsm -Force -ErrorAction Stop
            return
        }
    }
    $body = Invoke-RestMethod -Uri $Url -UseBasicParsing
    $tmp = Join-Path (Get-CuaDriverTempDir) ("CuaDriverInstall-" + [Guid]::NewGuid().ToString('N') + ".psm1")
    Set-Content -LiteralPath $tmp -Value $body -Encoding UTF8
    try {
        Import-Module -Name $tmp -Force -ErrorAction Stop
    } finally {
        Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
    }
}
Import-CuaDriverInstallModuleBootstrap `
    -LocalDir $PSScriptRoot `
    -Url "https://cua.ai/driver/_install-common.psm1"

function Register-CuaDriverAutostart {
    param([Parameter(Mandatory = $true)][string]$InstalledBinary)

    if (-not (Test-Path -LiteralPath $InstalledBinary)) {
        throw "binary not found at $InstalledBinary"
    }

    # The autostart task is registered with RunLevel=Highest so the daemon runs
    # at the user's elevated/admin token. This is what lets cua-driver drive
    # UWP / AppContainer apps (Calculator, modern Settings, Photos) — at the
    # default Medium IL token, the cross-AppContainer UIA RPC truncates the
    # tree to ~1 element (see issue 1602 / 1601). Registering a RunLevel=Highest
    # task itself requires admin, so we self-elevate if needed and run only
    # the registration step in an elevated PowerShell window. The rest of the
    # install (file extraction, junction creation, User PATH update) stays
    # unelevated as before.
    if (Test-IsElevated) {
        & $InstalledBinary autostart enable
        if ($LASTEXITCODE -ne 0) {
            throw "cua-driver autostart enable failed (exit $LASTEXITCODE)"
        }
        return
    }

    Write-Host ""
    Write-Host "Auto-start at logon needs admin one time to register the" -ForegroundColor Yellow
    Write-Host "Scheduled Task with RunLevel=Highest. A UAC prompt will appear." -ForegroundColor Yellow
    Write-Host "The task itself runs silently at every logon afterwards." -ForegroundColor Yellow
    Write-Host ""

    $elevCmd = "& `"$InstalledBinary`" autostart enable; `$ec = `$LASTEXITCODE; if (`$ec -ne 0) { Read-Host 'cua-driver autostart enable failed; press Enter to close' }; exit `$ec"
    try {
        $proc = Start-Process -FilePath "powershell.exe" `
            -ArgumentList "-NoProfile","-ExecutionPolicy","Bypass","-Command",$elevCmd `
            -Verb RunAs -Wait -PassThru -ErrorAction Stop
        if ($proc.ExitCode -ne 0) {
            throw "cua-driver autostart enable failed in elevated session (exit $($proc.ExitCode))"
        }
    } catch {
        throw "elevation cancelled or failed: $($_.Exception.Message). Re-run install.ps1 -AutoStart from an elevated PowerShell to retry."
    }
}

# ---------- Concurrent-install lockfile -----------------------------------
#
# A second install kicked off while a first is still running can race
# on the junction retarget and leave a half-installed state. Serialize
# installs per $HomeDir with a process-level mutex.
#
# Primitive: System.IO.FileStream opened with FileShare::None. Windows
# kernel rejects a second open of the same file with sharing=None until
# the first handle is closed, so the open call itself is the mutex
# acquisition (no separate ACL trick or named-mutex registration needed,
# no admin rights, no per-process cleanup gymnastics — close = release).
#
# Stale-lock recovery: if the holder dies without closing the handle
# (process kill, host reboot mid-install), Windows reclaims the file
# handle automatically — but only when the *kernel* notices the process
# has exited. From another process's perspective the file is no longer
# locked once that happens, so a fresh FileStream open succeeds without
# any timeout dance.
#
# However if the prior process is somehow still alive but stuck (e.g.
# wedged on a network call), we still want to recover after a bounded
# wait. After $Script:LockStaleAfterSeconds the polling loop probes
# the lockfile by attempting an exclusive open against the same
# FileShare::None primitive: success means the previous holder really
# is gone (its FileStream handle was reclaimed) and the leftover file
# is safe to delete; IOException means the holder is alive but slow,
# and we keep waiting rather than corrupting an in-flight install.
# This is the Windows equivalent of the Linux mkdir-mutex's "force
# release" path, but guarded by a liveness check instead of a blind
# Remove-Item.

$Script:LockPollIntervalSeconds = 1
$Script:LockStaleAfterSeconds   = 600
$Script:StandaloneRoot          = $HomeDir
$Script:LockFilePath            = Join-Path $Script:StandaloneRoot "install.lock"
$Script:LockStream              = $null

function Release-InstallLock {
    if ($Script:LockStream) {
        try { $Script:LockStream.Close() } catch {}
        try { $Script:LockStream.Dispose() } catch {}
        $Script:LockStream = $null
    }
    # Best-effort delete of the lockfile so subsequent installs don't
    # see a stale-but-unlocked file (cosmetic — the FileShare::None
    # primitive doesn't care if the file exists).
    if (Test-Path -LiteralPath $Script:LockFilePath) {
        try { Remove-Item -LiteralPath $Script:LockFilePath -Force -ErrorAction SilentlyContinue } catch {}
    }
}

function Acquire-InstallLock {
    # Ensure parent dir exists before we try to open the file in it.
    if (-not (Test-Path -LiteralPath $Script:StandaloneRoot)) {
        New-Item -ItemType Directory -Force -Path $Script:StandaloneRoot | Out-Null
    }
    $waited = 0
    $announced = $false
    while ($true) {
        try {
            # Mode=OpenOrCreate so the first install creates the file
            # and subsequent installs reuse it. Access=ReadWrite so we
            # can stamp pid/timestamp info after acquiring. Share=None
            # is the actual mutex — second open returns IOException.
            $Script:LockStream = [System.IO.FileStream]::new(
                $Script:LockFilePath,
                [System.IO.FileMode]::OpenOrCreate,
                [System.IO.FileAccess]::ReadWrite,
                [System.IO.FileShare]::None)
            break
        }
        catch [System.IO.IOException] {
            if (-not $announced) {
                Write-Step "another cua-driver-rs install is already in progress (lock at $($Script:LockFilePath)); waiting..."
                $announced = $true
            }
            Start-Sleep -Seconds $Script:LockPollIntervalSeconds
            $waited += $Script:LockPollIntervalSeconds
            if ($waited -ge $Script:LockStaleAfterSeconds) {
                # Don't yank the lock from a live install. Probe the
                # file with the same Share=None primitive — if the open
                # succeeds, the previous holder's FileStream is really
                # gone (process exited, kernel reclaimed the handle)
                # and the lockfile is just a stale leftover safe to
                # delete. If it still throws IOException, the holder is
                # alive but slow (big download, wedged network); keep
                # waiting rather than corrupting an in-flight install.
                $probeStream = $null
                try {
                    $probeStream = [System.IO.FileStream]::new(
                        $Script:LockFilePath,
                        [System.IO.FileMode]::Open,
                        [System.IO.FileAccess]::ReadWrite,
                        [System.IO.FileShare]::None)
                    # Acquisition succeeded → previous holder is gone.
                    # Close the probe so we can Remove-Item cleanly,
                    # then fall through to the next loop iteration
                    # which will reopen via the normal OpenOrCreate path
                    # (and re-stamp the info blob).
                    $probeStream.Close()
                    $probeStream.Dispose()
                    $probeStream = $null
                    try { Remove-Item -LiteralPath $Script:LockFilePath -Force -ErrorAction SilentlyContinue } catch {}
                    Write-WarningStep "previous install lock at $($Script:LockFilePath) appeared stale and was released"
                    $waited = 0
                }
                catch [System.IO.IOException] {
                    # Still locked by a live process. Reset waited so we
                    # re-check after another full window rather than
                    # spamming this branch every poll interval.
                    Write-Step "lock at $($Script:LockFilePath) still held by a live process; continuing to wait"
                    $waited = 0
                }
                finally {
                    if ($probeStream) {
                        try { $probeStream.Close() } catch {}
                        try { $probeStream.Dispose() } catch {}
                    }
                }
            }
        }
    }
    # Stamp pid + ISO timestamp + argv into the lockfile so a user
    # investigating a stuck install can see who holds it (Get-Content
    # $env:USERPROFILE\.cua-driver-rs\install.lock).
    try {
        $info = "pid=$PID`nstarted=$([DateTime]::UtcNow.ToString('o'))`ninvocation=install.ps1 -Release $Release`n"
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($info)
        $Script:LockStream.SetLength(0)
        $Script:LockStream.Write($bytes, 0, $bytes.Length)
        $Script:LockStream.Flush()
    }
    catch {
        # Non-fatal — failing to stamp info doesn't affect the lock.
    }
}

# ---------- Per-version release dir GC ------------------------------------
#
# Prune per-version release dirs under $ReleasesDir for the current
# $target, keeping the N most recent (by LastWriteTime). The dir that
# the `current` junction resolves to is always preserved on top of the
# keep budget — so the worst-case post-GC count is keep + 1 (active
# fell outside the keep window, e.g. after a rollback), common case
# is exactly keep. Per-target filtering keeps a multi-arch dev's
# x86_64 and arm64 histories independent of each other.

function Invoke-OldReleasesGc {
    param(
        [string]$releasesDir,
        [string]$currentDir,
        [string]$target,
        [int]$keep
    )

    if ($keep -eq 0) {
        Write-Step "version GC disabled (`$env:CUA_DRIVER_RS_KEEP_VERSIONS=0)"
        return
    }
    if (-not (Test-Path -LiteralPath $releasesDir)) { return }

    # Resolve $currentDir's junction target so we can exempt it from the
    # prune list. Get-JunctionTarget returns $null on non-junction paths,
    # in which case nothing is exempted (still safe — we only prune the
    # excess past $keep newest).
    $currentTarget = $null
    if (Test-Path -LiteralPath $currentDir) {
        if (Test-IsJunction $currentDir) {
            $currentTarget = Get-JunctionTarget $currentDir
            if ($currentTarget) { $currentTarget = $currentTarget.TrimEnd('\') }
        }
    }

    # Filter to dirs whose name ends in "-$target". Sort newest-first.
    # Wrap in @(...) so a single-result match doesn't get unwrapped to a
    # bare object (PowerShell's classic foot-gun).
    $candidates = @(Get-ChildItem -LiteralPath $releasesDir -Directory -ErrorAction SilentlyContinue |
                    Where-Object { $_.Name -like "*-$target" } |
                    Sort-Object LastWriteTime -Descending)

    if ($candidates.Count -eq 0) { return }

    $toPrune = @()
    $kept = 0
    foreach ($cand in $candidates) {
        $isCurrent = ($currentTarget -and ($cand.FullName.TrimEnd('\') -ieq $currentTarget))
        if ($kept -lt $keep) {
            $kept += 1
            continue
        }
        if ($isCurrent) {
            # Active install fell outside the keep window — preserve
            # anyway (never delete the dir backing the active junction).
            continue
        }
        $toPrune += $cand
    }

    if ($toPrune.Count -eq 0) { return }

    Write-Step "pruning $($toPrune.Count) old release dir(s) (keeping $keep most recent for $target):"
    foreach ($d in $toPrune) {
        Write-Step "  - $($d.Name)"
        Remove-Item -LiteralPath $d.FullName -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# ---------- Release resolution --------------------------------------------

function Resolve-Version {
    if ($env:CUA_DRIVER_RS_VERSION) {
        $v = $env:CUA_DRIVER_RS_VERSION -replace '^v', ''
        Write-Step "using version from `$env:CUA_DRIVER_RS_VERSION: $v"
        return $v
    }
    if ($Release -ne "latest") {
        $v = $Release -replace '^v', ''
        Write-Step "using -Release $v"
        return $v
    }
    # Baked-version fallback — set by the CD workflow after each release
    # so the default `irm | iex` install path doesn't hit the GitHub API.
    # See the BAKED_VERSION sentinel-block near the top of this file.
    if ($Script:CuaDriverRsBakedVersion) {
        $v = $Script:CuaDriverRsBakedVersion -replace '^v', ''
        Write-Step "using baked release: $TagPrefix$v"
        return $v
    }
    Write-Step "resolving latest $TagPrefix* release via GitHub API"
    # Paginate the /releases endpoint until we've seen every release or
    # collected enough $TagPrefix* matches to be confident the latest is
    # in hand. A single page (even at per_page=100) is not guaranteed to
    # include any cua-driver-rs-v* tag — the repo also ships Swift
    # cua-driver-v* releases plus other unrelated tags, which can push
    # our matches off the first page once the release cadence grows.
    #
    # Loop guards:
    #   - max 10 pages = 1000 releases — way more than the repo will
    #     ever hold, but cheap insurance against an unbounded loop.
    #   - stop early when a page comes back empty (we've exhausted the
    #     list).
    $matches = @()
    for ($page = 1; $page -le 10; $page++) {
        $uri = "https://api.github.com/repos/$Repo/releases?per_page=100&page=$page"
        $batch = Invoke-RestMethod -Uri $uri -UseBasicParsing
        if (-not $batch -or $batch.Count -eq 0) { break }
        $matches += @($batch | Where-Object { $_.tag_name -like "$TagPrefix*" })
        if ($batch.Count -lt 100) { break }
    }
    if (-not $matches -or $matches.Count -eq 0) {
        Write-ErrorStep "no release matching $TagPrefix* found on $Repo"
        exit 1
    }
    # Sort by SemVer descending. [version] correctly orders dotted triples.
    $latest = $matches | Sort-Object {
        $v = $_.tag_name.Substring($TagPrefix.Length)
        try { [version]$v } catch { [version]"0.0.0" }
    } -Descending | Select-Object -First 1
    $version = $latest.tag_name.Substring($TagPrefix.Length)
    Write-Step "latest release: $($latest.tag_name)"
    return $version
}

# ---------- Download + extract --------------------------------------------

function Get-ReleaseAsset([string]$version, [string]$archLabel, [string]$destDir) {
    $zipName = "cua-driver-rs-$version-$archLabel.zip"
    $url     = "https://github.com/$Repo/releases/download/$TagPrefix$version/$zipName"
    $zipPath = Join-Path $destDir $zipName

    Write-Step "downloading $url"
    try {
        Invoke-WebRequest -Uri $url -OutFile $zipPath -UseBasicParsing
    }
    catch {
        Write-ErrorStep "download failed: $($_.Exception.Message)"
        Write-ErrorStep "  Try pinning a known-good version via `$env:CUA_DRIVER_RS_VERSION = '<x.y.z>'`."
        exit 1
    }

    Write-Step "extracting $zipName"
    $extractDir = Join-Path $destDir "extracted"
    if (Test-Path -LiteralPath $extractDir) {
        Remove-Item -LiteralPath $extractDir -Force -Recurse
    }
    Expand-Archive -LiteralPath $zipPath -DestinationPath $extractDir -Force

    # Directory zip from the CD workflow expands to
    # cua-driver-rs-<v>-<arch>\cua-driver.exe (+ LICENSE).
    $stage = "cua-driver-rs-$version-$archLabel"
    $stageDir = Join-Path $extractDir $stage
    if (-not (Test-Path -LiteralPath (Join-Path $stageDir $BinaryName))) {
        Write-ErrorStep "expected $BinaryName inside $zipName but didn't find it"
        Get-ChildItem $extractDir -Recurse | ForEach-Object { Write-Host "  $($_.FullName)" }
        exit 1
    }
    return $stageDir
}

# ---------- Main -----------------------------------------------------------

Write-Step "cua-driver-rs installer (Windows)"
Write-Step "  install dir : $VisibleBinDir"
Write-Step "  package home: $HomeDir"

function Remove-LegacyInstall {
    # Best-effort cleanup of v0.2.13-and-earlier install paths. Runs before
    # any new install when default paths are in use (so users who override
    # CUA_DRIVER_RS_INSTALL_DIR / CUA_DRIVER_RS_HOME aren't surprised by us
    # touching legacy locations). Mirrors uninstall.ps1's logic so the
    # transition is symmetric: a single `irm install.ps1 | iex` upgrades
    # from v0.2.13 → v0.2.14+ without orphan files at the old layout.
    if ($env:CUA_DRIVER_RS_INSTALL_DIR -or $env:CUA_DRIVER_RS_HOME) {
        return
    }
    $hasLegacy = (Test-Path -LiteralPath $LegacyVisibleBinDir) -or `
                 (Test-Path -LiteralPath $LegacyHomeDir)
    if (-not $hasLegacy) { return }

    Write-Step "detected legacy install layout (v0.2.13 or earlier); migrating to Cua\cua-driver"

    # 1. End the running daemon. Order matters:
    #
    #    a. `schtasks /End` first — Task Scheduler runs as SYSTEM and can
    #       terminate elevated (RunLevel=Highest, High IL) processes that a
    #       Medium-IL Stop-Process from the user's shell cannot. This is
    #       the case any time the legacy daemon was spawned via the
    #       AtLogon trigger of the v0.2.13 autostart task on a non-RID-500
    #       admin account (e.g. cuademo). Without this, Step 4 below fails
    #       with "Access to the path 'cua-driver.exe' is denied" because
    #       the legacy binary is held open by an unkillable elevated
    #       process. Discovered during the cuademo v0.2.13 → v0.2.14
    #       migration dogfood.
    #    b. taskkill /F /IM as a backstop for any cua-driver process that
    #       wasn't task-attached (manual `cua-driver serve`, legacy uia
    #       worker, etc.). taskkill is more permissive than Stop-Process
    #       for cross-IL termination.
    #    c. Stop-Process last — catches anything taskkill missed.
    $prevEAP = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        # Ends the running task instance. Returns non-zero when the task
        # isn't running or doesn't exist, both of which we swallow.
        & schtasks.exe /End /TN "cua-driver-serve" 2>$null | Out-Null
        Start-Sleep -Milliseconds 250
        # Force-kill via taskkill — handles High-IL processes that
        # Stop-Process can't touch from a Medium-IL caller.
        & taskkill.exe /F /IM "cua-driver.exe" /T 2>$null | Out-Null
        & taskkill.exe /F /IM "cua-driver-uia.exe" /T 2>$null | Out-Null
    } finally {
        $ErrorActionPreference = $prevEAP
    }
    $procs = Get-Process -Name "cua-driver","cua-driver-uia" -ErrorAction SilentlyContinue
    if ($procs) {
        foreach ($p in $procs) {
            try { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } catch {}
        }
    }
    Start-Sleep -Milliseconds 500

    # 2. Unregister the autostart Scheduled Task if present. Idempotent —
    #    schtasks /Delete returns non-zero when the task is absent, which
    #    we swallow.
    $prevEAP = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & schtasks.exe /Delete /TN "cua-driver-serve" /F 2>$null | Out-Null
    } finally {
        $ErrorActionPreference = $prevEAP
    }

    # 3. Remove the visible bin junction (only when it's actually a reparse
    #    point — refuse to clobber a real directory). Then walk up and
    #    remove the empty `trycua` vendor dir if nothing else lives there.
    if (Test-Path -LiteralPath $LegacyVisibleBinDir) {
        try {
            $item = Get-Item -LiteralPath $LegacyVisibleBinDir -Force -ErrorAction Stop
            $isReparse = ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0
            if ($isReparse) {
                # NTFS junction — delete the link, not the target.
                [System.IO.Directory]::Delete($LegacyVisibleBinDir, $false)
            } else {
                Remove-Item -LiteralPath $LegacyVisibleBinDir -Recurse -Force -ErrorAction SilentlyContinue
            }
        } catch {
            Write-Host "  (could not remove $LegacyVisibleBinDir : $($_.Exception.Message))" -ForegroundColor Yellow
        }
    }
    # Remove the parent `cua-driver-rs` dir (now empty) and the vendor
    # `trycua` dir if no other apps live under it.
    $legacyParent = Split-Path -Parent $LegacyVisibleBinDir
    if ((Test-Path -LiteralPath $legacyParent) -and -not (Get-ChildItem -LiteralPath $legacyParent -Force -ErrorAction SilentlyContinue)) {
        Remove-Item -LiteralPath $legacyParent -Force -ErrorAction SilentlyContinue
    }
    if ((Test-Path -LiteralPath $LegacyVendorDir) -and -not (Get-ChildItem -LiteralPath $LegacyVendorDir -Force -ErrorAction SilentlyContinue)) {
        Remove-Item -LiteralPath $LegacyVendorDir -Force -ErrorAction SilentlyContinue
    }

    # 4. Remove the legacy package home tree.
    if (Test-Path -LiteralPath $LegacyHomeDir) {
        try {
            New-Item -ItemType Directory -Force -Path $HomeDir | Out-Null
            foreach ($telemetryFile in @('.telemetry_id', '.installation_recorded')) {
                $legacyTelemetryPath = Join-Path $LegacyHomeDir $telemetryFile
                $currentTelemetryPath = Join-Path $HomeDir $telemetryFile
                if ((Test-Path -LiteralPath $legacyTelemetryPath) -and -not (Test-Path -LiteralPath $currentTelemetryPath)) {
                    Copy-Item -LiteralPath $legacyTelemetryPath -Destination $currentTelemetryPath -Force -ErrorAction Stop
                }
            }
            Remove-Item -LiteralPath $LegacyHomeDir -Recurse -Force -ErrorAction Stop
        } catch {
            Write-Host "  (could not remove $LegacyHomeDir : $($_.Exception.Message))" -ForegroundColor Yellow
        }
    }

    # 5. Prune the legacy bin dir from User PATH. The new install will add
    #    the new path right after this; without removing the old we'd
    #    accumulate stale PATH entries on every upgrade.
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($userPath) {
        $legacyNorm = $LegacyVisibleBinDir.TrimEnd('\').ToLowerInvariant()
        $cleaned = ($userPath -split ';' |
            Where-Object { $_ -and $_.TrimEnd('\').ToLowerInvariant() -ne $legacyNorm }) -join ';'
        if ($cleaned -ne $userPath) {
            [Environment]::SetEnvironmentVariable('Path', $cleaned, 'User')
            Write-Step "  pruned legacy $LegacyVisibleBinDir from User PATH"
        }
    }

    Write-Step "legacy install removed"
}

Remove-LegacyInstall

# Serialize concurrent installs per $HomeDir. The lock is released in
# the finally below — covers normal exit, errors, and Ctrl-C (which
# triggers PowerShell's pipeline-stop = finally still runs).
Acquire-InstallLock

try {

$target    = Get-TargetTriple
$archLabel = Get-AssetArchLabel $target
Write-Step "  target      : $target"

$version = Resolve-Version
$versionedDir = Join-Path $ReleasesDir "$version-$target"

# If the per-version dir is already populated, skip the download — the
# binary is immutable per version, so a repeat install is a no-op apart
# from retargeting the `current` junction (cheap rollback path).
$skipDownload = $false
if (Test-Path -LiteralPath (Join-Path $versionedDir $BinaryName)) {
    Write-Step "release $version is already on disk at $versionedDir (skipping download)"
    $skipDownload = $true
}

if (-not $skipDownload) {
    $tmpRoot = Join-Path (Get-CuaDriverTempDir) ("cua-driver-rs-install-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $tmpRoot | Out-Null
    try {
        $stageDir = Get-ReleaseAsset $version $archLabel $tmpRoot
        New-Item -ItemType Directory -Force -Path $versionedDir | Out-Null
        Copy-Item -LiteralPath (Join-Path $stageDir $BinaryName) -Destination (Join-Path $versionedDir $BinaryName) -Force
        Write-Step "installed $versionedDir\$BinaryName (version $version, target $target)"
        # Optional sibling: the uiAccess'd worker (cua-driver-uia.exe). Started
        # shipping with cua-driver-rs-v0.2.8; absent in earlier releases. Copy
        # it when present so `cua-driver autostart enable` can register the
        # second ShellExecute-based scheduled task. See #1602.
        $uiaStage = Join-Path $stageDir 'cua-driver-uia.exe'
        if (Test-Path -LiteralPath $uiaStage) {
            Copy-Item -LiteralPath $uiaStage -Destination (Join-Path $versionedDir 'cua-driver-uia.exe') -Force
            Write-Step "installed $versionedDir\cua-driver-uia.exe (uiAccess worker)"
        }
    }
    finally {
        Remove-Item -LiteralPath $tmpRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# Persist the bounded installer channel before the new junction target becomes
# visible. If a user command wins the lifecycle lock before the detached hook,
# the runtime reads this hint and preserves installer attribution. It removes
# the hint after lifecycle delivery succeeds.
$allowedChannels = @("install_script", "update_apply", "python_package", "first_run")
$installChannel = $env:CUA_DRIVER_INSTALL_CHANNEL
if (-not $installChannel -or $installChannel -notin $allowedChannels) {
    $installChannel = "install_script"
}

# Mirror the runtime's consent precedence before writing the attribution hint:
# environment override, compatibility override, persisted preference, default-on.
$telemetryHintEnabled = $true
$telemetryHintFromEnvironment = $false
foreach ($telemetryEnvironmentName in @('CUA_DRIVER_RS_TELEMETRY_ENABLED', 'CUA_TELEMETRY_ENABLED')) {
    $telemetryEnvironmentValue = [Environment]::GetEnvironmentVariable($telemetryEnvironmentName)
    if ($null -eq $telemetryEnvironmentValue) {
        continue
    }
    $telemetryEnvironmentValue = $telemetryEnvironmentValue.Trim().ToLowerInvariant()
    if ($telemetryEnvironmentValue -in @('1', 'true', 'yes', 'on')) {
        $telemetryHintEnabled = $true
        $telemetryHintFromEnvironment = $true
        break
    }
    if ($telemetryEnvironmentValue -in @('0', 'false', 'no', 'off')) {
        $telemetryHintEnabled = $false
        $telemetryHintFromEnvironment = $true
        break
    }
}
if (-not $telemetryHintFromEnvironment) {
    $telemetryConfigPath = Join-Path $HomeDir 'config.json'
    if (Test-Path -LiteralPath $telemetryConfigPath) {
        try {
            $telemetryConfig = Get-Content -LiteralPath $telemetryConfigPath -Raw | ConvertFrom-Json
            $telemetryPreference = $telemetryConfig.PSObject.Properties['telemetry_enabled']
            if ($null -ne $telemetryPreference -and $telemetryPreference.Value -is [bool]) {
                $telemetryHintEnabled = $telemetryPreference.Value
            }
        }
        catch {
            # Match the runtime: malformed config falls through to default-on.
        }
    }
}
$telemetryHintPath = Join-Path $HomeDir '.telemetry_install_channel'
if ($telemetryHintEnabled) {
    New-Item -ItemType Directory -Force -Path $HomeDir | Out-Null
    Set-Content -LiteralPath $telemetryHintPath -Value $installChannel -Encoding Ascii -NoNewline
}
else {
    Remove-Item -LiteralPath $telemetryHintPath -Force -ErrorAction SilentlyContinue
}

# Wire up the junction chain. The inner junction (current → releases\<v>)
# is what makes the upgrade atomic; the outer junction (bin → current)
# is what gives users a stable PATH entry.
Ensure-Junction $CurrentDir    $versionedDir
Ensure-Junction $VisibleBinDir $CurrentDir

# Post-install GC of old per-version release dirs. Runs AFTER the junction
# retarget above so the about-to-be-active version is never a deletion
# candidate (it's both the newest by mtime and exempted via the
# current-junction check inside Invoke-OldReleasesGc).
$keepVersions = Resolve-KeepVersions
Invoke-OldReleasesGc -releasesDir $ReleasesDir -currentDir $CurrentDir -target $target -keep $keepVersions

# ---------- Record consent-aware install telemetry ------------------------
#
# Same shape as the Unix installer. The binary applies the normal effective
# consent policy, preserves the v1 registration marker, and records this
# release once per version. Keep the channel bounded before allowing it into
# analytics.
$installedBinary = Join-Path $VisibleBinDir $BinaryName
if (Test-Path -LiteralPath $installedBinary) {
    Write-Host "Telemetry defaults to enabled for new installations; saved preferences and environment overrides are honored." -ForegroundColor Cyan
    Write-Host "When enabled, Cua collects a pseudonymous installation ID and bounded, content-free usage metadata." -ForegroundColor Cyan
    Write-Host "  No prompts, tool arguments, screen contents, or file paths are collected."
    Write-Host "  Disable persistently at any time: $installedBinary telemetry disable"
    try {
        # install.ps1 commonly runs via `irm | iex` in the caller's shell.
        # Restore both variables after Start-Process snapshots the environment
        # so the install does not leak transient attribution into that shell.
        $savedChannel = $env:CUA_DRIVER_INSTALL_CHANNEL
        $savedReleaseVersion = $env:CUA_DRIVER_RELEASE_VERSION
        try {
            $env:CUA_DRIVER_INSTALL_CHANNEL = $installChannel
            $env:CUA_DRIVER_RELEASE_VERSION = $version
            Start-Process -FilePath $installedBinary -ArgumentList "telemetry","install-event" `
                          -WindowStyle Hidden -ErrorAction SilentlyContinue | Out-Null
        }
        finally {
            if ($null -eq $savedChannel) {
                Remove-Item Env:CUA_DRIVER_INSTALL_CHANNEL -ErrorAction SilentlyContinue
            } else {
                $env:CUA_DRIVER_INSTALL_CHANNEL = $savedChannel
            }
            if ($null -eq $savedReleaseVersion) {
                Remove-Item Env:CUA_DRIVER_RELEASE_VERSION -ErrorAction SilentlyContinue
            } else {
                $env:CUA_DRIVER_RELEASE_VERSION = $savedReleaseVersion
            }
        }
    }
    catch {
        # Ignore — telemetry must never block install.
    }
}

# ---------- PATH update (User scope, idempotent, fallback to manual) ------
#
# We append $VisibleBinDir to the User-scope PATH so `cua-driver` resolves
# in any newly-spawned shell. User scope (not Machine) keeps the installer
# non-admin — Machine scope would require elevation. The write doesn't
# affect the calling shell's $env:Path; the post-install message tells the
# user to open a new shell (or stitch $env:Path manually for this session).
#
# Failure modes we tolerate gracefully:
#   - Locked-down accounts where SetEnvironmentVariable throws (group
#     policy, OneDrive-redirected HKCU, etc.). We catch, print the
#     manual command, and exit success.
#   - User passed -NoPathUpdate. Same fallback message, no write attempt.
#
# Idempotency: if $VisibleBinDir is already on the User PATH we no-op
# silently (same shape as the existing "already added" branch).
function Get-UserPathEntries {
    $raw = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not $raw) { return @() }
    return @($raw.Split(';') | Where-Object { $_ })
}

function Test-OnUserPath([string]$dir) {
    $needle = $dir.TrimEnd('\')
    foreach ($entry in (Get-UserPathEntries)) {
        if ($entry.TrimEnd('\') -ieq $needle) { return $true }
    }
    return $false
}

function Add-UserPathEntry([string]$dir) {
    $existing = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($existing) {
        $newValue = ($existing.TrimEnd(';')) + ';' + $dir
    } else {
        $newValue = $dir
    }
    [Environment]::SetEnvironmentVariable("Path", $newValue, "User")

    # Also update the CURRENT process's $env:Path so `cua-driver` resolves
    # immediately in the same shell — the SetEnvironmentVariable('User') call
    # above only writes to the registry; existing processes have their
    # $env:Path cached at launch time and don't see the update otherwise.
    # The install.ps1 body runs in the caller's shell via iex (per the
    # `irm install.ps1 | iex` one-liner), so writing $env:Path here mutates
    # exactly the shell the user is typing into. See #1651.
    if (-not (($env:Path -split ';') -contains $dir)) {
        $env:Path = "$dir;$env:Path"
    }
}

function Write-ManualPathInstructions([string]$dir) {
    Write-Host "$dir is not on your user PATH yet." -ForegroundColor Yellow
    Write-Host "Add it for future shells with:" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"`$([Environment]::GetEnvironmentVariable('Path','User'));$dir`", 'User')"
    Write-Host ""
    Write-Host "To use it in the current session immediately:"
    Write-Host ""
    Write-Host "  `$env:Path += `";$dir`""
    Write-Host ""
}

Write-Host ""
Write-Host "cua-driver-rs $version installed."
Write-Host ""
$onPath = Test-OnUserPath $VisibleBinDir
if ($onPath) {
    Write-Host "$VisibleBinDir is on your user PATH -- cua-driver should resolve in any new shell."
    Write-Host ""
}
elseif ($NoPathUpdate) {
    Write-Host "Skipping PATH update (-NoPathUpdate set)." -ForegroundColor Yellow
    Write-ManualPathInstructions $VisibleBinDir
}
else {
    try {
        Add-UserPathEntry $VisibleBinDir
        Write-Host "Added $VisibleBinDir to your User PATH." -ForegroundColor Green
        Write-Host '  cua-driver resolves immediately in THIS shell and in any new shell.'
        Write-Host '  Opt out next time with: install.ps1 -NoPathUpdate'
        Write-Host ""
    }
    catch {
        Write-WarningStep "could not auto-update User PATH: $($_.Exception.Message)"
        Write-ManualPathInstructions $VisibleBinDir
    }
}

# Kill any cua-driver / cua-driver-uia process still running off the
# OLD binary, so the next time the daemon is invoked (autostart kick,
# manual `cua-driver mcp`, MCP client startup) it picks up the freshly-
# installed code. Without this, in-memory daemons keep serving old
# behaviour - which surfaces as "the bug I just patched is still
# there" because the user's in-memory code is pre-fix. Best-effort:
# High-IL daemons from the RunLevel=Highest autostart task survive a
# Medium-IL kill and get reported via Show-CuaDriverDaemonSurvivors.
Write-Host ""
Write-Host "Stopping any previous cua-driver processes (best-effort; High-IL needs admin)..." -ForegroundColor Cyan
# Repair- handles the wedged-daemon case: detect stale (process alive
# but pipe dead), prompt the user, and on consent self-elevate via
# UAC to kill the High-IL pids + restart the scheduled task. On UAC
# cancel / failure it falls back to printing the manual recovery
# instructions, same as the previous behavior.
$null = Repair-CuaDriverStaleDaemon

if ($AutoStart) {
    Write-Host ""
    Write-Host "Registering auto-start (cua-driver autostart enable)..." -ForegroundColor Cyan
    try {
        Register-CuaDriverAutostart -InstalledBinary $installedBinary
        Write-Host "  cua-driver serve will auto-start at every interactive logon (RunLevel=Highest)." -ForegroundColor Green
    }
    catch {
        Write-Host "  Failed: $($_.Exception.Message)" -ForegroundColor Red
        Write-Host '  Install otherwise succeeded; from an elevated shell run: cua-driver autostart enable'
        Write-Host "  In THIS shell (if already elevated), use: $installedBinary autostart enable"
        Write-Host ""
    }
} else {
    # No -AutoStart, but if a `cua-driver-serve` task is already
    # registered, re-register it against the fresh binary. Otherwise
    # the task <Command> still points at the previous release dir + an
    # older binary that may be missing the hidden-console wrapper (#1654)
    # or any later autostart-shape fix.
    $prevEAP = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & schtasks.exe /Query /TN "cua-driver-serve" 2>$null | Out-Null
        $hasTask = ($LASTEXITCODE -eq 0)
    } finally {
        $ErrorActionPreference = $prevEAP
    }
    if ($hasTask) {
        Write-Host ""
        Write-Host "Existing 'cua-driver-serve' autostart task detected - re-registering against the fresh binary..." -ForegroundColor Cyan
        try {
            Register-CuaDriverAutostart -InstalledBinary $installedBinary
            Write-Host "  Re-registered. Task action now uses this build's hidden-console wrapper." -ForegroundColor Green
        }
        catch {
            Write-Host "  Failed to re-register: $($_.Exception.Message)" -ForegroundColor Red
            Write-Host "  The existing task still points at the previous binary. Run 'cua-driver autostart enable' from an elevated shell to update."
        }
    }
}

# Unified post-install hints come from a single shared text file so the
# 4 Rust installers (this script + _install-rust.sh + install-local.ps1 +
# install-local.sh) never drift. The .txt holds the OS-agnostic bulk
# (Try-it / skill pack / MCP setup / docs link) with {{BINARY}}
# placeholders; OS-specific bits (autostart) stay inline below.
$HintsUrl = "https://cua.ai/driver/post-install-hints.txt"
try {
    $hintsRaw = (Invoke-WebRequest -Uri $HintsUrl -UseBasicParsing -TimeoutSec 10).Content
    Write-Host ($hintsRaw -replace '\{\{BINARY\}\}', $installedBinary)
}
catch {
    # Network fetch failed — print one-line essentials so users always
    # get enough to recover.
    Write-Host "Next steps: $installedBinary --version  |  $installedBinary mcp-config  |  $installedBinary skills install"
    Write-Host "Docs: https://github.com/trycua/cua/tree/main/libs/cua-driver/rust"
}

# Windows-specific autostart hint (kept inline; OS-natural location).
Write-Host ""
if ($AutoStart) {
    Write-Host "Auto-start: 'cua-driver-serve' is registered at RunLevel=Highest." -ForegroundColor Cyan
    Write-Host "  cua-driver autostart status    (inspect)" -ForegroundColor Cyan
    Write-Host "  cua-driver autostart disable   (remove)" -ForegroundColor Cyan
    Write-Host "  cua-driver autostart kick      (start now without re-logging)" -ForegroundColor Cyan
} else {
    Write-Host "Auto-start at logon (NOT enabled - re-run without -NoAutoStart to register, or:):" -ForegroundColor Cyan
    Write-Host "  cua-driver autostart enable    (Scheduled Task at RunLevel=Highest)" -ForegroundColor Cyan
    Write-Host "  cua-driver autostart kick      (start now without re-logging)" -ForegroundColor Cyan
    Write-Host "  cua-driver autostart status    (inspect)" -ForegroundColor Cyan
    Write-Host "  cua-driver autostart disable   (remove)" -ForegroundColor Cyan
}
}
finally {
    # Always release the lock — success, error, Ctrl-C, or `exit` all
    # land here. A partial install + held lock would wedge every
    # subsequent install for the full $LockStaleAfterSeconds window.
    Release-InstallLock
}
