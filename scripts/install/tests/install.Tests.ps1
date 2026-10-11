# Pester 5 tests for install.ps1. Hermetic: temp dirs, file:// release, no
# real installs (the app step is exercised with -DryRun only).
#   pwsh -c "Invoke-Pester scripts/install/tests -Output Detailed"

BeforeAll {
    function global:ConvertTo-FileUrl([string]$Path) {
        $p = $Path -replace '\\', '/'
        if (-not $p.StartsWith('/')) { $p = "/$p" }
        return "file://$p"
    }
    $script:Installer = Join-Path $PSScriptRoot '..' 'install.ps1'
    $script:Tool = Join-Path $PSScriptRoot '..' 'release_manifest.py'
    $script:Python = (Get-Command python3 -ErrorAction SilentlyContinue) ?? (Get-Command python)

    $script:Root = Join-Path ([IO.Path]::GetTempPath()) ("cua-ps-tests-" + [Guid]::NewGuid().ToString('N'))
    $script:Rel = Join-Path $Root 'release'
    New-Item -ItemType Directory -Force -Path $Rel, (Join-Path $Root 'stage') | Out-Null
    Set-Content -LiteralPath (Join-Path $Root 'stage/cua.exe') -Value 'fake cua'
    foreach ($arch in 'x64', 'arm64') {
        Compress-Archive -Path (Join-Path $Root 'stage/cua.exe') -DestinationPath (Join-Path $Rel "cua-cli-1.2.3-windows-$arch.zip")
        Set-Content -LiteralPath (Join-Path $Rel "cua-spaces-1.2.3-windows-$arch.msi") -Value 'msi'
        Set-Content -LiteralPath (Join-Path $Rel "cua-spaces-1.2.3-windows-$arch-setup.exe") -Value 'nsis'
    }
    $cli = Join-Path $Root 'cli.json'
    & $Python.Source $Tool --component cli --version 1.2.3 --dir $Rel --out $cli
    & $Python.Source $Tool --component app --version 1.2.3 --dir $Rel --merge $cli --out (Join-Path $Rel 'release-artifacts.json')
    $env:CUA_TEST_MANIFEST_URL = ConvertTo-FileUrl (Join-Path $Rel 'release-artifacts.json')
    $env:CUA_TEST_ROOT = $Root
    $env:CUA_TEST_INSTALLER = $Installer

    function script:Invoke-Installer([hashtable]$Params, [hashtable]$Vars = @{}) {
        $saved = @{}
        $all = @{ CUA_INSTALL_MANIFEST_URL = $env:CUA_TEST_MANIFEST_URL; CUA_INSTALL_ARCH = 'AMD64'; CUA_HOME = (Join-Path $env:CUA_TEST_ROOT 'cuahome') }
        foreach ($k in $Vars.Keys) { $all[$k] = $Vars[$k] }
        foreach ($k in $all.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k); [Environment]::SetEnvironmentVariable($k, $all[$k]) }
        try {
            return (& $env:CUA_TEST_INSTALLER @Params *>&1 | Out-String)
        } finally {
            foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
        }
    }
}

AfterAll {
    Remove-Item -Recurse -Force -LiteralPath $env:CUA_TEST_ROOT -ErrorAction SilentlyContinue
}

