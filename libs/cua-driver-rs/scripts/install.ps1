# cua-driver-rs installer (Windows) — download the latest cua-driver-rs
# release zip from GitHub Releases and wire it up via a chain of
# directory junctions, so future upgrades / rollbacks retarget a
# junction instead of overwriting files. Sudo-free, no Developer Mode
# required, no admin elevation.
#
# Usage (one-liner — recommended):
#   irm https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver-rs/scripts/install.ps1 | iex
#
# Pin a version:
#   $env:CUA_DRIVER_RS_VERSION = "0.2.0"
#   irm https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver-rs/scripts/install.ps1 | iex
#
# Layout on disk (three tiers, two directory junctions):
#
#   <visibleBinDir>            [directory junction → currentDir]
#     = %LOCALAPPDATA%\Programs\trycua\cua-driver-rs\bin
#   <currentDir>               [directory junction → release dir]
#     = %USERPROFILE%\.cua-driver-rs\packages\current
#   <release dir>              [real directory, immutable per version]
#     = %USERPROFILE%\.cua-driver-rs\packages\releases\<version>-<target>
#         cua-driver.exe
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
#                                    (default %LOCALAPPDATA%\Programs\trycua\cua-driver-rs\bin)
#   $env:CUA_DRIVER_RS_HOME          override the package home
#                                    (default %USERPROFILE%\.cua-driver-rs)
#
# Param:
#   -Release   release tag to install ("latest" or a bare version like "0.2.0").
#              Overridden by $env:CUA_DRIVER_RS_VERSION when set.
#
# WARNING — BETA: cua-driver-rs is the cross-platform Rust port of the
# Swift cua-driver. Windows and Linux support is feature-complete;
# macOS parity is in progress and tracked separately.

