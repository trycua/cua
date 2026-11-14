# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Names and checks shared by install-local.ps1 and uninstall-local.ps1 (Cua
# Spaces Local on Windows; the counterpart of apps/cua-spaces-macos/scripts/
# _local-common.sh). Dot-sourced; no side effects besides the definitions.
# Works in Windows PowerShell 5.1 and PowerShell 7.
#
# THE NAMES BELOW MIRROR src/channel.ts (the app's own) and
# packaging/electron-builder.local.cjs, and `CuaChannel::Local` in
# libs/cua/crates/cua-home/src/channel.rs for the CLI and home names;
# scripts/tests/install-local.tests.ps1 fails when one stops matching.

$script:Local = @{
    AppDirName      = 'Cua Spaces Local'
    ExeName         = 'Cua Spaces Local.exe'
    CliName         = 'cua-local'
    CliExeName      = 'cua-local.exe'
    AppId           = 'ai.cua.spaces.desktop.local'
    UrlScheme       = 'cua-local'
    HomeDirName     = '.cua-local'
    UserDataDirName = 'Cua Spaces Local'
    ShortcutName    = 'Cua Spaces Local.lnk'
    MarkerName      = '.cua-spaces-local.json'
}

# What the released Cua Spaces owns. Never created, replaced or removed here.
$script:Released = @{
    AppId       = 'ai.cua.spaces.desktop'
    UrlScheme   = 'cua'
    HomeDirName = '.cua'
    CliExeName  = 'cua.exe'
}

function Write-Step([string]$Message) { Write-Host "==> $Message" -ForegroundColor Cyan }
function Write-Ok([string]$Message) { Write-Host $Message -ForegroundColor Green }
function Write-Warn([string]$Message) { Write-Host "warning: $Message" -ForegroundColor Yellow }
function Write-Err([string]$Message) { [Console]::Error.WriteLine("error: $Message") }

# --- Paths ---------------------------------------------------------------------