Describe 'functions' {
    BeforeAll {
        $env:CUA_INSTALL_NO_RUN = '1'
        . $env:CUA_TEST_INSTALLER
        $env:CUA_INSTALL_NO_RUN = $null
    }

    It 'only downloads over HTTPS (loopback HTTP for tests)' {
        { Assert-DownloadUrl 'https://github.com/trycua/cua/a.zip' } | Should -Not -Throw
        { Assert-DownloadUrl 'http://127.0.0.1:8080/a.zip' } | Should -Not -Throw
        { Assert-DownloadUrl 'http://localhost/a.zip' } | Should -Not -Throw
        { Assert-DownloadUrl 'http://example.com/a.zip' } | Should -Throw '*non-HTTPS*'
        { Assert-DownloadUrl 'ftp://example.com/a.zip' } | Should -Throw '*non-HTTPS*'
        { Assert-DownloadUrl 'http://127.0.0.1:1@example.com/a.zip' } | Should -Throw '*non-HTTPS*'
        { Assert-DownloadUrl 'file:///tmp/a.zip' 'https://github.com/r.json' } | Should -Throw '*remote manifest*'
        { Assert-DownloadUrl 'file:///tmp/a.zip' 'file:///tmp/r.json' } | Should -Not -Throw
    }

    It 'maps architectures' {
        Get-CuaPlatform -Arch 'AMD64' | Should -Be 'windows-x64'
        Get-CuaPlatform -Arch 'ARM64' | Should -Be 'windows-arm64'
        { Get-CuaPlatform -Arch 'x86' } | Should -Throw '*unsupported CPU architecture*'
    }

    It 'builds silent MSI and NSIS commands with the install mode' {
        $msi = Get-AppInstallCommand 'C:\t\a.msi' 'msi' 'host'
        $msi.FilePath | Should -Be 'msiexec.exe'
        $msi.ArgumentList | Should -Contain '/qn'
        $msi.ArgumentList | Should -Contain 'CUA_SPACES_MODE=host'
        $nsis = Get-AppInstallCommand 'C:\t\a.exe' 'nsis' 'client'
        $nsis.ArgumentList | Should -Be @('/S', '/MODE=client')
        (Get-AppInstallCommand 'C:\t\a.msi' 'msi' '').ArgumentList | Should -Not -Contain 'CUA_SPACES_MODE='
    }

    It 'parses item lists' {
        ConvertTo-CuaItems 'cli,cua-driver, host,cua-driver' | Should -Be @('cua-driver', 'host')
        @(ConvertTo-CuaItems '') | Should -HaveCount 0
        { ConvertTo-CuaItems 'bogus' } | Should -Throw "*unknown item 'bogus' (valid: cli, spaces, cua-driver, host)*"
    }

    It 'selects defaults, -Select, -Only and the legacy switches' {
        $d = Get-CuaSelection -SelectList '' -OnlyList '' -Cli $false -App $false -InstallMode ''
        @($d.cli, $d.spaces, $d['cua-driver'], $d.host) | Should -Be @($true, $false, $false, $false)
        (Get-CuaSelection -SelectList 'cua-driver' -OnlyList '' -Cli $false -App $false -InstallMode '')['cua-driver'] | Should -BeTrue
        $o = Get-CuaSelection -SelectList '' -OnlyList 'host' -Cli $false -App $false -InstallMode ''
        @($o.cli, $o.spaces, $o.host) | Should -Be @($true, $false, $true)
        $a = Get-CuaSelection -SelectList '' -OnlyList '' -Cli $false -App $true -InstallMode ''
        @($a.cli, $a.spaces) | Should -Be @($false, $true)
        (Get-CuaSelection -SelectList '' -OnlyList '' -Cli $false -App $false -InstallMode 'host').spaces | Should -BeTrue
        (Get-CuaSelection -SelectList '' -OnlyList '' -Cli $true -App $false -InstallMode 'host').spaces | Should -BeFalse
        (Get-CuaSelection -SelectList 'spaces' -OnlyList '' -Cli $false -App $false -InstallMode '').spaces | Should -BeTrue
    }

    It 'rejects conflicting selections' {
        { Get-CuaSelection -SelectList 'host' -OnlyList 'host' -Cli $false -App $false -InstallMode '' } | Should -Throw '*mutually exclusive*'
        { Get-CuaSelection -SelectList 'spaces' -OnlyList '' -Cli $true -App $false -InstallMode '' } | Should -Throw '*use -Only spaces*'
        { Get-CuaSelection -SelectList '' -OnlyList 'cua-driver' -Cli $false -App $true -InstallMode '' } | Should -Throw '*cua-driver and host need*'
        { Get-CuaSelection -SelectList '' -OnlyList '' -Cli $true -App $true -InstallMode '' } | Should -Throw '*mutually exclusive*'
    }

    It 'toggles items in the numbered checklist; the CLI row is locked' {
        $script:answers = [Collections.Generic.Queue[string]]::new([string[]]@('1 2', '3', ''))
        $sel = Get-CuaSelection -SelectList '' -OnlyList '' -Cli $false -App $false -InstallMode ''
        $sel = Read-CuaSelection $sel { $script:answers.Dequeue() } 6>$null
        @($sel.cli, $sel.spaces, $sel['cua-driver'], $sel.host) | Should -Be @($true, $false, $true, $true)
        $script:Picked | Should -BeTrue
        $script:Picked = $false
    }

    It 'shows Spaces in the checklist only when preselected' {
        $script:answers = [Collections.Generic.Queue[string]]::new([string[]]@('2', ''))
        $sel = Get-CuaSelection -SelectList 'spaces' -OnlyList '' -Cli $false -App $false -InstallMode ''
        $sel = Read-CuaSelection $sel { $script:answers.Dequeue() } 6>$null
        @($sel.spaces, $sel['cua-driver']) | Should -Be @($false, $false)
        $script:Picked = $false
    }

    It 'forwards -RequireSignature and -NoPathUpdate to the cua-driver installer' {
        $RequireSignature = $false; $ModifyPath = $false
        Get-DriverInstallArgs | Should -Be @('-NoPathUpdate')
        $RequireSignature = $true; $ModifyPath = $true
        Get-DriverInstallArgs | Should -Be @('-RequireSignature')
    }

    It 'runs the cua-driver installer, then registers the skill and MCP server' -Skip:($IsWindows) {
        $root = Join-Path $env:CUA_TEST_ROOT 'drv'
        $prefix = Join-Path $root 'prefix'
        New-Item -ItemType Directory -Force -Path $root | Out-Null
        $log = Join-Path $root 'driver.log'
        $cuaLog = Join-Path $root 'cua.log'
        $fakeDriver = Join-Path $root 'install.ps1'
        Set-Content -LiteralPath $fakeDriver -Value @"
param([switch]`$NoPathUpdate, [switch]`$RequireSignature)
"NoPathUpdate=`$NoPathUpdate RequireSignature=`$RequireSignature dir=`$env:CUA_DRIVER_RS_INSTALL_DIR" | Set-Content '$log'
New-Item -ItemType Directory -Force -Path `$env:CUA_DRIVER_RS_INSTALL_DIR | Out-Null
Set-Content (Join-Path `$env:CUA_DRIVER_RS_INSTALL_DIR 'cua-driver.exe') 'fake'
"@
        $fakeCua = Join-Path $root 'cua'
        Set-Content -LiteralPath $fakeCua -Value "#!/bin/sh`necho `"`$*`" >>'$cuaLog'"
        chmod +x $fakeCua
        $env:CUA_INSTALL_DRIVER_URL = ConvertTo-FileUrl $fakeDriver
        $Prefix = $prefix; $DryRun = $false; $RequireSignature = $true; $ModifyPath = $false
        try {
            Install-CuaDriver $fakeCua (New-Item -ItemType Directory -Force -Path (Join-Path $root 'tmp')).FullName 'file:///r.json' 6>$null
        } finally { $env:CUA_INSTALL_DRIVER_URL = $null }
        $bin = Join-Path $prefix 'bin'
        Get-Content $log | Should -Be "NoPathUpdate=True RequireSignature=True dir=$bin"
        Get-Content $cuaLog | Should -Be "agents setup --cua-driver --agents all --yes --mcp-command $(Join-Path $bin 'cua-driver.exe')"
        $env:CUA_DRIVER_RS_INSTALL_DIR | Should -BeNullOrEmpty
    }

    It 'refuses a plain-HTTP cua-driver installer URL' {
        $env:CUA_INSTALL_DRIVER_URL = 'http://example.invalid/install.ps1'
        $DryRun = $false
        try { { Install-CuaDriver 'cua' $env:CUA_TEST_ROOT 'https://x/r.json' 6>$null } | Should -Throw '*non-HTTPS*' }
        finally { $env:CUA_INSTALL_DRIVER_URL = $null }
    }

    It 'resolves relative artifact URLs against the manifest' {
        Resolve-ArtifactUrl 'a.zip' 'https://x/rel/release-artifacts.json' | Should -Be 'https://x/rel/a.zip'
        Resolve-ArtifactUrl 'https://y/b.zip' 'https://x/r.json' | Should -Be 'https://y/b.zip'
    }
}