[CmdletBinding()]
param(
    [string]$Release = "latest"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
# Invoke-WebRequest's progress bar is ~10x slower than the actual download
# over PowerShell ISE and Windows PowerShell 5 — silence it. Restored
# nowhere on purpose: this script is a one-shot, the user can re-set it.
$ProgressPreference = "SilentlyContinue"

$Repo       = "trycua/cua"
$TagPrefix  = "cua-driver-rs-v"
$BinaryName = "cua-driver.exe"

# ---------- Path resolution ------------------------------------------------

if ($env:CUA_DRIVER_RS_INSTALL_DIR) {
    $VisibleBinDir = $env:CUA_DRIVER_RS_INSTALL_DIR
} else {
    $VisibleBinDir = Join-Path $env:LOCALAPPDATA "Programs\trycua\cua-driver-rs\bin"
}

if ($env:CUA_DRIVER_RS_HOME) {
    $HomeDir = $env:CUA_DRIVER_RS_HOME
} else {
    $HomeDir = Join-Path $env:USERPROFILE ".cua-driver-rs"
}

$PackagesDir = Join-Path $HomeDir   "packages"
$ReleasesDir = Join-Path $PackagesDir "releases"
$CurrentDir  = Join-Path $PackagesDir "current"

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
    $osArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($osArch) {
        "X64"   { return "x86_64-pc-windows-msvc" }
        "Arm64" { return "aarch64-pc-windows-msvc" }
        default {
            Write-ErrorStep "unsupported Windows architecture: $osArch"
            Write-ErrorStep "  cua-driver-rs ships prebuilts for: x86_64 (X64) and arm64 (Arm64)."
            Write-ErrorStep "  Please file an issue at https://github.com/trycua/cua/issues with the output of"
            Write-ErrorStep "  '[System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture'."
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
function Ensure-Junction([string]$linkPath, [string]$targetPath) {
    if (Test-Path -LiteralPath $linkPath) {
        if (Test-IsJunction $linkPath) {
            # Existing junction — retarget it. Removing the empty
            # reparse-point dir and recreating it is the simplest way to
            # change the target atomically from PowerShell's POV (the
            # underlying DeviceIoControl will fail with "directory not
            # empty" otherwise).
            $existingTarget = Get-JunctionTarget $linkPath
            if ($existingTarget -and ($existingTarget.TrimEnd('\') -ieq $targetPath.TrimEnd('\'))) {
                Write-Step "junction $linkPath already points at $targetPath (no change)"
                return
            }
            # Remove-Item on a junction removes the link, not the target.
            # -Force handles read-only / hidden attributes.
            Remove-Item -LiteralPath $linkPath -Force -Recurse
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
    Write-Step "resolving latest $TagPrefix* release via GitHub API"
    # Pull the first 40 releases so we still find the latest one even when
    # several unrelated tag prefixes have shipped recently. The cua-driver-rs
    # tag prefix is distinct from the Swift cua-driver tag prefix (one extra
    # "-rs-"), so the simple StartsWith filter is unambiguous.
    $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases?per_page=40" `
                                  -UseBasicParsing
    $matches = $releases | Where-Object { $_.tag_name -like "$TagPrefix*" }
    if (-not $matches) {
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
    $tmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("cua-driver-rs-install-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $tmpRoot | Out-Null
    try {
        $stageDir = Get-ReleaseAsset $version $archLabel $tmpRoot
        New-Item -ItemType Directory -Force -Path $versionedDir | Out-Null
        Copy-Item -LiteralPath (Join-Path $stageDir $BinaryName) -Destination (Join-Path $versionedDir $BinaryName) -Force
        Write-Step "installed $versionedDir\$BinaryName (version $version, target $target)"
    }
    finally {
        Remove-Item -LiteralPath $tmpRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# Wire up the junction chain. The inner junction (current → releases\<v>)
# is what makes the upgrade atomic; the outer junction (bin → current)
# is what gives users a stable PATH entry.
Ensure-Junction $CurrentDir    $versionedDir
Ensure-Junction $VisibleBinDir $CurrentDir

# ---------- Fire-and-forget install telemetry ping ------------------------
#
# Same shape as the Linux install.sh path: invoke `cua-driver telemetry
# install-event` once per install. The binary itself guards against
# double-counting via ~\.cua-driver-rs\.installation_recorded.
$installedBinary = Join-Path $VisibleBinDir $BinaryName
if (Test-Path -LiteralPath $installedBinary) {
    try {
        Start-Process -FilePath $installedBinary -ArgumentList "telemetry","install-event" `
                      -WindowStyle Hidden -ErrorAction SilentlyContinue | Out-Null
    }
    catch {
        # Ignore — telemetry must never block install.
    }
}

# ---------- PATH check (info only — we don't auto-edit on Windows) --------
#
# Modifying the user's PATH from PowerShell is doable
# ([Environment]::SetEnvironmentVariable("Path", ..., "User")) but it
# (a) requires the user's old PATH be sane, (b) doesn't take effect in
# the calling shell anyway, and (c) is the kind of side-effect that
# surprises power users. So we print a hint with the exact command
# instead and let the user opt in.
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$onPath   = $false
if ($userPath) {
    $entries = $userPath.Split(';') | Where-Object { $_ }
    foreach ($entry in $entries) {
        if ($entry.TrimEnd('\') -ieq $VisibleBinDir.TrimEnd('\')) {
            $onPath = $true
            break
        }
    }
}

Write-Host ""
Write-Host "cua-driver-rs $version installed."
Write-Host ""
Write-Host "Try it:"
Write-Host "  $installedBinary --version"
Write-Host ""

if (-not $onPath) {
    Write-Host "$VisibleBinDir is not on your user PATH yet." -ForegroundColor Yellow
    Write-Host "Add it for future shells with:" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"`$([Environment]::GetEnvironmentVariable('Path','User'));$VisibleBinDir`", 'User')"
    Write-Host ""
    Write-Host "Then open a new PowerShell window. To use it in the current session:"
    Write-Host ""
    Write-Host "  `$env:Path += `";$VisibleBinDir`""
    Write-Host ""
}
else {
    Write-Host "$VisibleBinDir is on your user PATH — cua-driver should resolve in any new shell."
    Write-Host ""
}

Write-Host "Docs: https://github.com/trycua/cua/tree/main/libs/cua-driver-rs"
Write-Host ""
Write-Host "WARNING — BETA: cua-driver-rs is a cross-platform Rust port of the Swift" -ForegroundColor Yellow
Write-Host "          cua-driver. Windows and Linux support is feature-complete; macOS" -ForegroundColor Yellow
Write-Host "          parity is in progress." -ForegroundColor Yellow