# The full path with `.`, `..` and the trailing separator resolved, and a link
# (symlink or junction) anywhere along the existing part replaced by its target,
# so nothing in a path can hide where it lands.
function Resolve-RealPath([string]$Path) {
    $full = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($full)
    $rest = $full.Substring($root.Length).Split([char[]]@('\', '/'), [System.StringSplitOptions]::RemoveEmptyEntries)
    $current = $root
    foreach ($segment in $rest) {
        $next = Join-Path $current $segment
        if (Test-Path -LiteralPath $next) {
            $item = Get-Item -LiteralPath $next -Force
            if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
                $target = @($item.Target)[0]
                if ($target) {
                    if (-not [System.IO.Path]::IsPathRooted($target)) { $target = Join-Path $current $target }
                    $next = Resolve-RealPath $target
                }
            }
        }
        $current = $next
    }
    if ($current.Length -le 3) { return $current }
    return $current.TrimEnd('\', '/')
}

function Test-PathInside([string]$Child, [string]$Parent) {
    $c = $Child.TrimEnd('\', '/').ToLowerInvariant()
    $p = $Parent.TrimEnd('\', '/').ToLowerInvariant()
    return ($c -eq $p) -or $c.StartsWith($p + '\')
}

# The roots the released Cua Spaces owns, for this user and machine. The
# released installer's folder is Programs\cua-spaces (the package name) or, in
# older builds, Programs\Cua Spaces; the CLI it puts on PATH is Programs\cua\bin.
function Get-ReleasedRoots([string]$LocalAppData, [string]$UserProfile, [string]$ProgramFiles) {
    $roots = @(
        (Join-Path $LocalAppData 'Programs\cua-spaces'),
        (Join-Path $LocalAppData 'Programs\Cua Spaces'),
        (Join-Path $LocalAppData 'Programs\Cua'),
        (Join-Path $UserProfile $script:Released.HomeDirName)
    )
    if ($ProgramFiles) {
        $roots += (Join-Path $ProgramFiles 'Cua Spaces')
        $roots += (Join-Path $ProgramFiles 'cua-spaces')
    }
    return $roots
}

# The released root `Path` is, is inside, or contains; $null when none. A path
# that holds a released root would delete it on replace, so both directions
# count. (Windows paths compare case-insensitively.)
function Find-ReleasedOverlap([string]$Path, [string[]]$Roots) {
    foreach ($r in $Roots) {
        if ((Test-PathInside $Path $r) -or (Test-PathInside $r $Path)) { return $r }
    }
    return $null
}

# --- What is at the install path ------------------------------------------------

# 'absent', 'empty', 'local' (our marker, with the Local app id) or 'foreign'
# (anything else: a released app, someone's files). Only absent, empty and local
# may be written; foreign is refused, so the script never replaces what it did
# not install.
function Get-InstallState([string]$Dir) {
    if (-not (Test-Path -LiteralPath $Dir)) { return 'absent' }
    $item = Get-Item -LiteralPath $Dir -Force
    if (-not $item.PSIsContainer) { return 'foreign' }
    if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { return 'foreign' }
    $entries = @(Get-ChildItem -LiteralPath $Dir -Force)
    if ($entries.Count -eq 0) { return 'empty' }
    $marker = Join-Path $Dir $script:Local.MarkerName
    if (-not (Test-Path -LiteralPath $marker -PathType Leaf)) { return 'foreign' }
    try { $info = Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json } catch { return 'foreign' }
    if ($info.appId -ne $script:Local.AppId) { return 'foreign' }
    return 'local'
}

# --- PATH (user scope) ----------------------------------------------------------

function Test-PathListHas([string]$List, [string]$Entry) {
    $e = $Entry.TrimEnd('\').ToLowerInvariant()
    foreach ($part in ($List -split ';')) {
        if ($part -and $part.TrimEnd('\').ToLowerInvariant() -eq $e) { return $true }
    }
    return $false
}

function Add-PathListEntry([string]$List, [string]$Entry) {
    if (Test-PathListHas $List $Entry) { return $List }
    if ([string]::IsNullOrEmpty($List)) { return $Entry }
    return $List.TrimEnd(';') + ';' + $Entry
}

function Remove-PathListEntry([string]$List, [string]$Entry) {
    $e = $Entry.TrimEnd('\').ToLowerInvariant()
    $kept = @(($List -split ';') | Where-Object { $_ -and $_.TrimEnd('\').ToLowerInvariant() -ne $e })
    return ($kept -join ';')
}

# --- Versions -------------------------------------------------------------------

# Node 22 or later.
function Test-NodeVersionOk([string]$Version) {
    if ($Version -notmatch '^v?(\d+)\.') { return $false }
    return [int]$Matches[1] -ge 22
}

# The pinned wasm-bindgen version out of apps/cua-spaces-web/scripts/build-core-wasm.mjs,
# and the toolchain channel out of a rust-toolchain.toml; $null when not there.
function Get-WasmBindgenPin([string]$ScriptPath) {
    if (-not (Test-Path -LiteralPath $ScriptPath)) { return $null }
    $m = Select-String -LiteralPath $ScriptPath -Pattern '^const WASM_BINDGEN = "(.+)"' | Select-Object -First 1
    if ($m) { return $m.Matches[0].Groups[1].Value }
    return $null
}

function Get-ToolchainChannel([string]$TomlPath) {
    if (-not (Test-Path -LiteralPath $TomlPath)) { return $null }
    $m = Select-String -LiteralPath $TomlPath -Pattern '^channel\s*=\s*"(.+)"' | Select-Object -First 1
    if ($m) { return $m.Matches[0].Groups[1].Value }
    return $null
}

# --- Processes ------------------------------------------------------------------

# Processes running a program from inside `Dir` (the Local app, its daemon):
# nothing outside the folder is ever named.
function Get-ProcessesUnder([string]$Dir) {
    $found = @()
    foreach ($p in (Get-Process -ErrorAction SilentlyContinue)) {
        $path = $null
        try { $path = $p.Path } catch { }
        if ($path -and (Test-PathInside $path $Dir)) { $found += $p }
    }
    return $found
}

function Stop-ProcessesUnder([string]$Dir) {
    $procs = @(Get-ProcessesUnder $Dir)
    foreach ($p in $procs) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
    if ($procs.Count -gt 0) { Start-Sleep -Milliseconds 800 }
    return $procs.Count
}

# --- Windows integration ---------------------------------------------------------

$script:ShortcutSource = @'
using System;
using System.Runtime.InteropServices;
public static class CuaLocalShortcut {
    [ComImport, Guid("00021401-0000-0000-C000-000000000046")] class CShellLink { }
    [ComImport, InterfaceType(ComInterfaceType.InterfaceIsIUnknown), Guid("000214F9-0000-0000-C000-000000000046")]
    interface IShellLinkW {
        void GetPath([Out, MarshalAs(UnmanagedType.LPWStr)] System.Text.StringBuilder f, int c, IntPtr d, int fl);
        void GetIDList(out IntPtr p); void SetIDList(IntPtr p);
        void GetDescription([Out, MarshalAs(UnmanagedType.LPWStr)] System.Text.StringBuilder f, int c);
        void SetDescription([MarshalAs(UnmanagedType.LPWStr)] string d);
        void GetWorkingDirectory([Out, MarshalAs(UnmanagedType.LPWStr)] System.Text.StringBuilder f, int c);
        void SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string d);
        void GetArguments([Out, MarshalAs(UnmanagedType.LPWStr)] System.Text.StringBuilder f, int c);
        void SetArguments([MarshalAs(UnmanagedType.LPWStr)] string a);
        void GetHotkey(out short h); void SetHotkey(short h);
        void GetShowCmd(out int s); void SetShowCmd(int s);
        void GetIconLocation([Out, MarshalAs(UnmanagedType.LPWStr)] System.Text.StringBuilder f, int c, out int i);
        void SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string f, int i);
        void SetRelativePath([MarshalAs(UnmanagedType.LPWStr)] string p, int r);
        void Resolve(IntPtr h, int f);
        void SetPath([MarshalAs(UnmanagedType.LPWStr)] string p);
    }
    [StructLayout(LayoutKind.Sequential, Pack = 4)] struct PROPERTYKEY { public Guid fmtid; public uint pid; }
    [StructLayout(LayoutKind.Explicit)] struct PROPVARIANT {
        [FieldOffset(0)] public ushort vt; [FieldOffset(8)] public IntPtr p;
    }
    [ComImport, InterfaceType(ComInterfaceType.InterfaceIsIUnknown), Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99")]
    interface IPropertyStore {
        void GetCount(out uint c); void GetAt(uint i, out PROPERTYKEY k);
        void GetValue(ref PROPERTYKEY k, out PROPVARIANT v);
        void SetValue(ref PROPERTYKEY k, ref PROPVARIANT v); void Commit();
    }
    [ComImport, InterfaceType(ComInterfaceType.InterfaceIsIUnknown), Guid("0000010b-0000-0000-C000-000000000046")]
    interface IPersistFile {
        void GetClassID(out Guid g); [PreserveSig] int IsDirty();
        void Load([MarshalAs(UnmanagedType.LPWStr)] string f, int m);
        void Save([MarshalAs(UnmanagedType.LPWStr)] string f, [MarshalAs(UnmanagedType.Bool)] bool r);
        void SaveCompleted([MarshalAs(UnmanagedType.LPWStr)] string f);
        void GetCurFile([MarshalAs(UnmanagedType.LPWStr)] out string f);
    }
    [DllImport("ole32.dll")] static extern int PropVariantClear(ref PROPVARIANT v);
    public static void Create(string link, string target, string dir, string aumid, string desc) {
        IShellLinkW sl = (IShellLinkW)new CShellLink();
        sl.SetPath(target); sl.SetWorkingDirectory(dir); sl.SetDescription(desc);
        sl.SetIconLocation(target, 0);
        IPropertyStore ps = (IPropertyStore)sl;
        PROPERTYKEY key = new PROPERTYKEY { fmtid = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), pid = 5 };
        PROPVARIANT v = new PROPVARIANT { vt = 31, p = Marshal.StringToCoTaskMemUni(aumid) };
        try { ps.SetValue(ref key, ref v); ps.Commit(); } finally { PropVariantClear(ref v); }
        ((IPersistFile)sl).Save(link, true);
    }
}
'@

# A Start Menu shortcut that carries the app's AppUserModelID (the one
# src/channel.ts gives the app), so the taskbar groups the window under it and
# toasts name it; IShellLink and IPropertyStore, which PowerShell has no cmdlet for.
function New-AppShortcut([string]$LinkPath, [string]$Target, [string]$WorkingDir, [string]$AppUserModelId, [string]$Description) {
    if (-not ('CuaLocalShortcut' -as [type])) { Add-Type -TypeDefinition $script:ShortcutSource }
    $dir = Split-Path -Parent $LinkPath
    if (-not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    [CuaLocalShortcut]::Create($LinkPath, $Target, $WorkingDir, $AppUserModelId, $Description)
}

# Tells running programs (Explorer, new terminals) that the environment changed.
function Send-EnvironmentChange {
    if (-not ('CuaLocalNative.Win' -as [type])) {
        Add-Type -Namespace CuaLocalNative -Name Win -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr h, uint m, System.UIntPtr w, string l, uint f, uint t, out System.UIntPtr r);
'@
    }
    $r = [UIntPtr]::Zero
    [void][CuaLocalNative.Win]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 2000, [ref]$r)
}

# The user PATH as stored: [Environment]::GetEnvironmentVariable expands %VARS% and writing it back would
# store every entry expanded (and change the value's type), so the registry value is read and written as it is.
function Get-UserPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $false)
    try {
        $value = $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        return [string]$value
    } finally { $key.Close() }
}
function Set-UserPath([string]$Value) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    try { $key.SetValue('Path', $Value, [Microsoft.Win32.RegistryValueKind]::ExpandString) } finally { $key.Close() }
    Send-EnvironmentChange
}

# Whether `Text` contains `Part`, as plain text (a path can hold wildcard characters), any case.
function Test-ContainsText([string]$Text, [string]$Part) {
    return ($Text.IndexOf($Part, [System.StringComparison]::OrdinalIgnoreCase) -ge 0)
}

# The protocol key for cua-local:// (what Electron's setAsDefaultProtocolClient
# writes as well), so links work before the first launch. Removed again only
# when its command still points into the Local app.
function Register-LocalProtocol([string]$Exe) {
    $key = "HKCU:\Software\Classes\$($script:Local.UrlScheme)"
    New-Item -Path $key -Force | Out-Null
    Set-ItemProperty -Path $key -Name '(default)' -Value "URL:$($script:Local.AppDirName)"
    Set-ItemProperty -Path $key -Name 'URL Protocol' -Value ''
    New-Item -Path "$key\shell\open\command" -Force | Out-Null
    Set-ItemProperty -Path "$key\shell\open\command" -Name '(default)' -Value ('"{0}" "%1"' -f $Exe)
}

function Get-LocalProtocolCommand {
    $key = "HKCU:\Software\Classes\$($script:Local.UrlScheme)\shell\open\command"
    if (-not (Test-Path $key)) { return $null }
    return (Get-ItemProperty -Path $key).'(default)'
}
