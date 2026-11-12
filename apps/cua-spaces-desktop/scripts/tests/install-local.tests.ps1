# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Tests the pure parts of scripts\install-local.ps1 and uninstall-local.ps1
# (the path, refusal, PATH-list and version checks in _local-common.ps1), and
# that the names in it are the ones the app (src\channel.ts), the builder config
# and CuaChannel::Local have. Builds nothing and touches no registry, PATH, Start
# Menu or real home: everything runs in a temporary folder. Plain PowerShell,
# works in Windows PowerShell 5.1 and PowerShell 7:
#
#   powershell -NoProfile -File scripts\tests\install-local.tests.ps1
#
# Each check prints PASS or FAIL; the exit status is the number of FAILs.
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$scripts = Split-Path -Parent $here
$app = Split-Path -Parent $scripts
. (Join-Path $scripts '_local-common.ps1')

$script:fails = 0
function Check([string]$Name, [bool]$Condition) {
    if ($Condition) { Write-Host "PASS  $Name" } else { Write-Host "FAIL  $Name"; $script:fails++ }
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("cua-install-local-test-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
try {
    # --- Released roots and overlap -------------------------------------------------
    $lad = Join-Path $work 'AppData\Local'
    $prof = Join-Path $work 'profile'
    $pf = Join-Path $work 'Program Files'
    $roots = Get-ReleasedRoots $lad $prof $pf
    $localApp = Join-Path $lad 'Programs\Cua Spaces Local'
    Check 'the Local app folder overlaps no released root' ($null -eq (Find-ReleasedOverlap $localApp $roots))
    Check 'Programs\cua-spaces is the released app' ($null -ne (Find-ReleasedOverlap (Join-Path $lad 'Programs\cua-spaces') $roots))
    Check 'a folder inside the released app is refused' ($null -ne (Find-ReleasedOverlap (Join-Path $lad 'Programs\cua-spaces\resources') $roots))
    Check 'Programs\Cua Spaces (older release) is refused, in any case' ($null -ne (Find-ReleasedOverlap (Join-Path $lad 'programs\CUA SPACES') $roots))
    Check 'the released home ~\.cua is refused' ($null -ne (Find-ReleasedOverlap (Join-Path $prof '.cua') $roots))
    Check '~\.cua-local is not the released home (a prefix is not a path)' ($null -eq (Find-ReleasedOverlap (Join-Path $prof '.cua-local') $roots))
    Check 'a folder that holds a released root is refused' ($null -ne (Find-ReleasedOverlap (Join-Path $lad 'Programs') $roots))
    Check 'Program Files\Cua Spaces is refused' ($null -ne (Find-ReleasedOverlap (Join-Path $pf 'Cua Spaces\x') $roots))
    Check 'Test-PathInside compares whole segments' (-not (Test-PathInside 'C:\a\bc' 'C:\a\b'))
    Check 'Test-PathInside is case-insensitive' (Test-PathInside 'C:\A\B\c' 'c:\a\b')

    # --- Resolve-RealPath: .. and links ---------------------------------------------
    $real = Join-Path $work 'real'
    New-Item -ItemType Directory -Path $real | Out-Null
    Check 'Resolve-RealPath resolves ..' ((Resolve-RealPath (Join-Path $real '..\real\x')) -eq (Join-Path $real 'x'))
    $link = Join-Path $work 'link'
    $linked = $false
    try { New-Item -ItemType Junction -Path $link -Target $real | Out-Null; $linked = $true } catch { Write-Host 'SKIP  junction (cannot create one here)' }
    if ($linked) {
        Check 'Resolve-RealPath follows a junction to its target' ((Resolve-RealPath (Join-Path $link 'x')) -eq (Join-Path $real 'x'))
        $hidden = Resolve-RealPath (Join-Path $link 'x')
        Check 'a junction to a released root cannot hide it' ($null -ne (Find-ReleasedOverlap (Resolve-RealPath (Join-Path $link 'y')) @($real)))
    }

    # --- Install state: only absent, empty and Local are writable -------------------
    $d = Join-Path $work 'state'
    Check 'an absent folder is absent' ((Get-InstallState $d) -eq 'absent')
    New-Item -ItemType Directory -Path $d | Out-Null
    Check 'an empty folder is empty' ((Get-InstallState $d) -eq 'empty')
    Set-Content -LiteralPath (Join-Path $d 'file.txt') 'x'
    Check 'a folder of someone else''s files is foreign' ((Get-InstallState $d) -eq 'foreign')
    Set-Content -LiteralPath (Join-Path $d $Local.MarkerName) '{"appId":"ai.cua.spaces.desktop"}'
    Check 'a marker with the released app id is foreign' ((Get-InstallState $d) -eq 'foreign')
    Set-Content -LiteralPath (Join-Path $d $Local.MarkerName) 'not json'
    Check 'a broken marker is foreign' ((Get-InstallState $d) -eq 'foreign')
    Set-Content -LiteralPath (Join-Path $d $Local.MarkerName) ('{"appId":"' + $Local.AppId + '"}')
    Check 'a marker with the Local app id is local' ((Get-InstallState $d) -eq 'local')
    $f = Join-Path $work 'afile'
    Set-Content -LiteralPath $f 'x'
    Check 'a file at the path is foreign' ((Get-InstallState $f) -eq 'foreign')

    # --- PATH list ---------------------------------------------------------------------
    $bin = 'C:\Users\me\AppData\Local\Programs\Cua Spaces Local\bin'
    $relBin = 'C:\Users\me\AppData\Local\Programs\cua\bin'
    $list = "C:\Windows;$relBin"
    $added = Add-PathListEntry $list $bin
    Check 'Add-PathListEntry appends' ($added -eq "$list;$bin")
    Check 'Add-PathListEntry is idempotent (any case, trailing slash)' ((Add-PathListEntry $added ($bin.ToUpper() + '\')) -eq $added)
    Check 'Add-PathListEntry on an empty PATH' ((Add-PathListEntry '' $bin) -eq $bin)
    Check 'Remove-PathListEntry removes only that entry' ((Remove-PathListEntry $added $bin) -eq $list)
    Check 'Remove-PathListEntry keeps the released cua folder' ((Remove-PathListEntry $added $bin) -like "*$relBin*")

    Check 'Test-ContainsText is plain text, any case (a path may hold [ or *)' ((Test-ContainsText 'x C:\A[1]\App.exe y' 'c:\a[1]\app.exe') -and -not (Test-ContainsText 'C:\a\b' 'C:\a*'))

    # --- Versions and pins -----------------------------------------------------------------
    Check 'Node 22 is enough' (Test-NodeVersionOk 'v22.0.0')
    Check 'Node 24 is enough' (Test-NodeVersionOk 'v24.13.0')
    Check 'Node 20 is not' (-not (Test-NodeVersionOk 'v20.11.1'))
    Check 'garbage is not a Node version' (-not (Test-NodeVersionOk 'node'))
    $pin = Get-WasmBindgenPin (Join-Path $app '..\cua-spaces-web\scripts\build-core-wasm.mjs')
    Check 'the wasm-bindgen pin is read from the web build script' ($pin -match '^\d+\.\d+\.\d+$')
    $chan = Get-ToolchainChannel (Join-Path $app '..\cua-spaces\rust-toolchain.toml')
    Check 'the Rust toolchain channel is read' ($chan -match '^\d+\.\d+\.\d+$')

    # --- The names are the app's, the builder's and CuaChannel::Local's -----------------
    $ts = Get-Content -LiteralPath (Join-Path $app 'src\channel.ts') -Raw
    Check 'src/channel.ts has the Local app id' ($ts.Contains('"' + $Local.AppId + '"'))
    Check 'src/channel.ts has the Local URL scheme' ($ts.Contains('"' + $Local.UrlScheme + '"'))
    Check 'src/channel.ts has the Local home folder' ($ts.Contains('"' + $Local.HomeDirName + '"'))
    Check 'src/channel.ts has the Local user data folder' ($ts.Contains('"' + $Local.UserDataDirName + '"'))
    Check 'src/channel.ts names the Local executable' ($ts.Contains($Local.ExeName.ToLowerInvariant()))
    Check 'the Local app id is the released one plus .local' ($Local.AppId -eq ($Released.AppId + '.local'))
    $cfg = Get-Content -LiteralPath (Join-Path $app 'packaging\electron-builder.local.cjs') -Raw
    Check 'the builder config has the Local app id' ($cfg.Contains('"' + $Local.AppId + '"'))
    Check 'the builder config has the Local product name (the executable)' ($cfg.Contains('productName: "' + $Local.AppDirName + '"'))
    Check 'the builder config has the Local scheme' ($cfg.Contains('"' + $Local.UrlScheme + '"'))
    Check 'the builder config has no update feed' ($cfg -match 'publish:\s*null')
    $rs = Get-Content -LiteralPath (Join-Path $app '..\..\libs\cua\crates\cua-home\src\channel.rs') -Raw
    Check 'CuaChannel::Local has the CLI name' ($rs.Contains('"' + $Local.CliName + '"'))
    Check 'CuaChannel::Local has the home folder' ($rs.Contains('"' + $Local.HomeDirName + '"'))
    Check 'CuaChannel::Local has the URL scheme' ($rs.Contains('"' + $Local.UrlScheme + '"'))
    Check 'the CLI is not named like the released cua.exe' ($Local.CliExeName -ne $Released.CliExeName)

    # --- The scripts parse (5.1 and 7) --------------------------------------------------------
    foreach ($s in @('install-local.ps1', 'uninstall-local.ps1', '_local-common.ps1')) {
        $errors = $null
        [void][System.Management.Automation.Language.Parser]::ParseFile((Join-Path $scripts $s), [ref]$null, [ref]$errors)
        Check "$s parses" ($errors.Count -eq 0)
    }
    Check 'the scripts hold no em dash (scripts/check-no-em-dash.sh)' (-not ((Get-ChildItem -LiteralPath $scripts -Filter '*.ps1' -Recurse | Select-String -Pattern ([string][char]0x2014)) | Select-Object -First 1))
} finally {
    Remove-Item -LiteralPath $link -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

if ($script:fails -gt 0) { Write-Host "$($script:fails) check(s) failed."; exit $script:fails }
Write-Host 'All checks passed.'
exit 0