Describe 'install.ps1' {
    It 'installs the CLI into -Prefix\bin and verifies the checksum' {
        $prefix = Join-Path $env:CUA_TEST_ROOT 'p1'
        $out = Invoke-Installer @{ CliOnly = $true; Yes = $true; NoOnboarding = $true; Prefix = $prefix }
        Join-Path $prefix 'bin/cua.exe' | Should -Exist
        $out | Should -Match 'Next steps'
        $out | Should -Match 'is not on your PATH'
    }

    # Cua Spaces is macOS-only for now: Windows skips the app with a note.
    It 'dry run: plans the CLI, skips the app (macOS only) and changes nothing' {
        $prefix = Join-Path $env:CUA_TEST_ROOT 'p2'
        $out = Invoke-Installer @{ DryRun = $true; Yes = $true; Prefix = $prefix; Mode = 'host' } @{ CUA_INSTALL_ARCH = 'ARM64' }
        $out | Should -Match 'cua-cli-1.2.3-windows-arm64.zip'
        $out | Should -Match 'Cua Spaces is macOS-only for now'
        $out | Should -Not -Match 'msiexec'
        $out | Should -Not -Match 'cua-spaces-1.2.3-windows'
        $prefix | Should -Not -Exist
    }

    It 'dry run: -AppOnly skips the app with a note and installs nothing' {
        $out = Invoke-Installer @{ DryRun = $true; Yes = $true; AppOnly = $true; Installer = 'nsis'; Mode = 'client' }
        $out | Should -Match 'Cua Spaces is macOS-only for now'
        $out | Should -Not -Match 'setup.exe'
        $out | Should -Not -Match 'cua-cli-1.2.3'
    }

    It 'aborts on a checksum mismatch' {
        $bad = Join-Path $env:CUA_TEST_ROOT 'bad'
        New-Item -ItemType Directory -Force -Path $bad | Out-Null
        Copy-Item (Join-Path (Join-Path $env:CUA_TEST_ROOT 'release') '*') $bad
        $json = Get-Content -Raw (Join-Path $bad 'release-artifacts.json')
        ($json -replace '"sha256":"[0-9a-f]{8}', '"sha256":"00000000') | Set-Content (Join-Path $bad 'release-artifacts.json')
        $prefix = Join-Path $env:CUA_TEST_ROOT 'p3'
        { Invoke-Installer @{ CliOnly = $true; Yes = $true; Prefix = $prefix } @{ CUA_INSTALL_MANIFEST_URL = ConvertTo-FileUrl (Join-Path $bad 'release-artifacts.json') } } |
            Should -Throw '*checksum mismatch*'
        Join-Path $prefix 'bin/cua.exe' | Should -Not -Exist
    }

    It 'rejects -CliOnly with -AppOnly' {
        { Invoke-Installer @{ CliOnly = $true; AppOnly = $true } } | Should -Throw '*mutually exclusive*'
    }

    It '-RequireSignature fails without a verifiable signature' {
        { Invoke-Installer @{ CliOnly = $true; Yes = $true; RequireSignature = $true; Prefix = (Join-Path $env:CUA_TEST_ROOT 'p4') } } |
            Should -Throw '*no verifiable signature*'
    }

    It 'reads the versioned manifest for -Version' {
        $base = ConvertTo-FileUrl $env:CUA_TEST_ROOT
        { Invoke-Installer @{ DryRun = $true; Yes = $true; CliOnly = $true; Version = 'v9.9.9' } @{ CUA_INSTALL_MANIFEST_URL = $null; CUA_INSTALL_BASE_URL = $base } } |
            Should -Throw '*cua-sdk-v9.9.9*release-artifacts.json*'
    }

    It 'verifies a cosign bundle against the exact release workflow and tag' {
        $sig = Join-Path $env:CUA_TEST_ROOT 'csig'
        $bin = Join-Path $env:CUA_TEST_ROOT 'fakebin-cosign'
        New-Item -ItemType Directory -Force -Path $sig, $bin | Out-Null
        Copy-Item (Join-Path $env:CUA_TEST_ROOT 'release/cua-cli-1.2.3-windows-x64.zip') $sig
        Set-Content -LiteralPath (Join-Path $sig 'cua-cli-1.2.3-windows-x64.zip.sigstore.json') -Value '{}'
        & $Python.Source $Tool --component cli --version 1.2.3 --dir $sig --out (Join-Path $sig 'release-artifacts.json')
        $log = Join-Path $env:CUA_TEST_ROOT 'cosign.log'
        $want = 'https://github.com/trycua/cua/.github/workflows/cd-cua-sdk.yml@refs/tags/cua-sdk-v1.2.3'
        if ($IsWindows) {
            # A batch file stands in for cosign.exe on Windows.
            Set-Content -LiteralPath (Join-Path $bin 'cosign.cmd') -Value "@echo off`r`necho %*>>`"$log`"`r`necho %* | findstr /C:`"--certificate-identity $want`" >nul && exit /b 0`r`nexit /b 1"
        } else {
            $fake = Join-Path $bin 'cosign'
            Set-Content -LiteralPath $fake -Value "#!/bin/sh`nprintf '%s\n' `"`$*`" >>'$log'`nfor a in `"`$@`"; do [ `"`$a`" = '$want' ] && exit 0; done`nexit 1"
            chmod +x $fake
        }
        $vars = @{ CUA_INSTALL_MANIFEST_URL = (ConvertTo-FileUrl (Join-Path $sig 'release-artifacts.json')); PATH = "$bin$([IO.Path]::PathSeparator)$env:PATH" }
        Invoke-Installer @{ CliOnly = $true; Yes = $true; RequireSignature = $true; Prefix = (Join-Path $env:CUA_TEST_ROOT 'p9') } $vars | Out-Null
        Get-Content $log | Should -Match ([regex]::Escape("--certificate-identity $want"))
        Get-Content $log | Should -Not -Match 'identity-regexp'
    }

    It 'dry run: Spaces is off by default on Windows' {
        $out = Invoke-Installer @{ DryRun = $true; Yes = $true }
        $out | Should -Match 'cua-cli-1.2.3-windows-x64.zip'
        $out | Should -Not -Match 'msiexec'
        $out | Should -Not -Match 'cua-driver'
    }

    It 'dry run: -Only spaces installs the CLI and skips the app' {
        $out = Invoke-Installer @{ DryRun = $true; Only = 'spaces' }
        $out | Should -Match 'cua-cli-1.2.3-windows-x64.zip'
        $out | Should -Match 'Cua Spaces is macOS-only for now'
        $out | Should -Not -Match 'msiexec'
    }

    It 'dry run: -Select cua-driver,host prints the driver, agents and host steps' {
        $out = Invoke-Installer @{ DryRun = $true; Select = 'cua-driver,host'; Prefix = (Join-Path $env:CUA_TEST_ROOT 'p10') } @{ CUA_INSTALL_NONINTERACTIVE = '1'; CUA_INSTALL_DRIVER_URL = 'https://example.test/driver/install.ps1' }
        $out | Should -Match '\[dry-run\] download https://example.test/driver/install.ps1'
        $out | Should -Match 'cua-driver-install.ps1 -NoPathUpdate'
        $out | Should -Match 'agents setup --cua-driver --agents all --yes --mcp-command .*p10.bin.cua-driver.exe'
        $out | Should -Match '\[dry-run\] .*cua.exe host setup'
        $out | Should -Not -Match 'msiexec'
    }

    It 'rejects unknown items and conflicting selections' {
        { Invoke-Installer @{ DryRun = $true; Select = 'bogus' } } | Should -Throw '*unknown item*'
        { Invoke-Installer @{ DryRun = $true; Select = 'host'; Only = 'host' } } | Should -Throw '*mutually exclusive*'
        { Invoke-Installer @{ DryRun = $true; CliOnly = $true; Select = 'spaces' } } | Should -Throw '*use -Only spaces*'
    }

    It 'dry run: -Mode writes no install-mode file while the app is skipped' {
        $out = Invoke-Installer @{ DryRun = $true; Yes = $true; AppOnly = $true; Mode = 'host' }
        $out | Should -Not -Match 'spaces-install-mode'
    }
}
